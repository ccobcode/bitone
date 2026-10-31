//! 对外接口:路由,中间件,HTTP 处理函数和 WebSocket.

use crate::chains;
use crate::config::Config;
use crate::error::ApiError;
use crate::follow::Follower;
use crate::prices::Prices;
use crate::upstream::Networks;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::handler::HandlerWithoutStateExt;
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bitone_core::Network;
use limit::{ConnectionLimit, RateLimiter};
use metrics::Metrics;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tower_http::services::ServeDir;

mod limit;
mod metrics;
mod query;
mod snapshot;
mod ws;

/// 请求体上限.101 个最长地址也只有约 10 KiB,超过地址上限时先报数量错误.
const MAX_BODY: usize = 64 * 1024;
/// 广播的请求体上限.标准交易最大约 400 KB,十六进制翻倍.
const MAX_BROADCAST_BODY: usize = 1024 * 1024;
/// 快照的请求体上限,放得下 1000 个带合约的条目.
const MAX_SNAPSHOT_BODY: usize = 256 * 1024;
/// 连接上游的时限.连接失败时请求肯定没有发出,广播可以放心换上游,
/// 所以要和总时限区分开,不能让没响应的地址耗尽总时限.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// CoinGecko 拒绝没有 User-Agent 的请求.这里只有网关版本,没有用户信息.
const USER_AGENT: &str = concat!("bitone-gateway/", env!("CARGO_PKG_VERSION"));

/// 见 docs/04-security.md §3.
const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; \
    img-src 'self'; manifest-src 'self'; font-src 'self'; connect-src 'self'; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// 限流中间件算出的客户端 IP,存在请求扩展里.
#[derive(Clone, Copy)]
pub struct ClientIp(IpAddr);

pub struct AppState {
    networks: Arc<Networks>,
    followers: HashMap<Network, Arc<Follower>>,
    prices: Option<Prices>,
    limiter: RateLimiter,
    ws_connections: ConnectionLimit,
    trusted_proxies: HashSet<IpAddr>,
    pub metrics: Metrics,
    /// 退出信号.发送端的 `closed()` 用来等 WebSocket 连接收尾,所以这里不保存接收端.
    pub closing: watch::Sender<bool>,
    poll_interval: Duration,
    heartbeat: Duration,
}

impl AppState {
    pub fn new(config: &Config) -> io::Result<Self> {
        // 不跟随重定向,否则查询的地址会被带到配置之外的服务器.
        let upstream_timeout = Duration::from_millis(config.upstream_timeout_ms);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(upstream_timeout))
            // 跟块的长连接里也有 HTTP 查询,那里没有 `Pool` 的总时限,节点不回应时会一直卡住.
            .timeout(upstream_timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| io::Error::other("创建 HTTP 客户端失败"))?;
        let follow_poll = Duration::from_millis(config.follow_poll_interval_ms);
        Ok(Self {
            networks: Arc::new(chains::networks(&config.networks, &client, upstream_timeout)),
            followers: config
                .networks
                .keys()
                .map(|network| (*network, Arc::new(Follower::new(*network, follow_poll))))
                .collect(),
            prices: config
                .prices
                .as_ref()
                .map(|prices| Prices::new(prices, &client, upstream_timeout)),
            limiter: RateLimiter::new(config.rate_limit_burst, config.rate_limit_per_second),
            ws_connections: ConnectionLimit::new(config.ws_max_connections_per_ip),
            trusted_proxies: config.trusted_proxies.iter().map(|ip| ip.to_canonical()).collect(),
            metrics: Metrics::default(),
            closing: watch::Sender::new(false),
            poll_interval: Duration::from_millis(config.ws_poll_interval_ms),
            heartbeat: Duration::from_millis(config.ws_heartbeat_ms),
        })
    }
}

async fn version(State(state): State<Arc<AppState>>) -> Json<Value> {
    let networks: Vec<&str> = Network::ALL
        .into_iter()
        .filter(|n| state.networks.pool(*n).is_some())
        .map(Network::id)
        .collect();
    let no_history: Vec<&str> = Network::ALL
        .into_iter()
        .filter(|n| state.networks.pool(*n).is_some_and(|pool| !pool.has_history()))
        .map(Network::id)
        .collect();
    Json(json!({"version": env!("CARGO_PKG_VERSION"), "networks": networks, "noHistory": no_history}))
}

async fn rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let ip = limit::client_ip(peer.ip(), &state.trusted_proxies, request.headers());
    if state.limiter.allow(ip) {
        request.extensions_mut().insert(ClientIp(ip));
        next.run(request).await
    } else {
        ApiError::RateLimited.into_response()
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(self)"),
    );
    response
}

async fn record_metrics(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let started = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .record(&method, response.status().as_u16(), started.elapsed());
    response
}

pub fn router(state: Arc<AppState>, web_dir: Option<&std::path::Path>) -> Router {
    // 这些接口只读网络,各条链自己的接口也在这一组.
    let queries = Router::new()
        .route(
            "/snapshot",
            post(snapshot::snapshot).layer(DefaultBodyLimit::max(MAX_SNAPSHOT_BODY)),
        )
        .route("/{network}/balances", post(query::balances))
        .route("/{network}/fees", get(query::fees))
        .route("/{network}/estimate", post(query::estimate))
        .route(
            "/{network}/broadcast",
            post(query::broadcast).layer(DefaultBodyLimit::max(MAX_BROADCAST_BODY)),
        )
        .route("/{network}/tx", post(query::tx))
        .route("/{network}/history", post(query::history))
        .merge(chains::routes())
        .with_state(state.networks.clone());
    // /v1 下的未知路径在这里返回 404,不能落到 SPA 回退上.
    let api = Router::new()
        .route("/version", get(version))
        .route("/prices", get(query::prices))
        .route("/ws", get(ws::upgrade))
        .merge(queries)
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(from_fn_with_state(state.clone(), rate_limit))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state.clone());
    let app = Router::new().nest("/v1", api);
    let app = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let spa = move |uri: Uri| async move {
                // 带扩展名的路径是静态资源,缺失时要返回 404,不能拿 HTML 冒充脚本.
                // nest 接不住 /v1/ 这个路径本身,所以这里也要排除.
                let path = uri.path();
                let is_asset = path.rsplit('/').next().is_some_and(|name| name.contains('.'));
                match tokio::fs::read(&index).await {
                    Ok(html) if !is_asset && !path.starts_with("/v1/") => Html(html).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            };
            app.fallback_service(ServeDir::new(dir).fallback(spa.into_service()))
        }
        None => app.fallback(|| async { StatusCode::NOT_FOUND }),
    };
    // 这两层在最外面,404,429 和 WebSocket 握手也都经过它们.
    app.layer(from_fn(security_headers))
        .layer(from_fn_with_state(state, record_metrics))
}

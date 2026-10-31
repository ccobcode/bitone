//! BitOne 网关。无状态：不存储用户数据，不鉴权，只对接链上数据。
//! 设计见 docs/03-system.md §4，接口格式见 docs/09-gateway-api.md。
//!
//! 隐私约束：这里不依赖任何日志框架，只有 metrics 模块的聚合行和启动提示会写到 stderr。
//! 新增日志时不要写入地址、路径、IP、请求体和上游错误文本。

mod address;
mod api;
mod cache;
pub mod config;
mod error;
mod follow;
mod limit;
mod metrics;
mod prices;
mod raw;
mod upstream;
mod ws;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::handler::HandlerWithoutStateExt;
use axum::http::{HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bitone_core::Network;
use config::Config;
use error::ApiError;
use follow::Follower;
use limit::{ConnectionLimit, RateLimiter};
use metrics::Metrics;
use prices::Prices;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};
use tower_http::services::ServeDir;
use upstream::Pool;

/// 请求体上限。101 个最长地址也只有约 10 KiB，超过地址上限时先报数量错误。
const MAX_BODY: usize = 64 * 1024;
/// 广播的请求体上限。标准交易最大约 400 KB，十六进制翻倍。
const MAX_BROADCAST_BODY: usize = 1024 * 1024;
/// 快照的请求体上限，放得下 1000 个带合约的条目。
const MAX_SNAPSHOT_BODY: usize = 256 * 1024;
/// 连接上游的时限。连接失败时请求肯定没有发出，广播可以放心换上游，
/// 所以要和总时限区分开，不能让没响应的地址耗尽总时限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// CoinGecko 拒绝没有 User-Agent 的请求。这里只有网关版本，没有用户信息。
const USER_AGENT: &str = concat!("bitone-gateway/", env!("CARGO_PKG_VERSION"));
const METRICS_PERIOD: Duration = Duration::from_secs(60);
/// 退出时等待在途请求和 WebSocket 收尾的上限，分别计时。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 见 docs/04-security.md §3。
const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; \
    img-src 'self'; manifest-src 'self'; font-src 'self'; connect-src 'self'; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// 限流中间件算出的客户端 IP，存在请求扩展里。
#[derive(Clone, Copy)]
struct ClientIp(IpAddr);

struct AppState {
    networks: HashMap<Network, Pool>,
    followers: HashMap<Network, Arc<Follower>>,
    prices: Option<Prices>,
    limiter: RateLimiter,
    ws_connections: ConnectionLimit,
    trusted_proxies: HashSet<IpAddr>,
    metrics: Metrics,
    /// 退出信号。发送端的 `closed()` 用来等 WebSocket 连接收尾，所以这里不保存接收端。
    closing: watch::Sender<bool>,
    poll_interval: Duration,
    heartbeat: Duration,
}

impl AppState {
    fn new(config: &Config) -> io::Result<Self> {
        // 不跟随重定向，否则查询的地址会被带到配置之外的服务器。
        let upstream_timeout = Duration::from_millis(config.upstream_timeout_ms);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(upstream_timeout))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| io::Error::other("创建 HTTP 客户端失败"))?;
        let networks: HashMap<Network, Pool> = config
            .networks
            .iter()
            .map(|(network, upstreams)| (*network, Pool::new(*network, upstreams, &client, upstream_timeout)))
            .collect();
        Ok(Self {
            followers: networks
                .keys()
                .map(|network| (*network, Arc::new(Follower::new(*network))))
                .collect(),
            networks,
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

    /// 未知网络和没有配置上游的网络对外没有区别。
    fn network(&self, id: &str) -> Result<(Network, &Pool), ApiError> {
        let network = Network::from_id(id).ok_or(ApiError::UnknownNetwork)?;
        Ok((network, self.networks.get(&network).ok_or(ApiError::UnknownNetwork)?))
    }
}

async fn version(State(state): State<Arc<AppState>>) -> Json<Value> {
    let networks: Vec<&str> = Network::ALL
        .into_iter()
        .filter(|n| state.networks.contains_key(n))
        .map(Network::id)
        .collect();
    let no_history: Vec<&str> = Network::ALL
        .into_iter()
        .filter(|n| state.networks.get(n).is_some_and(|pool| !pool.has_history()))
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

fn router(state: Arc<AppState>, web_dir: Option<&std::path::Path>) -> Router {
    // /v1 下的未知路径在这里返回 404，不能落到 SPA 回退上。
    let api = Router::new()
        .route("/version", get(version))
        .route("/prices", get(api::prices))
        .route("/ws", get(ws::upgrade))
        .route(
            "/snapshot",
            post(api::snapshot).layer(DefaultBodyLimit::max(MAX_SNAPSHOT_BODY)),
        )
        .route("/{network}/balances", post(api::balances))
        .route("/{network}/utxos", post(api::utxos))
        .route("/{network}/raw-txs", post(api::raw_txs))
        .route("/{network}/nonce", post(api::nonce))
        .route("/{network}/resources", post(api::resources))
        .route("/{network}/ref-block", get(api::ref_block))
        .route("/{network}/nonce-account", post(api::nonce_account))
        .route("/{network}/blockhash", get(api::blockhash))
        .route("/{network}/fees", get(api::fees))
        .route("/{network}/estimate", post(api::estimate))
        .route(
            "/{network}/broadcast",
            post(api::broadcast).layer(DefaultBodyLimit::max(MAX_BROADCAST_BODY)),
        )
        .route("/{network}/tx", post(api::tx))
        .route("/{network}/history", post(api::history))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(from_fn_with_state(state.clone(), rate_limit))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state.clone());
    let app = Router::new().nest("/v1", api);
    let app = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let spa = move |uri: Uri| async move {
                // 带扩展名的路径是静态资源，缺失时要返回 404，不能拿 HTML 冒充脚本。
                // nest 接不住 /v1/ 这个路径本身，所以这里也要排除。
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
    // 这两层在最外面，404、429 和 WebSocket 握手也都经过它们。
    app.layer(from_fn(security_headers))
        .layer(from_fn_with_state(state, record_metrics))
}

/// 运行网关，直到 `shutdown` 完成，然后优雅退出。
pub async fn serve(
    config: Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let state = Arc::new(AppState::new(&config)?);
    let app = router(state.clone(), config.web_dir.as_deref());

    let metrics = state.clone();
    let flush = tokio::spawn(async move {
        loop {
            sleep(METRICS_PERIOD).await;
            metrics.metrics.flush();
        }
    });

    let mut closing = state.closing.subscribe();
    let signal = {
        let state = state.clone();
        async move {
            shutdown.await;
            state.closing.send_replace(true);
        }
    };
    let server =
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(signal);
    tokio::select! {
        result = server.into_future() => result?,
        // 卡住的连接不能阻止退出。
        _ = async {
            let _ = closing.wait_for(|closing| *closing).await;
            sleep(SHUTDOWN_GRACE).await;
        } => {}
    }
    // axum 不管已升级的连接，这里等 WebSocket 发完关闭帧。
    drop(closing);
    let _ = timeout(SHUTDOWN_GRACE, state.closing.closed()).await;
    flush.abort();
    state.metrics.flush();
    Ok(())
}

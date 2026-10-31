//! WebSocket，JSON-RPC 2.0，见 docs/09-gateway-api.md。
//!
//! 每个订阅是一个任务，任务句柄只存在于连接的内存里，
//! 连接结束时句柄被丢弃，任务随之中止，这就是「断开即清除」。
//! 地址频道由 follow.rs 共用的跟块任务推送，其余频道各自轮询。
//!
//! 单 IP 的连接数有上限，单连接的订阅数也有上限，轮询上游的成本因此是有界的。

use crate::address;
use crate::error::ApiError;
use crate::follow::{AbortOnDrop, Watcher, current_balances};
use crate::limit::ConnectionSlot;
use crate::prices::PriceMap;
use crate::upstream::Tx;
use crate::{AppState, ClientIp};
use axum::Extension;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::{IntoResponse, Response};
use bitone_core::Network;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval_at, sleep, timeout};

/// 单连接的地址上限，所有订阅合计。
const MAX_ADDRESSES: usize = 1000;
/// 单连接的交易上限，见 docs/03-system.md §4.3。
const MAX_TXS: usize = 100;
/// 单连接的 blocks 和 prices 订阅合计。每个网络一个，再加一个价格，正常用法到不了这个数。
const MAX_FEEDS: usize = Network::ALL.len() + 1;
/// 一条消息的上限，放得下 1000 个最长的地址。
const MAX_MESSAGE: usize = 128 * 1024;
const OUTBOUND_QUEUE: usize = 64;
/// 客户端不读数据时，超过这个时间就断开。
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

pub async fn upgrade(
    upgrade: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
) -> Response {
    let Some(slot) = state.ws_connections.acquire(ip) else {
        return ApiError::RateLimited.into_response();
    };
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(socket, state, ip, slot))
}

#[derive(Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(tag = "channel", rename_all = "lowercase")]
enum SubscribeParams {
    Addresses { network: String, addresses: Vec<String> },
    Tx { network: String, hash: String },
    Blocks { network: String },
    Prices {},
}

fn rpc_result(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn rpc_error(id: Value, code: i32, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// 业务错误带上 HTTP 接口的错误码，放在 data 里。
fn api_error(id: Value, error: &ApiError) -> String {
    let error = json!({"code": error.rpc_code(), "message": error.message(), "data": {"code": error.code()}});
    json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string()
}

/// 单值频道的推送，`params` 是一个对象，这里补上订阅 ID。
fn push(subscription: &str, mut params: Value) -> String {
    params["subscription"] = json!(subscription);
    json!({"jsonrpc": "2.0", "method": "update", "params": params}).to_string()
}

/// 加入这个网络的跟块，一直占着成员资格，直到连接结束或退订。
async fn follow_addresses(
    state: Arc<AppState>,
    network: Network,
    id: String,
    request: Value,
    addresses: Vec<String>,
    out: mpsc::Sender<String>,
) {
    let (Some(pool), Some(follower)) = (state.networks.get(&network), state.followers.get(&network)) else {
        return;
    };
    // 先取基线再回复订阅 ID。客户端收到回复后取一次快照，之后的变化一定会被推送。
    // 基线缺失的地址（上游故障）在下一次查询成功时推送，宁可多推一次，不能漏掉。
    let height = pool.head().await.ok();
    let baseline = current_balances(pool, &addresses).await;
    let watcher = Watcher::new(id.clone(), addresses, out.clone(), height, baseline);
    let _member = follower.join(&state, watcher);
    if out.send(rpc_result(request, json!(id))).await.is_ok() {
        std::future::pending::<()>().await;
    }
}

/// 单值频道的轮询规则。
struct Feed<T> {
    changed: fn(&T, &T) -> bool,
    finished: fn(&T) -> bool,
    params: fn(&T) -> Value,
}

/// 轮询一个值，变化时推送。和地址频道一样，先取基线再回复订阅 ID，
/// 基线取不到时下一次查询成功就推送，宁可多推一次，不能漏掉。
async fn watch<T, Fetch, Output>(
    interval: Duration,
    id: String,
    request: Value,
    out: mpsc::Sender<String>,
    fetch: Fetch,
    feed: Feed<T>,
) where
    Fetch: Fn() -> Output,
    Output: Future<Output = Option<T>>,
{
    let mut last = fetch().await;
    if out.send(rpc_result(request, json!(id))).await.is_err() {
        return;
    }
    while !last.as_ref().is_some_and(feed.finished) {
        sleep(interval).await;
        let Some(value) = fetch().await else { continue };
        if last.as_ref().is_none_or(|last| (feed.changed)(last, &value)) {
            if out.send(push(&id, (feed.params)(&value))).await.is_err() {
                return;
            }
            last = Some(value);
        }
    }
}

/// 订阅占用的额度。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quota {
    Addresses,
    Txs,
    Feeds,
}

struct Subscription {
    quota: Quota,
    units: usize,
    _poll: AbortOnDrop,
}

struct Connection {
    state: Arc<AppState>,
    ip: IpAddr,
    out: mpsc::Sender<String>,
    subscriptions: HashMap<String, Subscription>,
    next_id: u64,
}

impl Connection {
    /// 处理一条消息，返回要立刻发送的回复。subscribe 成功时由轮询任务回复。
    fn handle(&mut self, text: &str) -> Option<String> {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Some(rpc_error(Value::Null, PARSE_ERROR, "解析失败"));
        };
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        // 通知没有 id，不能回复。这里的方法都需要回复，所以直接忽略。
        if id.is_null() {
            return None;
        }
        if !self.state.limiter.allow(self.ip) {
            return Some(api_error(id, &ApiError::RateLimited));
        }
        let request = match serde_json::from_value::<RpcRequest>(value) {
            Ok(request) if request.jsonrpc == "2.0" => request,
            _ => return Some(rpc_error(id, INVALID_REQUEST, "请求无效")),
        };
        match request.method.as_str() {
            "subscribe" => self.subscribe(id, request.params),
            "unsubscribe" => Some(self.unsubscribe(id, &request.params)),
            // 浏览器看不到 WebSocket 的 ping 帧，要靠这个发现连接已经断了。
            "ping" => Some(rpc_result(id, json!("pong"))),
            _ => Some(rpc_error(id, METHOD_NOT_FOUND, "方法不存在")),
        }
    }

    fn used(&self, quota: Quota) -> usize {
        self.subscriptions
            .values()
            .filter(|s| s.quota == quota)
            .map(|s| s.units)
            .sum()
    }

    fn subscribe(&mut self, id: Value, params: Value) -> Option<String> {
        let Ok(params) = serde_json::from_value::<SubscribeParams>(params) else {
            return Some(rpc_error(id, INVALID_PARAMS, "参数无效"));
        };
        if matches!(&params, SubscribeParams::Addresses { addresses, .. } if addresses.is_empty()) {
            return Some(rpc_error(id, INVALID_PARAMS, "参数无效"));
        }
        let subscription = (self.next_id + 1).to_string();
        match self.start(&subscription, id.clone(), params) {
            Ok((quota, units, task)) => {
                self.next_id += 1;
                self.subscriptions.insert(
                    subscription,
                    Subscription {
                        quota,
                        units,
                        _poll: AbortOnDrop(task),
                    },
                );
                None
            }
            Err(error) => Some(api_error(id, &error)),
        }
    }

    /// 校验订阅并启动轮询任务。回复由任务在取到基线后发送。
    fn start(
        &self,
        id: &str,
        request: Value,
        params: SubscribeParams,
    ) -> Result<(Quota, usize, JoinHandle<()>), ApiError> {
        let state = self.state.clone();
        let (id, out, interval) = (id.to_string(), self.out.clone(), state.poll_interval);
        match params {
            SubscribeParams::Addresses { network, addresses } => {
                let (network, _) = state.network(&network)?;
                let addresses = address::checked(network, addresses, MAX_ADDRESSES - self.used(Quota::Addresses))?;
                let units = addresses.len();
                Ok((
                    Quota::Addresses,
                    units,
                    tokio::spawn(follow_addresses(state, network, id, request, addresses, out)),
                ))
            }
            SubscribeParams::Tx { network, hash } => {
                let (network, _) = state.network(&network)?;
                if self.used(Quota::Txs) >= MAX_TXS {
                    return Err(ApiError::TooManySubscriptions);
                }
                if !address::is_valid_hash(network, &hash) {
                    return Err(ApiError::InvalidHash);
                }
                let hashes = [hash];
                let fetch = move || {
                    let (state, hashes) = (state.clone(), hashes.clone());
                    async move {
                        let pool = state.networks.get(&network)?;
                        pool.txs(&hashes).await.ok()?.pop()
                    }
                };
                let feed = Feed {
                    changed: Tx::progressed_from,
                    finished: Tx::settled,
                    params: |tx: &Tx| json!({"tx": tx}),
                };
                Ok((
                    Quota::Txs,
                    1,
                    tokio::spawn(watch(interval, id, request, out, fetch, feed)),
                ))
            }
            SubscribeParams::Blocks { network } => {
                let (network, _) = state.network(&network)?;
                if self.used(Quota::Feeds) >= MAX_FEEDS {
                    return Err(ApiError::TooManySubscriptions);
                }
                let fetch = move || {
                    let state = state.clone();
                    async move { state.networks.get(&network)?.height().await.ok() }
                };
                let feed = Feed {
                    changed: |last, height| last != height,
                    finished: |_| false,
                    params: |height: &u64| json!({"height": height}),
                };
                Ok((
                    Quota::Feeds,
                    1,
                    tokio::spawn(watch(interval, id, request, out, fetch, feed)),
                ))
            }
            SubscribeParams::Prices {} => {
                if self.used(Quota::Feeds) >= MAX_FEEDS {
                    return Err(ApiError::TooManySubscriptions);
                }
                let fetch = move || {
                    let state = state.clone();
                    async move {
                        match &state.prices {
                            Some(prices) => prices.get().await.ok(),
                            None => Some(Default::default()),
                        }
                    }
                };
                let feed = Feed {
                    changed: |last, prices| last != prices,
                    finished: |_| false,
                    params: |prices: &PriceMap| json!({"prices": prices}),
                };
                Ok((
                    Quota::Feeds,
                    1,
                    tokio::spawn(watch(interval, id, request, out, fetch, feed)),
                ))
            }
        }
    }

    /// 参数是订阅 ID，接受字符串、单元素数组和 `{"subscription": ID}` 三种写法。
    fn unsubscribe(&mut self, id: Value, params: &Value) -> String {
        let subscription = params
            .as_str()
            .or_else(|| params.get(0)?.as_str())
            .or_else(|| params.get("subscription")?.as_str());
        match subscription {
            Some(subscription) => rpc_result(id, json!(self.subscriptions.remove(subscription).is_some())),
            None => rpc_error(id, INVALID_PARAMS, "参数无效"),
        }
    }
}

/// `_slot` 占着单 IP 的连接名额，这个函数返回时归还。
async fn run(mut socket: WebSocket, state: Arc<AppState>, ip: IpAddr, _slot: ConnectionSlot) {
    let period = state.heartbeat;
    let mut closing = state.closing.subscribe();
    let (out, mut outbound) = mpsc::channel(OUTBOUND_QUEUE);
    let mut connection = Connection {
        state,
        ip,
        out,
        subscriptions: HashMap::new(),
        next_id: 0,
    };
    let mut heartbeat = interval_at(Instant::now() + period, period);
    let mut last_seen = Instant::now();
    loop {
        let message = tokio::select! {
            incoming = socket.recv() => {
                last_seen = Instant::now();
                match incoming {
                    Some(Ok(Message::Text(text))) => match connection.handle(&text) {
                        Some(reply) => Message::text(reply),
                        None => continue,
                    },
                    Some(Ok(Message::Binary(_))) => Message::text(rpc_error(Value::Null, INVALID_REQUEST, "只接受文本消息")),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                }
            }
            Some(text) = outbound.recv() => Message::text(text),
            _ = heartbeat.tick() => {
                // 连续两个周期没有任何数据，对端已经失联，这里断开才能退出跟块。
                if last_seen.elapsed() > period * 2 {
                    break;
                }
                Message::Ping(Default::default())
            }
            _ = async { let _ = closing.wait_for(|closing| *closing).await; } => {
                Message::Close(Some(CloseFrame { code: close_code::AWAY, reason: "".into() }))
            }
        };
        let closing_now = matches!(message, Message::Close(_));
        if !matches!(timeout(SEND_TIMEOUT, socket.send(message)).await, Ok(Ok(()))) || closing_now {
            break;
        }
    }
}

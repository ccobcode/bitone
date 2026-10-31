//! 上游适配层。一个 trait，四条链各一个实现。查询都走 HTTP，只有跟块用长连接。
//!
//! 适配器只报告「这个上游这次不可用」。reqwest 的错误文本带 URL，
//! URL 里可能有查询地址或 API key，所以错误内容一律丢弃，不传出也不记录。
//!
//! 余额都是已上链的状态：Esplora 只算 chain_stats，EVM 取 latest，Solana 用默认的 finalized。
//!
//! 最终性在这里判断，标准见 docs/03-system.md §9。每条链都有一个纯函数把上游数据
//! 映射成 `Tx`，执行失败的交易永远是 `failed`，不能因为区块已经最终确定就报成 `finalized`。

pub(crate) mod electrum;
mod esplora;
mod evm;
mod solana;
mod tron;

use crate::cache::Cached;
use crate::config::NetworkConfig;
use crate::error::ApiError;
use crate::raw::RawTx;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use bitone_core::{Chain, Network};
use electrum::Electrum;
use esplora::Esplora;
use evm::Evm;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt, TryStreamExt, stream};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use solana::Solana;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tron::Tron;

pub const NATIVE: &str = "native";
/// 单次查询里对同一上游的并发请求数。
const CONCURRENCY: usize = 8;
/// 连接长连接的时限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 手续费随区块变化，几秒的误差没有影响。
const FEES_TTL: Duration = Duration::from_secs(15);
/// 区块相关的公共数据，比出块间隔略短。
const BLOCK_TTL: Duration = Duration::from_secs(3);
/// TronGrid 的 API key 请求头。
const TRON_API_KEY: &str = "tron-pro-api-key";
/// 游标里单个续页标记的长度上限。
const MAX_CURSOR_TOKEN: usize = 512;
/// 全部上游的长连接都断开后，隔这么久再试。其间网关轮询高度。
const WATCH_RETRY: Duration = Duration::from_secs(30);
/// 新块推送停了这么久就当连接已经失效。比最慢的出块间隔长。
const HEAD_SILENCE: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Balance {
    pub address: String,
    pub asset: String,
    pub amount: String,
}

fn balance(address: &str, asset: &str, amount: String) -> Balance {
    Balance {
        address: address.to_string(),
        asset: asset.to_string(),
        amount,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Utxo {
    pub address: String,
    pub txid: String,
    pub vout: u32,
    pub amount: String,
    /// 0 表示还在内存池。
    pub height: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TxStatus {
    Unknown,
    Pending,
    Included,
    Finalized,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tx {
    pub hash: String,
    pub status: TxStatus,
    pub confirmations: u64,
    pub stage: Option<&'static str>,
    pub block: Option<u64>,
    pub fee: Option<String>,
}

impl Tx {
    fn without_block(hash: &str, status: TxStatus) -> Self {
        Self {
            hash: hash.to_string(),
            status,
            confirmations: 0,
            stage: None,
            block: None,
            fee: None,
        }
    }

    fn unknown(hash: &str) -> Self {
        Self::without_block(hash, TxStatus::Unknown)
    }

    fn pending(hash: &str) -> Self {
        Self::without_block(hash, TxStatus::Pending)
    }

    /// 状态不会再变化，可以停止轮询。失败的交易在最终确定之前仍可能被重组。
    pub fn settled(&self) -> bool {
        match self.status {
            TxStatus::Finalized => true,
            TxStatus::Failed => matches!(self.stage, Some("finalized" | "solidified")),
            _ => false,
        }
    }

    /// 只有 Bitcoin 看确认数，其余链看阶段，确认数随区块增长不算状态变化。
    pub fn progressed_from(&self, last: &Tx) -> bool {
        let key = |tx: &Tx| {
            let confirmations = if tx.stage.is_none() { tx.confirmations } else { 0 };
            (tx.status, tx.stage, tx.block, tx.fee.clone(), confirmations)
        };
        key(self) != key(last)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HistoryItem {
    pub hash: String,
    /// 还没有上链时为空。
    pub time: Option<u64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub asset: String,
    pub amount: String,
    pub status: TxStatus,
}

/// 分页游标。一次查询里每个地址可能有多条独立分页的记录流，这里记下每条流的续页标记。
/// 键以地址在请求里的下标开头，流结束后从游标里删除。空表表示第一页。
pub type Cursor = BTreeMap<String, String>;

#[derive(Debug, Default)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub cursor: Cursor,
    /// 已经不可回滚的位置。下次带着它查询，只返回这之后的记录。单位由链决定，客户端原样传回。
    pub checkpoint: u64,
    /// 上次之后没有任何变化，这一页没有查询地址。
    pub unchanged: bool,
}

/// 长连接发给跟块任务的信号。
pub enum Signal {
    /// 新高度。
    Head(u64),
    /// 这些地址有新动静，包括还在内存池的交易。
    Touched(Vec<String>),
    /// 长连接断了，改回轮询高度。
    Lost,
}

/// 校验客户端传回的游标。键和标记都可能被拼进上游请求，所以只放行固定的字符。
pub fn parse_cursor(text: &str, addresses: usize) -> Result<Cursor, ApiError> {
    let cursor: Cursor = serde_json::from_str(text).map_err(|_| ApiError::InvalidRequest)?;
    let safe = |text: &str| {
        text.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'='))
    };
    let valid = cursor.iter().all(|(key, token)| {
        stream_index(key).is_some_and(|index| index < addresses)
            && safe(key)
            && safe(token)
            && !token.is_empty()
            && token.len() <= MAX_CURSOR_TOKEN
    });
    if valid {
        Ok(cursor)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn stream_index(key: &str) -> Option<usize> {
    key.split(':').next()?.parse().ok()
}

/// 这一页要查的流：第一页是全部，之后只查游标里还有下文的。
fn streams(all: impl Iterator<Item = String>, cursor: &Cursor) -> Vec<(String, Option<String>)> {
    if cursor.is_empty() {
        all.map(|key| (key, None)).collect()
    } else {
        cursor
            .iter()
            .map(|(key, token)| (key.clone(), Some(token.clone())))
            .collect()
    }
}

/// 一条流的一页。
struct StreamPage {
    key: String,
    next: Option<String>,
    items: Vec<HistoryItem>,
}

/// 合并各条流。同一笔转账可能同时出现在转出方和转入方的流里。
fn merge_pages(pages: Vec<StreamPage>) -> HistoryPage {
    let mut seen = HashSet::new();
    let mut page = HistoryPage::default();
    for stream in pages {
        page.items
            .extend(stream.items.into_iter().filter(|item| seen.insert(item.clone())));
        if let Some(next) = stream.next {
            page.cursor.insert(stream.key, next);
        }
    }
    // 没有时间的是内存池里的交易，排在最前面。
    page.items.sort_by_key(|item| Reverse(item.time.unwrap_or(u64::MAX)));
    page
}

/// 三条链估算费用的请求，字段都已校验。
pub enum Estimate {
    Evm {
        from: String,
        to: String,
        value: String,
        data: String,
    },
    Tron {
        from: String,
        to: String,
        contract: Option<String>,
        data: Option<String>,
    },
    Solana {
        recipient: String,
        mint: Option<String>,
    },
}

/// 一次广播的结果。
pub enum Broadcast {
    /// 节点接收了，或者这笔交易已经在内存池或链上。
    Accepted,
    /// 节点明确拒绝，带节点原文。
    Rejected(String),
    /// 连接失败，请求没有发出去，可以换下一个上游。
    Unreachable,
    /// 请求可能已经到达节点，结果不明。不能换上游重试，也不能当成功或失败。
    Unknown,
}

impl From<reqwest::Error> for Broadcast {
    fn from(error: reqwest::Error) -> Self {
        if error.is_connect() {
            Broadcast::Unreachable
        } else {
            Broadcast::Unknown
        }
    }
}

/// 节点拒绝后，这笔交易可能其实已经上链：EVM 报 nonce 太低，Bitcoin 报输入已花费，
/// Solana 和 TRON 报已过期。按哈希查到了就算成功，查询失败时仍然按节点的话报拒绝。
async fn rejected(upstream: &dyn Upstream, raw: &RawTx, message: String) -> Broadcast {
    match upstream.txs(std::slice::from_ref(&raw.hash)).await {
        Ok(txs) if txs.iter().any(|tx| tx.status != TxStatus::Unknown) => Broadcast::Accepted,
        _ => Broadcast::Rejected(message),
    }
}

#[derive(Debug)]
pub struct Failed;

impl From<reqwest::Error> for Failed {
    fn from(_: reqwest::Error) -> Self {
        Failed
    }
}

type Query<'a, T> = BoxFuture<'a, Result<T, Failed>>;

fn not_applicable<'a, T>() -> Query<'a, T> {
    Box::pin(async { Err(Failed) })
}

/// 四条链都有的查询是必选方法。只属于某条链的方法有默认实现，
/// 路由层已经按链拦截，不适用的链走不到这里。
pub trait Upstream: Send + Sync {
    /// 查一批地址。每个地址都返回原生币和清单里的全部代币，余额为 0 也返回，顺序固定。
    /// 上游数据有任何异常都算失败，不能把解析不了的内容当成 0。
    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>>;
    /// 最新区块高度。Solana 是 slot。
    fn height(&self) -> Query<'_, u64>;
    fn fees(&self) -> Query<'_, Value>;
    /// 一批交易的状态，顺序和请求相同。
    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>>;
    fn history<'a>(&'a self, addresses: &'a [String], cursor: &'a Cursor, since: Option<u64>)
    -> Query<'a, HistoryPage>;
    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast>;

    /// 保持一条长连接，发来新高度和地址变化，断开时返回。上游没有长连接时立刻返回。
    fn watch(&self, _addresses: watch::Receiver<Vec<String>>, _signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    /// 跟块时取这批地址最近的交易，`previous` 是上一轮的高度。默认取最新一页。
    fn recent<'a>(&'a self, addresses: &'a [String], _previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(async move {
            let cursor = Cursor::new();
            Ok(self.history(addresses, &cursor, None).await?.items)
        })
    }

    /// 内层的 `Err` 是节点拒绝这次调用，带节点原文，换上游也不会有不同结果。
    fn estimate<'a>(&'a self, _request: &'a Estimate) -> Query<'a, Result<Value, String>> {
        not_applicable()
    }
    fn utxos<'a>(&'a self, _addresses: &'a [String]) -> Query<'a, Vec<Utxo>> {
        not_applicable()
    }
    /// 上游没有的交易不出现在结果里。
    fn raw_txs<'a>(&'a self, _txids: &'a [String]) -> Query<'a, BTreeMap<String, String>> {
        not_applicable()
    }
    fn nonces<'a>(&'a self, _addresses: &'a [String]) -> Query<'a, BTreeMap<String, Value>> {
        not_applicable()
    }
    fn resources<'a>(&'a self, _addresses: &'a [String]) -> Query<'a, BTreeMap<String, Value>> {
        not_applicable()
    }
    fn ref_block(&self) -> Query<'_, Value> {
        not_applicable()
    }
    fn nonce_account<'a>(&'a self, _address: &'a str) -> Query<'a, Value> {
        not_applicable()
    }
    fn blockhash(&self) -> Query<'_, Value> {
        not_applicable()
    }
}

/// 订阅节点的新块，收到的高度交给跟块任务。节点在 HTTP 地址上同时提供 WebSocket。
async fn watch_heads(url: String, subscribe: Value, height: fn(&Value) -> Option<u64>, signals: mpsc::Sender<Signal>) {
    let Ok(Ok((mut socket, _))) = timeout(CONNECT_TIMEOUT, connect_async(url)).await else {
        return;
    };
    let subscribe = Message::text(subscribe.to_string());
    if socket.send(subscribe).await.is_err() {
        return;
    }
    while let Ok(Some(Ok(message))) = timeout(HEAD_SILENCE, socket.next()).await {
        let Ok(text) = message.to_text() else { continue };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let Some(head) = value.pointer("/params/result").and_then(height) else {
            continue;
        };
        if signals.send(Signal::Head(head)).await.is_err() {
            return;
        }
    }
}

struct Http {
    client: Client,
    url: String,
    headers: HeaderMap,
}

/// 节点返回的 JSON-RPC 错误。
#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
    data: Value,
}

impl RpcError {
    /// 节点自身的问题不算拒绝：解析错误、方法不存在、内部错误、限流或节点不健康。
    fn is_rejection(&self) -> bool {
        !matches!(self.code, -32700 | -32603 | -32601 | -32600 | -32005)
    }
}

fn rpc_reply(mut reply: Value) -> Option<Result<Value, RpcError>> {
    if let Some(error) = reply.get("error").filter(|error| !error.is_null()) {
        let code = error.get("code")?.as_i64()?;
        let message = error.get("message")?.as_str()?.to_string();
        let data = error.get("data").cloned().unwrap_or(Value::Null);
        return Some(Err(RpcError { code, message, data }));
    }
    Some(Ok(reply.get_mut("result")?.take()))
}

impl Http {
    fn new(client: &Client, url: &str, headers: HeaderMap) -> Self {
        Self {
            client: client.clone(),
            url: url.trim_end_matches('/').to_string(),
            headers,
        }
    }

    fn socket_url(&self) -> String {
        self.url.replacen("http", "ws", 1)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.url))
            .headers(self.headers.clone())
    }

    async fn get(&self, path: &str) -> Result<Value, Failed> {
        let response = self.request(Method::GET, path).send().await?;
        Ok(response.error_for_status()?.json().await?)
    }

    async fn get_query(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, Failed> {
        let response = self.request(Method::GET, path).query(query).send().await?;
        Ok(response.error_for_status()?.json().await?)
    }

    async fn get_text(&self, path: &str) -> Result<String, Failed> {
        let response = self.request(Method::GET, path).send().await?;
        Ok(response.error_for_status()?.text().await?)
    }

    /// 404 表示上游没有这条记录，不算故障。
    async fn find_text(&self, path: &str) -> Result<Option<String>, Failed> {
        let response = self.request(Method::GET, path).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.text().await?))
    }

    async fn post(&self, body: &Value) -> Result<Value, Failed> {
        self.post_to("", body).await
    }

    async fn post_to(&self, path: &str, body: &Value) -> Result<Value, Failed> {
        let response = self.request(Method::POST, path).json(body).send().await?;
        Ok(response.error_for_status()?.json().await?)
    }

    /// 发一个 JSON-RPC 调用，保留连接失败的信息。节点的错误对象在内层返回。
    /// 有的服务商把节点错误放在 4xx 响应里，所以这里只把限流和 5xx 算作服务故障。
    async fn rpc_send(&self, method: &str, params: Value) -> Result<Result<Value, RpcError>, Broadcast> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response: Response = self.request(Method::POST, "").json(&body).send().await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(Broadcast::Unknown);
        }
        let reply: Value = response.json().await.map_err(|_| Broadcast::Unknown)?;
        rpc_reply(reply).ok_or(Broadcast::Unknown)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Result<Value, RpcError>, Failed> {
        self.rpc_send(method, params).await.map_err(|_| Failed)
    }
}

/// 限制并发地跑完一组查询，结果保持输入顺序。
async fn run_all<F, T>(queries: Vec<F>) -> Result<Vec<T>, Failed>
where
    F: Future<Output = Result<Vec<T>, Failed>>,
{
    let nested: Vec<Vec<T>> = stream::iter(queries).buffered(CONCURRENCY).try_collect().await?;
    Ok(nested.into_iter().flatten().collect())
}

/// `0x` 开头的十六进制转十进制字符串。ERC-20 余额是 uint256，放不进任何原生整数。
fn hex_to_decimal(hex: &str) -> Option<String> {
    let hex = hex
        .strip_prefix("0x")
        .filter(|h| !h.is_empty())?
        .trim_start_matches('0');
    if hex.len() > 64 {
        return None;
    }
    // 十进制各位，低位在前。每读一位十六进制就整体乘 16 再加上它。
    let mut digits = vec![0u8];
    for c in hex.chars() {
        let mut carry = c.to_digit(16)?;
        for digit in &mut digits {
            let value = u32::from(*digit) * 16 + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    Some(digits.iter().rev().map(|d| char::from(b'0' + d)).collect())
}

/// 十进制字符串转 `0x` 开头的十六进制，上限 uint256。
pub fn decimal_to_hex(decimal: &str) -> Option<String> {
    if decimal.is_empty() {
        return None;
    }
    // 十六进制各位，低位在前。每读一位十进制就整体乘 10 再加上它。
    let mut digits = vec![0u32];
    for c in decimal.chars() {
        let mut carry = c.to_digit(10)?;
        for digit in &mut digits {
            let value = *digit * 10 + carry;
            *digit = value % 16;
            carry = value / 16;
        }
        while carry > 0 {
            digits.push(carry % 16);
            carry /= 16;
        }
    }
    if digits.len() > 64 {
        return None;
    }
    let hex: String = digits
        .iter()
        .rev()
        .map(|d| char::from_digit(*d, 16).unwrap_or('0'))
        .collect();
    Some(format!("0x{hex}"))
}

fn hex_u64(value: &Value) -> Result<u64, Failed> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or(Failed)?;
    u64::from_str_radix(hex, 16).map_err(|_| Failed)
}

fn hex_u128(value: &Value) -> Result<u128, Failed> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or(Failed)?;
    u128::from_str_radix(hex, 16).map_err(|_| Failed)
}

/// 十进制整数字符串，或者 JSON 整数。
fn decimal(value: &Value) -> Result<String, Failed> {
    match value {
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => Ok(s.clone()),
        Value::Number(n) => n.as_u64().map(|n| n.to_string()).ok_or(Failed),
        _ => Err(Failed),
    }
}

/// 一个网络的全部上游，失败时切换下一个。
pub struct Pool {
    upstreams: Vec<Box<dyn Upstream>>,
    indexed: bool,
    /// 从上次成功的上游开始，这样坏掉的上游不会让每次请求都白等一个超时。
    current: AtomicUsize,
    timeout: Duration,
    fees: Cached<Value>,
    ref_block: Cached<Value>,
    blockhash: Cached<Value>,
    height: Cached<u64>,
}

impl Pool {
    pub fn new(network: Network, config: &NetworkConfig, client: &Client, timeout: Duration) -> Self {
        let mut headers = HeaderMap::new();
        if let Some(mut key) = config
            .tron_api_key
            .as_deref()
            .and_then(|key| HeaderValue::from_str(key).ok())
        {
            key.set_sensitive(true);
            headers.insert(HeaderName::from_static(TRON_API_KEY), key);
        }
        let upstreams = config
            .upstreams
            .iter()
            .map(|url| {
                let http = Http::new(client, url, headers.clone());
                let tokens = config.tokens.clone();
                let mut auth = HeaderMap::new();
                if let Some(mut token) = config
                    .indexer_token
                    .as_deref()
                    .and_then(|token| HeaderValue::from_str(&format!("Bearer {token}")).ok())
                {
                    token.set_sensitive(true);
                    auth.insert(AUTHORIZATION, token);
                }
                let hypersync = config.indexer_token.is_some();
                let indexer = config
                    .indexer
                    .as_deref()
                    .map(|url| Http::new(client, url, auth.clone()));
                let upstream: Box<dyn Upstream> = match network.chain() {
                    Chain::Bitcoin => match Electrum::new(url, network) {
                        Some(electrum) => Box::new(electrum),
                        None => Box::new(Esplora { http, network }),
                    },
                    Chain::Evm => Box::new(Evm {
                        http,
                        tokens,
                        indexer,
                        hypersync,
                    }),
                    Chain::Tron => Box::new(Tron { http, tokens }),
                    Chain::Solana => Box::new(Solana { http, tokens }),
                };
                upstream
            })
            .collect();
        Self {
            upstreams,
            indexed: network.chain() != Chain::Evm || config.indexer.is_some(),
            current: AtomicUsize::new(0),
            timeout,
            fees: Cached::new(FEES_TTL),
            ref_block: Cached::new(BLOCK_TTL),
            blockhash: Cached::new(BLOCK_TTL),
            height: Cached::new(BLOCK_TTL),
        }
    }

    async fn attempt<'a, T>(&'a self, query: impl Fn(&'a dyn Upstream) -> Query<'a, T>) -> Result<T, ApiError> {
        let start = self.current.load(Ordering::Relaxed);
        for offset in 0..self.upstreams.len() {
            let index = (start + offset) % self.upstreams.len();
            if let Ok(Ok(value)) = timeout(self.timeout, query(self.upstreams[index].as_ref())).await {
                self.current.store(index, Ordering::Relaxed);
                return Ok(value);
            }
        }
        Err(ApiError::UpstreamUnavailable)
    }

    pub async fn balances(&self, addresses: &[String]) -> Result<Vec<Balance>, ApiError> {
        self.attempt(|upstream| upstream.balances(addresses)).await
    }

    pub async fn height(&self) -> Result<u64, ApiError> {
        self.height.get(self.attempt(|upstream| upstream.height())).await
    }

    /// 跟块用的高度，不走缓存。
    pub async fn head(&self) -> Result<u64, ApiError> {
        self.attempt(|upstream| upstream.height()).await
    }

    pub async fn fees(&self) -> Result<Value, ApiError> {
        self.fees.get(self.attempt(|upstream| upstream.fees())).await
    }

    pub async fn txs(&self, hashes: &[String]) -> Result<Vec<Tx>, ApiError> {
        self.attempt(|upstream| upstream.txs(hashes)).await
    }

    pub async fn history(
        &self,
        addresses: &[String],
        cursor: &Cursor,
        since: Option<u64>,
    ) -> Result<HistoryPage, ApiError> {
        self.attempt(|upstream| upstream.history(addresses, cursor, since))
            .await
    }

    pub async fn recent(&self, addresses: &[String], previous: u64) -> Result<Vec<HistoryItem>, ApiError> {
        self.attempt(|upstream| upstream.recent(addresses, previous)).await
    }

    /// 依次用每个上游保持长连接，断开就换下一个，直到跟块任务结束。
    pub async fn watch(&self, addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) {
        while !signals.is_closed() {
            for upstream in &self.upstreams {
                upstream.watch(addresses.clone(), signals.clone()).await;
                let _ = signals.send(Signal::Lost).await;
            }
            sleep(WATCH_RETRY).await;
        }
    }

    /// EVM 没有配置索引服务时查不到历史，这是配置决定的，不是临时故障。
    pub fn has_history(&self) -> bool {
        self.indexed
    }

    pub async fn estimate(&self, request: &Estimate) -> Result<Value, ApiError> {
        let reply = self.attempt(|upstream| upstream.estimate(request)).await?;
        reply.map_err(|text| ApiError::call_rejected(&text))
    }

    pub async fn utxos(&self, addresses: &[String]) -> Result<Vec<Utxo>, ApiError> {
        self.attempt(|upstream| upstream.utxos(addresses)).await
    }

    pub async fn raw_txs(&self, txids: &[String]) -> Result<BTreeMap<String, String>, ApiError> {
        self.attempt(|upstream| upstream.raw_txs(txids)).await
    }

    pub async fn nonces(&self, addresses: &[String]) -> Result<BTreeMap<String, Value>, ApiError> {
        self.attempt(|upstream| upstream.nonces(addresses)).await
    }

    pub async fn resources(&self, addresses: &[String]) -> Result<BTreeMap<String, Value>, ApiError> {
        self.attempt(|upstream| upstream.resources(addresses)).await
    }

    pub async fn ref_block(&self) -> Result<Value, ApiError> {
        self.ref_block.get(self.attempt(|upstream| upstream.ref_block())).await
    }

    pub async fn nonce_account(&self, address: &str) -> Result<Value, ApiError> {
        self.attempt(|upstream| upstream.nonce_account(address)).await
    }

    pub async fn blockhash(&self) -> Result<Value, ApiError> {
        self.blockhash.get(self.attempt(|upstream| upstream.blockhash())).await
    }

    /// 广播是唯一会写链的接口，所以不走 `attempt`：只有确定请求没有发出去时才换上游。
    /// 超时和其他不明情况都就地返回，让网页端按结果不明处理，见 docs/03-system.md §7。
    pub async fn broadcast(&self, raw: &RawTx) -> Result<(), ApiError> {
        let start = self.current.load(Ordering::Relaxed);
        for offset in 0..self.upstreams.len() {
            let index = (start + offset) % self.upstreams.len();
            match timeout(self.timeout, self.upstreams[index].broadcast(raw)).await {
                Ok(Broadcast::Accepted) => {
                    self.current.store(index, Ordering::Relaxed);
                    return Ok(());
                }
                Ok(Broadcast::Rejected(message)) => return Err(ApiError::broadcast_rejected(&message)),
                Ok(Broadcast::Unreachable) => {}
                Ok(Broadcast::Unknown) | Err(_) => return Err(ApiError::UpstreamUnavailable),
            }
        }
        Err(ApiError::UpstreamUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(status: TxStatus, stage: Option<&'static str>, confirmations: u64) -> Tx {
        Tx {
            hash: "h".into(),
            status,
            confirmations,
            stage,
            block: Some(1),
            fee: None,
        }
    }

    #[test]
    fn hex_to_decimal_handles_uint256() {
        assert_eq!(hex_to_decimal("0x0").unwrap(), "0");
        assert_eq!(hex_to_decimal("0xff").unwrap(), "255");
        assert_eq!(hex_to_decimal("0xDE0B6B3A7640000").unwrap(), "1000000000000000000");
        assert_eq!(
            hex_to_decimal(&format!("0x{}", "f".repeat(64))).unwrap(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        );
        assert_eq!(hex_to_decimal(&format!("0x{:0>64}", "2a")).unwrap(), "42");
    }

    #[test]
    fn hex_to_decimal_rejects_garbage() {
        // 合约不存在时 eth_call 返回 0x，不能当成余额 0。
        for bad in ["0x", "ff", "", "0xzz", &format!("0x1{}", "0".repeat(64))] {
            assert_eq!(hex_to_decimal(bad), None, "{bad}");
        }
    }

    #[test]
    fn decimal_to_hex_round_trips() {
        for value in ["0", "1", "255", "1000000000000000000"] {
            assert_eq!(hex_to_decimal(&decimal_to_hex(value).unwrap()).unwrap(), value);
        }
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(decimal_to_hex(max).unwrap(), format!("0x{}", "f".repeat(64)));
    }

    #[test]
    fn decimal_to_hex_rejects_garbage() {
        let above_max = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        for bad in ["", "-1", "1.5", "0x10", "abc", above_max] {
            assert_eq!(decimal_to_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn decimal_accepts_only_integers() {
        assert_eq!(decimal(&json!("123")).unwrap(), "123");
        assert_eq!(decimal(&json!(123)).unwrap(), "123");
        for bad in [json!(""), json!("-1"), json!("1.5"), json!(1.5), json!(-1), json!(null)] {
            assert!(decimal(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hex_numbers_reject_garbage() {
        assert_eq!(hex_u64(&json!("0x10")).unwrap(), 16);
        for bad in [json!("0x"), json!("10"), json!(16), json!(null), json!("0xzz")] {
            assert!(hex_u64(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_finalized_or_settled_failures_stop_polling() {
        assert!(tx(TxStatus::Finalized, None, 6).settled());
        assert!(tx(TxStatus::Failed, Some("finalized"), 1).settled());
        assert!(tx(TxStatus::Failed, Some("solidified"), 1).settled());
        // 还能被重组。
        assert!(!tx(TxStatus::Failed, Some("latest"), 1).settled());
        assert!(!tx(TxStatus::Failed, Some("confirmed"), 1).settled());
        assert!(!tx(TxStatus::Included, Some("safe"), 1).settled());
        assert!(!tx(TxStatus::Unknown, None, 0).settled());
    }

    #[test]
    fn confirmations_only_count_as_progress_on_bitcoin() {
        let bitcoin = tx(TxStatus::Included, None, 1);
        assert!(tx(TxStatus::Included, None, 2).progressed_from(&bitcoin));
        let evm = tx(TxStatus::Included, Some("latest"), 1);
        assert!(!tx(TxStatus::Included, Some("latest"), 2).progressed_from(&evm));
        assert!(tx(TxStatus::Included, Some("safe"), 2).progressed_from(&evm));
    }

    #[test]
    fn cursor_only_accepts_known_streams_and_safe_tokens() {
        let good = parse_cursor(r#"{"0":"abc","1:native":"a_b-c=","1:tokentx":"7"}"#, 2).unwrap();
        assert_eq!(good.len(), 3);
        for bad in [
            "不是 JSON",
            "[]",
            r#"{"x":"abc"}"#,
            r#"{"2":"abc"}"#,
            r#"{"0":""}"#,
            r#"{"0":"../../admin"}"#,
            r#"{"0":"a&b=c"}"#,
            r#"{"0:/x":"a"}"#,
        ] {
            assert_eq!(parse_cursor(bad, 2).unwrap_err(), ApiError::InvalidRequest, "{bad}");
        }
        assert!(parse_cursor(&format!(r#"{{"0":"{}"}}"#, "a".repeat(MAX_CURSOR_TOKEN + 1)), 2).is_err());
    }

    #[test]
    fn pages_are_merged_newest_first_without_duplicates() {
        let item = |hash: &str, time: Option<u64>| HistoryItem {
            hash: hash.into(),
            time,
            from: None,
            to: None,
            asset: NATIVE.into(),
            amount: "1".into(),
            status: TxStatus::Included,
        };
        let pages = vec![
            StreamPage {
                key: "0".into(),
                next: Some("n".into()),
                items: vec![item("a", Some(1)), item("b", Some(3))],
            },
            StreamPage {
                key: "1".into(),
                next: None,
                items: vec![item("b", Some(3)), item("c", None)],
            },
        ];
        let page = merge_pages(pages);
        let hashes: Vec<&str> = page.items.iter().map(|item| item.hash.as_str()).collect();
        assert_eq!(hashes, ["c", "b", "a"]);
        assert_eq!(page.cursor, Cursor::from([("0".to_string(), "n".to_string())]));
    }
}

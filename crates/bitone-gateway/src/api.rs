//! HTTP 接口，格式见 docs/09-gateway-api.md。
//!
//! 每个处理函数的顺序相同：网络、链、请求体、业务校验、上游。
//! 校验都在调用上游之前，错误的请求不产生上游成本。

use crate::AppState;
use crate::address;
use crate::error::ApiError;
use crate::prices::PriceMap;
use crate::raw::RawTx;
use crate::upstream::{Estimate, HistoryPage, NATIVE, decimal_to_hex, parse_cursor};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use bitcoin::hex::FromHex;
use bitone_core::{Chain, Network};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;

/// 单次请求的地址上限。
const MAX_ADDRESSES: usize = 100;
/// 单次请求的交易哈希上限。
const MAX_HASHES: usize = 100;
/// 一次快照的条目上限，和 WebSocket 单连接的地址上限一致。
const MAX_SNAPSHOT_ITEMS: usize = 1000;

type Shared = State<Arc<AppState>>;
type Body = Result<Bytes, BytesRejection>;
type Reply = Result<Json<Value>, ApiError>;

fn parse<T: DeserializeOwned>(body: Body) -> Result<T, ApiError> {
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::BodyTooLarge,
        _ => ApiError::InvalidRequest,
    })?;
    serde_json::from_slice(&body).map_err(|_| ApiError::InvalidRequest)
}

fn require(network: Network, chain: Chain) -> Result<(), ApiError> {
    if network.chain() == chain {
        Ok(())
    } else {
        Err(ApiError::WrongChain)
    }
}

#[derive(Deserialize)]
struct AddressesRequest {
    addresses: Vec<String>,
}

#[derive(Deserialize)]
struct AddressRequest {
    address: String,
}

#[derive(Deserialize)]
struct HashesRequest {
    hashes: Vec<String>,
}

#[derive(Deserialize)]
struct TxidsRequest {
    txids: Vec<String>,
}

#[derive(Deserialize)]
struct BroadcastRequest {
    raw: String,
}

#[derive(Deserialize)]
struct HistoryRequest {
    addresses: Vec<String>,
    #[serde(default)]
    cursor: Option<String>,
    /// 上次返回的 `checkpoint`。只查这之后的记录。
    #[serde(default)]
    since: Option<String>,
}

#[derive(Deserialize)]
struct SnapshotRequest {
    items: Vec<SnapshotItem>,
}

#[derive(Deserialize)]
struct SnapshotItem {
    network: String,
    address: String,
    /// 没有时是原生币。
    #[serde(default)]
    contract: Option<String>,
    /// 这个地址上次的 `checkpoint`。
    #[serde(default)]
    since: Option<String>,
    /// 客户端的分批编号。翻页游标绑定一批地址，水位相同也不能合并。
    #[serde(default)]
    group: u32,
}

/// 同一网络、同一水位、同一批的条目一起查询。
#[derive(Default)]
struct SnapshotGroup {
    addresses: Vec<String>,
    assets: HashSet<(String, String)>,
}

#[derive(Deserialize)]
struct EvmEstimate {
    from: String,
    to: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct TronEstimate {
    from: String,
    to: String,
    #[serde(default)]
    contract: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct SolanaEstimate {
    recipient: String,
    #[serde(default)]
    mint: Option<String>,
}

fn addresses_from(network: Network, body: Body) -> Result<Vec<String>, ApiError> {
    let request: AddressesRequest = parse(body)?;
    address::checked(network, request.addresses, MAX_ADDRESSES)
}

pub async fn balances(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    let addresses = addresses_from(network, body)?;
    Ok(Json(json!({"balances": pool.balances(&addresses).await?})))
}

pub async fn utxos(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Bitcoin)?;
    let addresses = addresses_from(network, body)?;
    Ok(Json(json!({"utxos": pool.utxos(&addresses).await?})))
}

pub async fn raw_txs(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Bitcoin)?;
    let request: TxidsRequest = parse(body)?;
    let txids = address::checked_hashes(network, request.txids, MAX_HASHES)?;
    Ok(Json(json!({"txs": pool.raw_txs(&txids).await?})))
}

pub async fn nonce(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Evm)?;
    let addresses = addresses_from(network, body)?;
    Ok(Json(json!({"nonces": pool.nonces(&addresses).await?})))
}

pub async fn resources(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Tron)?;
    let addresses = addresses_from(network, body)?;
    Ok(Json(json!({"resources": pool.resources(&addresses).await?})))
}

pub async fn ref_block(State(state): Shared, Path(network): Path<String>) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Tron)?;
    Ok(Json(pool.ref_block().await?))
}

pub async fn nonce_account(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Solana)?;
    let request: AddressRequest = parse(body)?;
    if !address::is_valid(network, &request.address) {
        return Err(ApiError::InvalidAddress);
    }
    Ok(Json(pool.nonce_account(&request.address).await?))
}

pub async fn blockhash(State(state): Shared, Path(network): Path<String>) -> Reply {
    let (network, pool) = state.network(&network)?;
    require(network, Chain::Solana)?;
    Ok(Json(pool.blockhash().await?))
}

pub async fn fees(State(state): Shared, Path(network): Path<String>) -> Reply {
    let (_, pool) = state.network(&network)?;
    Ok(Json(pool.fees().await?))
}

/// 可选字段空着和不写是一回事。
fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

fn checked_address(network: Network, address: String) -> Result<String, ApiError> {
    if address::is_valid(network, &address) {
        Ok(address)
    } else {
        Err(ApiError::InvalidAddress)
    }
}

fn is_hex(text: &str) -> bool {
    Vec::<u8>::from_hex(text).is_ok()
}

fn estimate_request(network: Network, body: Body) -> Result<Estimate, ApiError> {
    match network.chain() {
        Chain::Bitcoin => Err(ApiError::WrongChain),
        Chain::Evm => {
            let request: EvmEstimate = parse(body)?;
            let value = present(request.value).unwrap_or_else(|| "0".to_string());
            let data = present(request.data).unwrap_or_else(|| "0x".to_string());
            if !data.strip_prefix("0x").is_some_and(is_hex) {
                return Err(ApiError::InvalidRequest);
            }
            Ok(Estimate::Evm {
                from: checked_address(network, request.from)?,
                to: checked_address(network, request.to)?,
                value: decimal_to_hex(&value).ok_or(ApiError::InvalidRequest)?,
                data,
            })
        }
        Chain::Tron => {
            let request: TronEstimate = parse(body)?;
            let contract = present(request.contract)
                .map(|c| checked_address(network, c))
                .transpose()?;
            let data = present(request.data);
            // 合约调用要有调用数据，普通转账不看这个字段。
            if contract.is_some() && !data.as_deref().is_some_and(is_hex) {
                return Err(ApiError::InvalidRequest);
            }
            Ok(Estimate::Tron {
                from: checked_address(network, request.from)?,
                to: checked_address(network, request.to)?,
                contract,
                data,
            })
        }
        Chain::Solana => {
            let request: SolanaEstimate = parse(body)?;
            Ok(Estimate::Solana {
                recipient: checked_address(network, request.recipient)?,
                mint: present(request.mint)
                    .map(|mint| checked_address(network, mint))
                    .transpose()?,
            })
        }
    }
}

pub async fn estimate(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    let request = estimate_request(network, body)?;
    Ok(Json(pool.estimate(&request).await?))
}

pub async fn broadcast(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    let request: BroadcastRequest = parse(body)?;
    let raw = RawTx::parse(network, &request.raw)?;
    pool.broadcast(&raw).await?;
    Ok(Json(json!({"hash": raw.hash, "status": "accepted"})))
}

pub async fn tx(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    let request: HashesRequest = parse(body)?;
    let hashes = address::checked_hashes(network, request.hashes, MAX_HASHES)?;
    Ok(Json(json!({"txs": pool.txs(&hashes).await?})))
}

fn history_reply(page: &HistoryPage) -> Value {
    let cursor = (!page.cursor.is_empty()).then(|| json!(page.cursor).to_string());
    json!({
        "items": page.items,
        "cursor": cursor,
        "checkpoint": page.checkpoint.to_string(),
        "unchanged": page.unchanged,
    })
}

fn asset_key(address: &str, asset: &str) -> (String, String) {
    (address.to_string(), asset.to_ascii_lowercase())
}

/// 一组地址的最新状态。余额和历史分开报告失败，查到的部分照常返回。
async fn snapshot_group(
    state: &AppState,
    network: Network,
    since: Option<u64>,
    group: SnapshotGroup,
) -> Result<Value, ApiError> {
    let (_, pool) = state.network(network.id())?;
    // 先取高度。之后的变化都在它之上，客户端不会丢掉这部分推送，内容重复也只是覆盖。
    let height = pool.height().await?;
    let cursor = Default::default();
    let history = async {
        if pool.has_history() {
            pool.history(&group.addresses, &cursor, since).await.ok()
        } else {
            None
        }
    };
    let (balances, history) = tokio::join!(pool.balances(&group.addresses), history);
    let balances = balances.ok().map(|balances| {
        let wanted = balances
            .into_iter()
            .filter(|row| group.assets.contains(&asset_key(&row.address, &row.asset)));
        wanted.collect::<Vec<_>>()
    });
    Ok(json!({
        "network": network.id(),
        "addresses": group.addresses,
        "height": height,
        "balances": balances,
        "history": history.as_ref().map(history_reply),
    }))
}

/// 一次取回多个网络的最新状态。响应是一行一个 JSON，哪批先查完就先发，慢的网络不拖累其他网络。
/// 一个网络失败不影响其他网络，那一行的 `error` 带错误码。
pub async fn snapshot(State(state): Shared, body: Body) -> Result<Response, ApiError> {
    let request: SnapshotRequest = parse(body)?;
    if request.items.len() > MAX_SNAPSHOT_ITEMS {
        return Err(ApiError::TooManyAddresses);
    }
    let mut groups: BTreeMap<(&'static str, Option<u64>, u32), SnapshotGroup> = BTreeMap::new();
    for item in request.items {
        let (network, _) = state.network(&item.network)?;
        let contract = item.contract.as_deref().filter(|contract| !contract.is_empty());
        if contract.is_some_and(|contract| !address::is_valid(network, contract)) {
            return Err(ApiError::InvalidAddress);
        }
        let since = present(item.since)
            .map(|since| since.parse().map_err(|_| ApiError::InvalidRequest))
            .transpose()?;
        let group = groups.entry((network.id(), since, item.group)).or_default();
        if !group.addresses.contains(&item.address) {
            group.addresses.push(item.address.clone());
        }
        group
            .assets
            .insert(asset_key(&item.address, contract.unwrap_or(NATIVE)));
    }
    let mut queries = Vec::with_capacity(groups.len());
    for ((id, since, _), mut group) in groups {
        let (network, _) = state.network(id)?;
        group.addresses = address::checked(network, group.addresses, MAX_ADDRESSES)?;
        let state = state.clone();
        queries.push(async move {
            let addresses = group.addresses.clone();
            let line = match snapshot_group(&state, network, since, group).await {
                Ok(result) => result,
                Err(error) => json!({"network": id, "addresses": addresses, "error": error.code()}),
            };
            Ok::<_, Infallible>(format!("{line}\n"))
        });
    }
    let groups = queries.len().max(1);
    let lines = stream::iter(queries).buffer_unordered(groups);
    Response::builder()
        .header(CONTENT_TYPE, "application/x-ndjson")
        // 反向代理默认攒够一块才转发，这里要逐行送达。
        .header("x-accel-buffering", "no")
        .body(axum::body::Body::from_stream(lines))
        .map_err(|_| ApiError::InvalidRequest)
}

pub async fn history(State(state): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = state.network(&network)?;
    let request: HistoryRequest = parse(body)?;
    let addresses = address::checked(network, request.addresses, MAX_ADDRESSES)?;
    let cursor = match present(request.cursor) {
        Some(cursor) => parse_cursor(&cursor, addresses.len())?,
        None => Default::default(),
    };
    let since = present(request.since)
        .map(|since| since.parse().map_err(|_| ApiError::InvalidRequest))
        .transpose()?;
    Ok(Json(history_reply(&pool.history(&addresses, &cursor, since).await?)))
}

pub async fn prices(State(state): Shared) -> Reply {
    let prices = match &state.prices {
        Some(feed) => feed.get().await?,
        None => PriceMap::new(),
    };
    Ok(Json(json!({"prices": prices})))
}

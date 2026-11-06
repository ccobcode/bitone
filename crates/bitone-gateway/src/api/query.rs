//! 每条链都有的 HTTP 接口.只属于一条链的接口在这条链的目录里.
//!
//! 每个处理函数的顺序相同:网络,请求体,业务校验,上游.

use super::AppState;
use crate::chains::request::{
    Body, Budget, MAX_ADDRESSES, MAX_HASHES, Reply, Shared, addresses_from, checked, checked_hashes, parse, present,
};
use crate::chains::{raw_tx, spec};
use crate::prices::PriceMap;
use crate::upstream::{HistoryPage, parse_cursor};
use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
struct HashesRequest {
    hashes: Vec<String>,
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
    /// 上次返回的 `checkpoint`.只查这之后的记录.
    #[serde(default)]
    since: Option<String>,
}

pub async fn balances(
    State(networks): Shared,
    Path(network): Path<String>,
    Extension(budget): Extension<Budget>,
    body: Body,
) -> Reply {
    let (network, pool) = networks.get(&network)?;
    let addresses = addresses_from(network, body)?;
    budget.spend(addresses.len())?;
    Ok(Json(json!({"balances": pool.balances(&addresses).await?})))
}

pub async fn fees(State(networks): Shared, Path(network): Path<String>) -> Reply {
    let (_, pool) = networks.get(&network)?;
    Ok(Json(pool.fees().await?))
}

pub async fn estimate(State(networks): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.get(&network)?;
    spec(network.chain()).estimate(network, pool, body).await
}

pub async fn broadcast(State(networks): Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.get(&network)?;
    let request: BroadcastRequest = parse(body)?;
    let raw = raw_tx(network, &request.raw)?;
    pool.broadcast(&raw).await?;
    Ok(Json(json!({"hash": raw.hash, "status": "accepted"})))
}

pub async fn tx(
    State(networks): Shared,
    Path(network): Path<String>,
    Extension(budget): Extension<Budget>,
    body: Body,
) -> Reply {
    let (network, pool) = networks.get(&network)?;
    let request: HashesRequest = parse(body)?;
    let hashes = checked_hashes(network, request.hashes, MAX_HASHES)?;
    budget.spend(hashes.len())?;
    Ok(Json(json!({"txs": pool.txs(&hashes).await?})))
}

pub fn history_reply(page: &HistoryPage) -> Value {
    let cursor = (!page.cursor.is_empty()).then(|| json!(page.cursor).to_string());
    json!({
        "items": page.items,
        "cursor": cursor,
        "checkpoint": page.checkpoint.to_string(),
        "unchanged": page.unchanged,
    })
}

pub async fn history(
    State(networks): Shared,
    Path(network): Path<String>,
    Extension(budget): Extension<Budget>,
    body: Body,
) -> Reply {
    let (network, pool) = networks.get(&network)?;
    let request: HistoryRequest = parse(body)?;
    let addresses = checked(network, request.addresses, MAX_ADDRESSES)?;
    let cursor = match present(request.cursor) {
        Some(cursor) => parse_cursor(&cursor, addresses.len())?,
        None => Default::default(),
    };
    let since = present(request.since)
        .map(|since| since.parse().map_err(|_| crate::error::ApiError::InvalidRequest))
        .transpose()?;
    budget.spend(addresses.len())?;
    Ok(Json(history_reply(&pool.history(&addresses, &cursor, since).await?)))
}

pub async fn prices(State(state): State<Arc<AppState>>) -> Reply {
    let prices = match &state.prices {
        Some(feed) => feed.get().await?,
        None => PriceMap::new(),
    };
    Ok(Json(json!({"prices": prices})))
}

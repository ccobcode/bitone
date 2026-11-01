//! 快照:一次取回多个网络的余额,高度和历史.

use super::query::history_reply;
use super::{AppState, ClientIp};
use crate::chains::is_valid;
use crate::chains::request::{Body, MAX_ADDRESSES, checked, parse, present};
use crate::error::ApiError;
use crate::upstream::{NATIVE, Networks};
use axum::Extension;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use bitone_core::Network;
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;

/// 一次快照的条目上限,和 WebSocket 单连接的地址上限一致.
const MAX_SNAPSHOT_ITEMS: usize = 1000;
/// 一个快照同时查询的组数.组由客户端指定,不能打出任意多的上游并发.
const MAX_SNAPSHOT_QUERIES: usize = Network::ALL.len();

#[derive(Deserialize)]
struct SnapshotRequest {
    items: Vec<SnapshotItem>,
}

#[derive(Deserialize)]
struct SnapshotItem {
    network: String,
    address: String,
    /// 没有时是原生币.
    #[serde(default)]
    contract: Option<String>,
    /// 这个地址上次的 `checkpoint`.
    #[serde(default)]
    since: Option<String>,
    /// 客户端的分批编号.翻页游标绑定一批地址,水位相同也不能合并.
    #[serde(default)]
    group: u32,
}

/// 同一网络,同一水位,同一批的条目一起查询.
#[derive(Default)]
struct SnapshotGroup {
    addresses: Vec<String>,
    assets: HashSet<(String, String)>,
}

fn asset_key(address: &str, asset: &str) -> (String, String) {
    (address.to_string(), asset.to_ascii_lowercase())
}

/// 一组地址的最新状态.余额和历史分开报告失败,查到的部分照常返回.
async fn snapshot_group(
    networks: &Networks,
    network: Network,
    since: Option<u64>,
    group: SnapshotGroup,
) -> Result<Value, ApiError> {
    let (_, pool) = networks.get(network.id())?;
    // 先取高度.之后的变化都在它之上,客户端不会丢掉这部分推送,内容重复也只是覆盖.
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

/// 一次取回多个网络的最新状态.响应是一行一个 JSON,哪批先查完就先发,慢的网络不拖累其他网络.
/// 一个网络失败不影响其他网络,那一行的 `error` 带错误码.每组消耗一个限流令牌,上游成本跟着组数走.
pub async fn snapshot(
    State(state): State<Arc<AppState>>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Body,
) -> Result<Response, ApiError> {
    let networks = &state.networks;
    let request: SnapshotRequest = parse(body)?;
    if request.items.len() > MAX_SNAPSHOT_ITEMS {
        return Err(ApiError::TooManyAddresses);
    }
    let mut groups: BTreeMap<(&'static str, Option<u64>, u32), SnapshotGroup> = BTreeMap::new();
    for item in request.items {
        let (network, _) = networks.get(&item.network)?;
        let contract = item.contract.as_deref().filter(|contract| !contract.is_empty());
        if contract.is_some_and(|contract| !is_valid(network, contract)) {
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
    if !state.limiter.charge(ip, groups.len().saturating_sub(1)) {
        return Err(ApiError::RateLimited);
    }
    let mut queries = Vec::with_capacity(groups.len());
    for ((id, since, _), mut group) in groups {
        let (network, _) = networks.get(id)?;
        group.addresses = checked(network, group.addresses, MAX_ADDRESSES)?;
        let networks = networks.clone();
        queries.push(async move {
            let addresses = group.addresses.clone();
            let line = match snapshot_group(&networks, network, since, group).await {
                Ok(result) => result,
                Err(error) => json!({"network": id, "addresses": addresses, "error": error.code()}),
            };
            Ok::<_, Infallible>(format!("{line}\n"))
        });
    }
    let lines = stream::iter(queries).buffer_unordered(MAX_SNAPSHOT_QUERIES);
    Response::builder()
        .header(CONTENT_TYPE, "application/x-ndjson")
        // 反向代理默认攒够一块才转发,这里要逐行送达.
        .header("x-accel-buffering", "no")
        .body(axum::body::Body::from_stream(lines))
        .map_err(|_| ApiError::InvalidRequest)
}

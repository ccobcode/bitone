//! Bitcoin 的 Esplora 接口.Blockstream 和 mempool.space 都提供.

use super::shape::{FINAL_CONFIRMATIONS, fee_rates, history_item, is_already_known, parse_tx};
use super::{Queries, Utxo};
use crate::upstream::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, RawTx, StreamPage, Tx, TxStatus,
    Upstream, balance, merge_pages, read_text, rejected, run_all, stream_index, streams,
};
use bitcoin::hex::FromHex;
use bitone_core::Network;
use futures_util::future::BoxFuture;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Esplora 确认交易的分页大小.
const CHAIN_PAGE: usize = 25;

pub struct Esplora {
    pub http: Http,
    pub network: Network,
}

/// 还在内存池的交易没有高度,总是算新的.
fn newer_than(tx: &Value, since: Option<u64>) -> bool {
    let height = tx.pointer("/status/block_height").and_then(Value::as_u64);
    since.is_none_or(|since| height.is_none_or(|height| height > since))
}

impl Esplora {
    async fn tip(&self) -> Result<u64, Failed> {
        self.http
            .find_text("/blocks/tip/height")
            .await?
            .ok_or(Failed)?
            .trim()
            .parse()
            .map_err(|_| Failed)
    }

    /// 查询结果用 `Vec` 包一层,和 `run_all` 的形状一致.
    async fn tx_count(&self, address: &str) -> Result<Vec<u64>, Failed> {
        let reply = self.http.get(&format!("/address/{address}")).await?;
        let count = |key: &str| reply.pointer(key).and_then(Value::as_u64).unwrap_or(0);
        Ok(vec![count("/chain_stats/tx_count") + count("/mempool_stats/tx_count")])
    }

    async fn address(&self, address: &str) -> Result<Vec<Balance>, Failed> {
        let reply = self.http.get(&format!("/address/{address}")).await?;
        let stat = |key: &str| {
            reply
                .pointer(&format!("/chain_stats/{key}"))
                .and_then(Value::as_u64)
                .ok_or(Failed)
        };
        let amount = stat("funded_txo_sum")?
            .checked_sub(stat("spent_txo_sum")?)
            .ok_or(Failed)?;
        Ok(vec![balance(address, NATIVE, amount.to_string())])
    }

    async fn tx(&self, hash: &str, tip: u64) -> Result<Vec<Tx>, Failed> {
        let Some(text) = self.http.find_text(&format!("/tx/{hash}")).await? else {
            return Ok(vec![Tx::unknown(hash)]);
        };
        let reply: Value = serde_json::from_str(&text).map_err(|_| Failed)?;
        Ok(vec![parse_tx(hash, &reply, tip)?])
    }

    async fn utxos_of(&self, address: &str) -> Result<Vec<Utxo>, Failed> {
        let reply = self.http.get(&format!("/address/{address}/utxo")).await?;
        let rows = reply.as_array().ok_or(Failed)?;
        rows.iter()
            .map(|row| {
                let status = row.get("status").ok_or(Failed)?;
                let confirmed = status.get("confirmed").and_then(Value::as_bool).ok_or(Failed)?;
                let height = if confirmed {
                    status.get("block_height").and_then(Value::as_u64).ok_or(Failed)?
                } else {
                    0
                };
                Ok(Utxo {
                    address: address.to_string(),
                    txid: row.get("txid").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                    vout: row
                        .get("vout")
                        .and_then(Value::as_u64)
                        .ok_or(Failed)?
                        .try_into()
                        .map_err(|_| Failed)?,
                    amount: row.get("value").and_then(Value::as_u64).ok_or(Failed)?.to_string(),
                    height,
                })
            })
            .collect()
    }

    async fn raw_tx(&self, txid: &str) -> Result<Vec<(String, String)>, Failed> {
        let Some(hex) = self.http.find_text(&format!("/tx/{txid}/hex")).await? else {
            return Ok(Vec::new());
        };
        let hex = hex.trim();
        Vec::<u8>::from_hex(hex).map_err(|_| Failed)?;
        Ok(vec![(txid.to_string(), hex.to_string())])
    }

    async fn stream(
        &self,
        key: String,
        token: Option<String>,
        addresses: &[String],
        own: &HashSet<&str>,
        tip: u64,
        since: Option<u64>,
    ) -> Result<Vec<StreamPage>, Failed> {
        let address = stream_index(&key)
            .and_then(|index| addresses.get(index))
            .ok_or(Failed)?;
        let path = match token {
            Some(last_seen) => format!("/address/{address}/txs/chain/{last_seen}"),
            None => format!("/address/{address}/txs"),
        };
        let reply = self.http.get(&path).await?;
        let txs = reply.as_array().ok_or(Failed)?;
        // 新的在前,碰到 `since` 之前的交易就不用再往后翻了.
        let fresh: Vec<&Value> = txs.iter().filter(|tx| newer_than(tx, since)).collect();
        let items = fresh
            .iter()
            .map(|tx| history_item(tx, own, tip))
            .collect::<Result<Vec<_>, _>>()?;
        // 内存池部分不分页,下一页从最后一笔已确认的交易往后.
        let chain: Vec<&HistoryItem> = items.iter().filter(|item| item.status != TxStatus::Pending).collect();
        let next = (fresh.len() == txs.len() && chain.len() >= CHAIN_PAGE)
            .then(|| chain.last().map(|item| item.hash.clone()))
            .flatten();
        Ok(vec![StreamPage { key, next, items }])
    }

    async fn submit(&self, raw: &RawTx) -> Result<Broadcast, Broadcast> {
        let response = self.http.request(Method::POST, "/tx").body(raw.hex()).send().await?;
        let status = response.status();
        let text = read_text(response).await.map_err(|_| Broadcast::Unknown)?;
        if status.is_success() {
            return Ok(Broadcast::Accepted);
        }
        // Esplora 把节点的拒绝放在 400 里,其他状态码是服务自身的问题.
        if status != StatusCode::BAD_REQUEST {
            return Ok(Broadcast::Unknown);
        }
        if is_already_known(&text) {
            return Ok(Broadcast::Accepted);
        }
        Ok(rejected(self, raw, text).await)
    }
}

impl Upstream for Esplora {
    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>> {
        Box::pin(run_all(addresses.iter().map(|a| self.address(a)).collect()))
    }

    fn height(&self) -> Query<'_, u64> {
        Box::pin(self.tip())
    }

    fn fees(&self) -> Query<'_, Value> {
        Box::pin(async { fee_rates(&self.http.get("/fee-estimates").await?, self.network.mode()) })
    }

    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>> {
        Box::pin(async move {
            let tip = self.tip().await?;
            run_all(hashes.iter().map(|hash| self.tx(hash, tip)).collect()).await
        })
    }

    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(async move {
            let tip = self.tip().await?;
            let checkpoint = (tip + 1).saturating_sub(FINAL_CONFIRMATIONS);
            // 公共节点按 IP 限流,一批地址要逐个查.高度没变就没有新的已确认交易,一个地址也不用查.
            if cursor.is_empty() && since == Some(checkpoint) {
                return Ok(HistoryPage {
                    checkpoint,
                    unchanged: true,
                    ..Default::default()
                });
            }
            let own: HashSet<&str> = addresses.iter().map(String::as_str).collect();
            let own = &own;
            let queries = streams((0..addresses.len()).map(|index| index.to_string()), cursor)
                .into_iter()
                .map(|(key, token)| self.stream(key, token, addresses, own, tip, since))
                .collect();
            let page = merge_pages(run_all(queries).await?);
            Ok(HistoryPage { checkpoint, ..page })
        })
    }

    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast> {
        Box::pin(async move { self.submit(raw).await.unwrap_or_else(|outcome| outcome) })
    }
}

impl Queries for Esplora {
    fn used<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<String>> {
        Box::pin(async move {
            let counts = run_all(addresses.iter().map(|address| self.tx_count(address)).collect()).await?;
            let used = addresses.iter().zip(counts).filter(|(_, count)| *count > 0);
            Ok(used.map(|(address, _)| address.clone()).collect())
        })
    }

    fn utxos<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Utxo>> {
        Box::pin(run_all(addresses.iter().map(|a| self.utxos_of(a)).collect()))
    }

    fn raw_txs<'a>(&'a self, txids: &'a [String]) -> Query<'a, BTreeMap<String, String>> {
        Box::pin(async move {
            let pairs = run_all(txids.iter().map(|txid| self.raw_tx(txid)).collect()).await?;
            Ok(pairs.into_iter().collect())
        })
    }
}

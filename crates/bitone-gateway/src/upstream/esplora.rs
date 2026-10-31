//! Bitcoin。Blockstream 和 mempool.space 都是 Esplora 接口。

use super::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, StreamPage, Tx, TxStatus,
    Upstream, Utxo, balance, merge_pages, rejected, run_all, stream_index, streams,
};
use crate::raw::RawTx;
use bitcoin::hex::FromHex;
use bitone_core::{Mode, Network};
use futures_util::future::BoxFuture;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

/// 完成标准，见 docs/03-system.md §9。
pub(super) const FINAL_CONFIRMATIONS: u64 = 6;
/// Esplora 确认交易的分页大小。
const CHAIN_PAGE: usize = 25;
/// 手续费档位对应的目标确认块数。
pub(super) const FAST_TARGET: u64 = 1;
pub(super) const MEDIUM_TARGET: u64 = 6;
pub(super) const SLOW_TARGET: u64 = 144;
/// 节点报出这些内容时，交易已经在内存池或链上。不同版本的措辞都有。
const ALREADY_KNOWN: [&str; 5] = [
    "txn-already-known",
    "txn-already-in-mempool",
    "txn-same-nonwitness-data-in-mempool",
    "already in block chain",
    "already in utxo set",
];

pub struct Esplora {
    pub http: Http,
    pub network: Network,
}

fn confirmations(tip: u64, height: u64) -> u64 {
    tip.saturating_sub(height) + 1
}

fn status_for(confirmed: bool, confirmations: u64) -> TxStatus {
    match (confirmed, confirmations >= FINAL_CONFIRMATIONS) {
        (false, _) => TxStatus::Pending,
        (true, true) => TxStatus::Finalized,
        (true, false) => TxStatus::Included,
    }
}

/// `/tx/{txid}` 的响应映射成交易状态。
pub(super) fn parse_tx(hash: &str, reply: &Value, tip: u64) -> Result<Tx, Failed> {
    let status = reply.get("status").ok_or(Failed)?;
    if !status.get("confirmed").and_then(Value::as_bool).ok_or(Failed)? {
        return Ok(Tx::pending(hash));
    }
    let block = status.get("block_height").and_then(Value::as_u64).ok_or(Failed)?;
    let fee = reply.get("fee").and_then(Value::as_u64).ok_or(Failed)?;
    let confirmations = confirmations(tip, block);
    Ok(Tx {
        hash: hash.to_string(),
        status: status_for(true, confirmations),
        confirmations,
        stage: None,
        block: Some(block),
        fee: Some(fee.to_string()),
    })
}

/// 取不小于目标的最近档位，没有时取最大的档位。
fn estimate_for(estimates: &BTreeMap<u64, f64>, target: u64) -> Option<f64> {
    estimates
        .range(target..)
        .next()
        .or_else(|| estimates.iter().next_back())
        .map(|(_, rate)| *rate)
}

/// 网页端用整数费率。向上取整，下限 1 sat/vB，低于这个值的交易很多节点不转发。
pub(super) fn fee_rates(reply: &Value, mode: Mode) -> Result<Value, Failed> {
    let mut estimates = BTreeMap::new();
    for (target, rate) in reply.as_object().ok_or(Failed)? {
        let target: u64 = target.parse().map_err(|_| Failed)?;
        estimates.insert(
            target,
            rate.as_f64()
                .filter(|rate| rate.is_finite() && *rate >= 0.0)
                .ok_or(Failed)?,
        );
    }
    // 测试网经常没有估算数据，主网没有就是上游出了问题。
    if estimates.is_empty() && mode == Mode::Mainnet {
        return Err(Failed);
    }
    let rate = |target| estimate_for(&estimates, target).unwrap_or(1.0).ceil().max(1.0) as u64;
    let slow = rate(SLOW_TARGET);
    let medium = rate(MEDIUM_TARGET).max(slow);
    let fast = rate(FAST_TARGET).max(medium);
    Ok(json!({"kind": "bitcoin", "slow": slow, "medium": medium, "fast": fast}))
}

pub(super) fn is_already_known(text: &str) -> bool {
    let text = text.to_lowercase();
    ALREADY_KNOWN.iter().any(|known| text.contains(known))
}

/// 还在内存池的交易没有高度，总是算新的。
fn newer_than(tx: &Value, since: Option<u64>) -> bool {
    let height = tx.pointer("/status/block_height").and_then(Value::as_u64);
    since.is_none_or(|since| height.is_none_or(|height| height > since))
}

fn address_of(output: &Value) -> Option<String> {
    output
        .get("scriptpubkey_address")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `/address/{address}/txs` 里的一笔交易，按这批地址的视角归纳：
/// 输入里有自己的地址算转出，金额是付给别人的部分，否则算转入，金额是收到的部分。
pub(super) fn history_item(tx: &Value, own: &HashSet<&str>, tip: u64) -> Result<HistoryItem, Failed> {
    let hash = tx.get("txid").and_then(Value::as_str).ok_or(Failed)?.to_string();
    let status = tx.get("status").ok_or(Failed)?;
    let confirmed = status.get("confirmed").and_then(Value::as_bool).ok_or(Failed)?;
    let height = status.get("block_height").and_then(Value::as_u64);
    if confirmed && height.is_none() {
        return Err(Failed);
    }

    // coinbase 的 prevout 是 null。
    let vin = tx.get("vin").and_then(Value::as_array).ok_or(Failed)?;
    let inputs: Vec<String> = vin
        .iter()
        .filter_map(|input| address_of(input.get("prevout")?))
        .collect();
    let vout = tx.get("vout").and_then(Value::as_array).ok_or(Failed)?;
    let outputs = vout
        .iter()
        .map(|output| {
            Ok((
                address_of(output),
                output.get("value").and_then(Value::as_u64).ok_or(Failed)?,
            ))
        })
        .collect::<Result<Vec<_>, Failed>>()?;
    let is_own = |address: &Option<String>| address.as_deref().is_some_and(|address| own.contains(address));
    let (mine, others): (Vec<_>, Vec<_>) = outputs.iter().partition(|(address, _)| is_own(address));
    let total = |list: &[&(Option<String>, u64)]| {
        list.iter()
            .try_fold(0u64, |sum, (_, value)| sum.checked_add(*value))
            .ok_or(Failed)
    };
    // 数据输出没有地址，不算收款方。
    let first = |list: &[&(Option<String>, u64)]| list.iter().find_map(|(address, _)| address.clone());

    let (from, to, amount) = match inputs.iter().find(|input| own.contains(input.as_str())) {
        Some(sender) => match first(&others) {
            Some(recipient) => (Some(sender.clone()), Some(recipient), total(&others)?),
            // 没有付给别人，是转给自己。
            None => (Some(sender.clone()), first(&mine), total(&mine)?),
        },
        None => (inputs.first().cloned(), first(&mine), total(&mine)?),
    };
    Ok(HistoryItem {
        hash,
        time: status.get("block_time").and_then(Value::as_u64),
        from,
        to,
        asset: NATIVE.to_string(),
        amount: amount.to_string(),
        status: status_for(confirmed, height.map_or(0, |height| confirmations(tip, height))),
    })
}

impl Esplora {
    async fn tip(&self) -> Result<u64, Failed> {
        self.http
            .get_text("/blocks/tip/height")
            .await?
            .trim()
            .parse()
            .map_err(|_| Failed)
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
        // 新的在前，碰到水位之前的交易就不用再往后翻了。
        let fresh: Vec<&Value> = txs.iter().filter(|tx| newer_than(tx, since)).collect();
        let items = fresh
            .iter()
            .map(|tx| history_item(tx, own, tip))
            .collect::<Result<Vec<_>, _>>()?;
        // 内存池部分不分页，下一页从最后一笔已确认的交易往后。
        let chain: Vec<&HistoryItem> = items.iter().filter(|item| item.status != TxStatus::Pending).collect();
        let next = (fresh.len() == txs.len() && chain.len() >= CHAIN_PAGE)
            .then(|| chain.last().map(|item| item.hash.clone()))
            .flatten();
        Ok(vec![StreamPage { key, next, items }])
    }

    async fn submit(&self, raw: &RawTx) -> Result<Broadcast, Broadcast> {
        let response = self.http.request(Method::POST, "/tx").body(raw.hex()).send().await?;
        let status = response.status();
        let text = response.text().await.map_err(|_| Broadcast::Unknown)?;
        if status.is_success() {
            return Ok(Broadcast::Accepted);
        }
        // Esplora 把节点的拒绝放在 400 里，其他状态码是服务自身的问题。
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
            // 公共节点按 IP 限流，一批地址要逐个查。高度没变就没有新的已确认交易，一个地址也不用查。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmed_at(height: u64) -> Value {
        json!({"fee": 141, "status": {"confirmed": true, "block_height": height, "block_time": 1700000000}})
    }

    #[test]
    fn finalized_takes_six_confirmations() {
        let tip = 100;
        let tx = parse_tx("h", &confirmed_at(96), tip).unwrap();
        assert_eq!((tx.status, tx.confirmations), (TxStatus::Included, 5));
        let tx = parse_tx("h", &confirmed_at(95), tip).unwrap();
        assert_eq!((tx.status, tx.confirmations), (TxStatus::Finalized, 6));
        assert_eq!(tx.stage, None, "Bitcoin 没有阶段");
        assert_eq!(tx.fee.as_deref(), Some("141"));
        assert_eq!(tx.block, Some(95));
    }

    #[test]
    fn a_tip_behind_the_tx_still_counts_one_confirmation() {
        assert_eq!(parse_tx("h", &confirmed_at(101), 100).unwrap().confirmations, 1);
    }

    #[test]
    fn mempool_is_pending_with_no_block() {
        let tx = parse_tx("h", &json!({"status": {"confirmed": false}, "fee": 1}), 100).unwrap();
        assert_eq!((tx.status, tx.confirmations, tx.block), (TxStatus::Pending, 0, None));
    }

    #[test]
    fn malformed_status_is_an_upstream_failure() {
        for bad in [
            json!({}),
            json!({"status": {}}),
            json!({"status": {"confirmed": true}, "fee": 1}),
        ] {
            assert!(parse_tx("h", &bad, 100).is_err(), "{bad}");
        }
    }

    #[test]
    fn fee_rates_round_up_and_stay_ordered() {
        let reply = json!({"1": 8.2, "6": 3.0, "144": 0.125, "1008": 0.09});
        assert_eq!(
            fee_rates(&reply, Mode::Mainnet).unwrap(),
            json!({"kind": "bitcoin", "slow": 1, "medium": 3, "fast": 9})
        );
        // 估算不一定单调。
        let reply = json!({"1": 2, "6": 5, "144": 9});
        assert_eq!(
            fee_rates(&reply, Mode::Mainnet).unwrap(),
            json!({"kind": "bitcoin", "slow": 9, "medium": 9, "fast": 9})
        );
    }

    #[test]
    fn fee_rates_fall_back_to_the_nearest_target() {
        let reply = json!({"2": 5, "25": 3});
        assert_eq!(
            fee_rates(&reply, Mode::Mainnet).unwrap(),
            json!({"kind": "bitcoin", "slow": 3, "medium": 3, "fast": 5})
        );
    }

    #[test]
    fn empty_estimates_are_a_failure_only_on_mainnet() {
        assert!(fee_rates(&json!({}), Mode::Mainnet).is_err());
        assert_eq!(
            fee_rates(&json!({}), Mode::Testnet).unwrap(),
            json!({"kind": "bitcoin", "slow": 1, "medium": 1, "fast": 1})
        );
        assert!(fee_rates(&json!({"x": 1}), Mode::Testnet).is_err());
        assert!(fee_rates(&json!({"1": "1"}), Mode::Testnet).is_err());
    }

    #[test]
    fn already_known_covers_the_node_wording() {
        for text in [
            r#"sendrawtransaction RPC error: {"code":-27,"message":"Transaction already in block chain"}"#,
            r#"sendrawtransaction RPC error: {"code":-27,"message":"Transaction outputs already in utxo set"}"#,
            r#"sendrawtransaction RPC error: {"code":-26,"message":"txn-already-known"}"#,
            r#"sendrawtransaction RPC error: {"code":-26,"message":"txn-already-in-mempool"}"#,
        ] {
            assert!(is_already_known(text), "{text}");
        }
        for text in [
            "bad-txns-inputs-missingorspent",
            "txn-mempool-conflict",
            "min relay fee not met",
        ] {
            assert!(!is_already_known(text), "{text}");
        }
    }

    fn output(address: &str, value: u64) -> Value {
        json!({"scriptpubkey_address": address, "value": value})
    }

    fn sample(vin: Vec<Value>, vout: Vec<Value>) -> Value {
        let status = json!({"confirmed": true, "block_height": 95, "block_time": 1700000000});
        json!({"txid": "t", "status": status, "vin": vin, "vout": vout})
    }

    #[test]
    fn incoming_counts_what_the_batch_received() {
        let own = HashSet::from(["me"]);
        let tx = sample(
            vec![json!({"prevout": output("them", 900)})],
            vec![output("me", 500), output("rest", 300)],
        );
        let item = history_item(&tx, &own, 100).unwrap();
        assert_eq!((item.from.as_deref(), item.to.as_deref()), (Some("them"), Some("me")));
        assert_eq!(item.amount, "500");
        assert_eq!(item.status, TxStatus::Finalized);
        assert_eq!(item.time, Some(1700000000));
    }

    #[test]
    fn outgoing_counts_what_left_the_batch() {
        let own = HashSet::from(["me", "change"]);
        let tx = sample(
            vec![json!({"prevout": output("me", 900)})],
            vec![output("change", 300), output("them", 500)],
        );
        let item = history_item(&tx, &own, 100).unwrap();
        assert_eq!((item.from.as_deref(), item.to.as_deref()), (Some("me"), Some("them")));
        assert_eq!(item.amount, "500", "不含找零和手续费");
    }

    #[test]
    fn self_transfer_keeps_the_moved_amount() {
        let own = HashSet::from(["me", "other"]);
        let tx = sample(vec![json!({"prevout": output("me", 900)})], vec![output("other", 800)]);
        let item = history_item(&tx, &own, 100).unwrap();
        assert_eq!((item.to.as_deref(), item.amount.as_str()), (Some("other"), "800"));
    }

    #[test]
    fn coinbase_has_no_sender_and_data_outputs_are_not_recipients() {
        let own = HashSet::from(["me"]);
        let tx = sample(
            vec![json!({"prevout": null})],
            vec![json!({"value": 0}), output("me", 500)],
        );
        let item = history_item(&tx, &own, 100).unwrap();
        assert_eq!((item.from, item.to.as_deref()), (None, Some("me")));
    }

    #[test]
    fn mempool_history_has_no_time() {
        let own = HashSet::from(["me"]);
        let mut tx = sample(vec![json!({"prevout": output("them", 9)})], vec![output("me", 5)]);
        tx["status"] = json!({"confirmed": false});
        let item = history_item(&tx, &own, 100).unwrap();
        assert_eq!((item.status, item.time), (TxStatus::Pending, None));
    }
}

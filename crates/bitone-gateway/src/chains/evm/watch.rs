//! EVM 的跟块。

use super::adapter::{BATCH_SIZE, Evm, TRANSFER, evm_call, item_status, log_topic, topic_address};
use crate::upstream::{
    Failed, HistoryItem, NATIVE, Signal, hex_to_decimal, hex_u64, next_message, open_socket, run_all, send_call,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

/// 跟块时最多回看的区块数。这里要取完整区块，只在地址有变化时才查。
const MAX_SCAN: u64 = 32;
/// 跟块时往回多看的区块数。原生币隔几秒才查一次，发现变化时已经过了好几个块，
/// BSC 一秒两个块。重复的记录由客户端按标识合并。
const SCAN_BACK: u64 = 16;
/// 查原生币余额的最短间隔。
const NATIVE_CHECK: Duration = Duration::from_secs(3);

/// 有地址还没有原生币基线。基线在订阅之后才取到，这段时间的变化要靠对账找回来。
fn needs_baseline(natives: &HashMap<String, String>, owners: &HashMap<String, String>) -> bool {
    owners.values().any(|address| !natives.contains_key(address))
}

impl Evm {
    /// 一批地址的原生币余额。比完整区块轻得多，还能发现区块交易列表里没有的内部转账。
    async fn native_balances(&self, addresses: &[String]) -> Result<HashMap<String, String>, Failed> {
        let calls: Vec<(&str, Value)> = addresses.iter().map(|a| evm_call(a, NATIVE)).collect();
        let results = run_all(calls.chunks(BATCH_SIZE).map(|chunk| self.batch(chunk)).collect()).await?;
        addresses
            .iter()
            .zip(&results)
            .map(|(address, hex)| Ok((address.clone(), hex.as_str().ok_or(Failed)?.to_string())))
            .collect()
    }

    /// 跟块。`newHeads` 给高度。代币转账用 `logs` 订阅，由节点按地址过滤，内部转账也在里面，
    /// `removed` 的日志同样算动静。原生币转账没有日志，内部转账也不在区块的交易列表里，
    /// 免费节点又没有 trace，所以出块时批量查一次原生币余额。
    pub(super) async fn follow(
        &self,
        mut addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut socket = open_socket(self.http.socket_url()).await?;
        send_call(&mut socket, 1, "eth_subscribe", json!(["newHeads"])).await?;
        let mut next_id = 2;
        // 等回复的订阅。被拒绝时这条连接收不到这些推送，换上游或者退回轮询。
        let mut subscribing = HashSet::from([1]);
        let mut filters: Vec<String> = Vec::new();
        let mut owners: HashMap<String, String> = HashMap::new();
        let mut natives: HashMap<String, String> = HashMap::new();
        let mut checked = Instant::now();
        addresses.mark_changed();
        loop {
            tokio::select! {
                message = next_message(&mut socket) => {
                    let message = message?;
                    let id = message.get("id").and_then(Value::as_u64);
                    let pending = id.is_some_and(|id| subscribing.remove(&id));
                    if pending && message.get("error").is_some_and(|error| !error.is_null()) {
                        return Err(Failed);
                    }
                    if let Some(filter) = message.get("result").and_then(Value::as_str) {
                        if id.is_some_and(|id| id > 1) {
                            filters.push(filter.to_string());
                        }
                        continue;
                    }
                    let Some(result) = message.pointer("/params/result") else { continue };
                    if result.get("topics").is_some() {
                        let parties = [result.pointer("/topics/1"), result.pointer("/topics/2")];
                        let touched: Vec<String> = parties
                            .into_iter()
                            .filter_map(|topic| topic_address(topic).ok())
                            .filter_map(|party| owners.get(&party).cloned())
                            .collect();
                        signals.send(Signal::Touched(touched)).await.map_err(|_| Failed)?;
                    } else if let Some(number) = result.get("number") {
                        signals.send(Signal::Head(hex_u64(number)?)).await.map_err(|_| Failed)?;
                        let fresh = needs_baseline(&natives, &owners);
                        if fresh || checked.elapsed() >= NATIVE_CHECK {
                            checked = Instant::now();
                            let watched: Vec<String> = owners.values().cloned().collect();
                            let current = self.native_balances(&watched).await?;
                            let touched: Vec<String> = current
                                .iter()
                                .filter(|(address, amount)| natives.get(*address).is_some_and(|last| last != *amount))
                                .map(|(address, _)| address.clone())
                                .collect();
                            natives = current;
                            signals.send(Signal::Touched(touched)).await.map_err(|_| Failed)?;
                            // 这里的基线比订阅的基线晚，两者之间的变化要靠对账找回来。
                            if fresh {
                                signals.send(Signal::Reconcile).await.map_err(|_| Failed)?;
                            }
                        }
                    }
                }
                changed = addresses.changed() => {
                    changed.map_err(|_| Failed)?;
                    let list = addresses.borrow_and_update().clone();
                    owners = list.iter().map(|a| (a.to_ascii_lowercase(), a.clone())).collect();
                    for id in filters.drain(..) {
                        send_call(&mut socket, next_id, "eth_unsubscribe", json!([id])).await?;
                        next_id += 1;
                    }
                    if self.tokens.is_empty() || list.is_empty() {
                        continue;
                    }
                    let own: Vec<String> = list.iter().map(|address| log_topic(address)).collect();
                    for topics in [json!([TRANSFER, own, null]), json!([TRANSFER, null, own])] {
                        let filter = json!({"address": self.tokens, "topics": topics});
                        send_call(&mut socket, next_id, "eth_subscribe", json!(["logs", filter])).await?;
                        subscribing.insert(next_id);
                        next_id += 1;
                    }
                }
            }
        }
    }

    /// 直接从节点取最近的交易。索引服务有延迟，BSC 和 Polygon 也没有免费的，所以跟块不用它。
    // ponytail: 合约内部转入的原生币在区块里看不到，只更新余额，交易记录要等索引服务。
    // 没有索引服务的网络要显示这部分时接 trace 接口。
    pub(super) async fn scan(&self, addresses: &[String], previous: u64) -> Result<Vec<HistoryItem>, Failed> {
        let tips = self.tips().await?;
        let oldest = tips.latest.saturating_sub(MAX_SCAN - 1);
        // 中间有看不到的块时不能报成功，失败时 `txs` 是空，客户端自己补取。
        if previous + 1 < oldest {
            return Err(Failed);
        }
        let first = previous.saturating_sub(SCAN_BACK).max(oldest).min(tips.latest);
        let own: HashSet<String> = addresses.iter().map(|a| a.to_ascii_lowercase()).collect();
        let hex = |block: u64| format!("{block:#x}");

        let mut calls: Vec<(&str, Value)> = (first..=tips.latest)
            .map(|block| ("eth_getBlockByNumber", json!([hex(block), true])))
            .collect();
        let blocks = calls.len();
        if !self.tokens.is_empty() {
            let topics: Vec<String> = addresses.iter().map(|address| log_topic(address)).collect();
            let sent = json!([TRANSFER, topics, null]);
            let received = json!([TRANSFER, null, topics]);
            for topics in [sent, received] {
                let filter = json!({
                    "fromBlock": hex(first),
                    "toBlock": hex(tips.latest),
                    "address": self.tokens,
                    "topics": topics,
                });
                calls.push(("eth_getLogs", json!([filter])));
            }
        }
        let results = self.batch(&calls).await?;
        let (blocks, logs) = results.split_at(blocks);

        let mut times = HashMap::new();
        let mut native = Vec::new();
        for block in blocks {
            let number = hex_u64(block.get("number").ok_or(Failed)?)?;
            times.insert(number, hex_u64(block.get("timestamp").ok_or(Failed)?)?);
            let rows = block.get("transactions").and_then(Value::as_array).ok_or(Failed)?;
            for tx in rows {
                let party = |key: &str| tx.get(key).and_then(Value::as_str).map(str::to_ascii_lowercase);
                let (from, to) = (party("from"), party("to"));
                let mine = |party: &Option<String>| party.as_ref().is_some_and(|party| own.contains(party));
                if mine(&from) || mine(&to) {
                    native.push((number, tx, from, to));
                }
            }
        }
        let receipts: Vec<(&str, Value)> = native
            .iter()
            .map(|(_, tx, ..)| ("eth_getTransactionReceipt", json!([tx.get("hash")])))
            .collect();
        let receipts = if receipts.is_empty() {
            Vec::new()
        } else {
            self.batch(&receipts).await?
        };

        let mut items = Vec::new();
        for ((block, tx, from, to), receipt) in native.into_iter().zip(&receipts) {
            let failed = receipt.get("status").and_then(Value::as_str) == Some("0x0");
            let amount = hex_to_decimal(tx.get("value").and_then(Value::as_str).ok_or(Failed)?).ok_or(Failed)?;
            // 和索引服务的规则一样：金额为 0 的成功调用不是转账。
            if amount == "0" && !failed {
                continue;
            }
            items.push(HistoryItem {
                hash: tx.get("hash").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                time: times.get(&block).copied(),
                from,
                to,
                asset: NATIVE.to_string(),
                amount,
                status: item_status(failed, block, &tips),
                flows: None,
            });
        }
        for log in logs.iter().flat_map(|logs| logs.as_array().into_iter().flatten()) {
            let contract = log.get("address").and_then(Value::as_str).ok_or(Failed)?;
            let Some(asset) = self.tokens.iter().find(|token| token.eq_ignore_ascii_case(contract)) else {
                continue;
            };
            let block = hex_u64(log.get("blockNumber").ok_or(Failed)?)?;
            items.push(HistoryItem {
                hash: log
                    .get("transactionHash")
                    .and_then(Value::as_str)
                    .ok_or(Failed)?
                    .to_string(),
                time: times.get(&block).copied(),
                from: Some(topic_address(log.pointer("/topics/1"))?),
                to: Some(topic_address(log.pointer("/topics/2"))?),
                asset: asset.clone(),
                amount: hex_to_decimal(log.get("data").and_then(Value::as_str).ok_or(Failed)?).ok_or(Failed)?,
                status: item_status(false, block, &tips),
                flows: None,
            });
        }
        // 自己转给自己的代币会同时出现在转出和转入两次查询里。
        let mut seen = HashSet::new();
        items.retain(|item| seen.insert(item.clone()));
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners(list: &[&str]) -> HashMap<String, String> {
        list.iter()
            .map(|address| (address.to_string(), address.to_string()))
            .collect()
    }

    /// 地址等量替换时数量不变，比较数量会跳过对账。
    #[test]
    fn a_replaced_address_needs_a_baseline() {
        let natives = HashMap::from([("a".to_string(), "0x1".to_string())]);
        assert!(!needs_baseline(&natives, &owners(&["a"])));
        assert!(needs_baseline(&natives, &owners(&["b"])));
        assert!(!needs_baseline(&natives, &owners(&[])), "地址变少不用对账");
    }
}

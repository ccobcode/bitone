//! TRON 的跟块.

use super::adapter::Tron;
use crate::upstream::{Failed, Signal};
use bitcoin::base58;
use bitcoin::hex::DisplayHex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep};

/// TRC-20 的 Transfer 事件.
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// 查新块的间隔.TRON 三秒一个块,一轮要三次调用,这样一条网络一天约 4 万次.
const BLOCK_POLL: Duration = Duration::from_secs(6);
/// 一轮最多取的区块数.`getblockbylimitnext` 一次最多 100 个.
const MAX_BLOCKS: u64 = 100;
/// 内部转入的 TRX 不在区块和日志里,隔这么久对一次余额.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// base58 地址转成 `41` 开头的十六进制,区块里的地址是这种写法.
fn hex_address(address: &str) -> Result<String, Failed> {
    let bytes = base58::decode_check(address).map_err(|_| Failed)?;
    Ok(bytes.to_lower_hex_string())
}

/// 这批区块里,付款方或收款方是这些地址的交易.
fn parties_in(blocks: &Value, owners: &HashMap<String, String>) -> Vec<String> {
    let mut touched = Vec::new();
    for block in blocks.get("block").and_then(Value::as_array).into_iter().flatten() {
        for tx in block
            .get("transactions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = tx.pointer("/raw_data/contract/0/parameter/value");
            for key in ["owner_address", "to_address"] {
                let party = value.and_then(|value| value.get(key)).and_then(Value::as_str);
                touched.extend(party.and_then(|party| owners.get(party)).cloned());
            }
        }
    }
    touched
}

impl Tron {
    /// 跟块.TRON 节点没有订阅接口,只能轮询,但一轮的请求数和地址数无关:
    /// 区块里有原生币转账和全部发起方,代币转入靠 `eth_getLogs` 按收款地址过滤.
    /// 合约内部转入的 TRX 这两处都看不到,靠定期对账.
    pub(super) async fn follow(
        &self,
        addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut last = self.tip().await?;
        signals.send(Signal::Head(last)).await.map_err(|_| Failed)?;
        // 从这个高度开始解析,订阅的基线到这里之间的变化要靠对账找回来.
        signals.send(Signal::Reconcile).await.map_err(|_| Failed)?;
        let mut reconciled = Instant::now();
        // 刚上链的交易可能还没进 TronGrid 的索引,这些地址下次对账时再看一遍.
        let mut again: HashSet<String> = HashSet::new();
        loop {
            sleep(BLOCK_POLL).await;
            let tip = self.tip().await?;
            if tip <= last {
                continue;
            }
            let list = addresses.borrow().clone();
            let owners: HashMap<String, String> = list
                .iter()
                .map(|address| Ok((hex_address(address)?, address.clone())))
                .collect::<Result<_, Failed>>()?;
            let first = (last + 1).max(tip.saturating_sub(MAX_BLOCKS - 1));
            let body = json!({"startNum": first, "endNum": tip + 1});
            let blocks = self.http.post_to("/wallet/getblockbylimitnext", &body).await?;
            let mut touched = parties_in(&blocks, &owners);

            let own: Vec<String> = owners.keys().map(|hex| format!("0x{:0>64}", &hex[2..])).collect();
            let filter = json!({
                "fromBlock": format!("{first:#x}"),
                "toBlock": format!("{tip:#x}"),
                "topics": [TRANSFER, null, own],
            });
            let call = json!({"jsonrpc": "2.0", "id": 1, "method": "eth_getLogs", "params": [filter]});
            let logs = self.http.post_to("/jsonrpc", &call).await?;
            for log in logs.get("result").and_then(Value::as_array).ok_or(Failed)? {
                let topic = log.pointer("/topics/2").and_then(Value::as_str).ok_or(Failed)?;
                let tail = topic.get(topic.len().saturating_sub(40)..).ok_or(Failed)?;
                touched.extend(owners.get(&format!("41{tail}")).cloned());
            }

            last = tip;
            again.extend(touched.iter().cloned());
            signals.send(Signal::Block(tip, touched)).await.map_err(|_| Failed)?;
            if reconciled.elapsed() >= RECONCILE_INTERVAL {
                reconciled = Instant::now();
                let recheck = std::mem::take(&mut again).into_iter().collect();
                signals.send(Signal::Touched(recheck)).await.map_err(|_| Failed)?;
                signals.send(Signal::Reconcile).await.map_err(|_| Failed)?;
            }
        }
    }
}

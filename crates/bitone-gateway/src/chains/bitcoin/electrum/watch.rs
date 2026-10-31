//! Electrum 的跟块.

use super::{Electrum, address_hash, by_address};
use crate::upstream::{Failed, Signal};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, timeout};

/// 长连接的心跳.服务器会断开长时间没有请求的连接,连接死了也要靠它发现.
const PING_INTERVAL: Duration = Duration::from_secs(60);
/// 长连接上一次请求的时限.这里没有 `Pool` 的总时限,服务器不回应时要自己放弃.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// 一批订阅的地址数.公共服务器处理得慢,时限按批算.
const SUBSCRIBE_BATCH: usize = 100;

/// 把一条推送转给跟块任务.没有 id 的消息只有这两种.
async fn forward(
    message: &Value,
    owners: &HashMap<String, String>,
    signals: &mpsc::Sender<Signal>,
) -> Result<(), Failed> {
    let params = message.get("params");
    let signal = match message.get("method").and_then(Value::as_str) {
        Some("blockchain.headers.subscribe") => {
            let height = params
                .and_then(|params| params.pointer("/0/height"))
                .and_then(Value::as_u64);
            Signal::Head(height.ok_or(Failed)?)
        }
        Some("blockchain.scripthash.subscribe") => {
            let hash = params.and_then(|params| params.get(0)).and_then(Value::as_str);
            // 退订之前已经发出的推送还会到,这时地址已经没有人看了.
            let Some(owner) = hash.and_then(|hash| owners.get(hash)) else {
                return Ok(());
            };
            Signal::Touched(vec![owner.clone()])
        }
        _ => return Ok(()),
    };
    signals.send(signal).await.map_err(|_| Failed)
}

impl Electrum {
    /// 一条长连接:订阅区块头和全部地址,把推送转给跟块任务.推送包含内存池里的交易.
    pub(super) async fn follow(
        &self,
        mut addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut session = timeout(CALL_TIMEOUT, self.open()).await.map_err(|_| Failed)??;
        let tip = timeout(CALL_TIMEOUT, session.tip()).await.map_err(|_| Failed)??;
        signals.send(Signal::Head(tip)).await.map_err(|_| Failed)?;
        let mut owners: HashMap<String, String> = HashMap::new();
        let mut known: HashSet<String> = HashSet::new();
        let mut ping = interval(PING_INTERVAL);
        ping.tick().await;
        loop {
            let list: HashSet<String> = addresses.borrow_and_update().iter().cloned().collect();
            let fresh: Vec<String> = list.difference(&known).cloned().collect();
            for chunk in fresh.chunks(SUBSCRIBE_BATCH) {
                let calls = by_address("blockchain.scripthash.subscribe", chunk)?;
                let replies = timeout(CALL_TIMEOUT, session.call(&calls))
                    .await
                    .map_err(|_| Failed)??;
                for reply in replies {
                    reply.map_err(|_| Failed)?;
                }
            }
            for address in &fresh {
                owners.insert(address_hash(address)?, address.clone());
            }
            known.extend(fresh.iter().cloned());
            // 基线比这里的订阅早,换连接时还可能错过了推送.这段时间的变化服务器不会再说,补查一次.
            if !fresh.is_empty() {
                signals.send(Signal::Touched(fresh)).await.map_err(|_| Failed)?;
            }
            let stale: Vec<String> = known.difference(&list).cloned().collect();
            for chunk in stale.chunks(SUBSCRIBE_BATCH) {
                let calls = by_address("blockchain.scripthash.unsubscribe", chunk)?;
                timeout(CALL_TIMEOUT, session.call(&calls))
                    .await
                    .map_err(|_| Failed)??;
            }
            if !stale.is_empty() {
                owners.retain(|_, address| list.contains(address));
                known.retain(|address| list.contains(address));
            }
            let pushes = std::mem::take(&mut session.pushes);
            for push in &pushes {
                forward(push, &owners, &signals).await?;
            }
            tokio::select! {
                message = session.read_message() => forward(&message?, &owners, &signals).await?,
                changed = addresses.changed() => changed.map_err(|_| Failed)?,
                _ = ping.tick() => {
                    timeout(CALL_TIMEOUT, session.one("server.ping", json!([])))
                        .await
                        .map_err(|_| Failed)??;
                }
            }
        }
    }
}

//! Electrum 的跟块。

use super::{Electrum, address_hash, by_address};
use crate::upstream::{Failed, Signal};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, timeout};

/// 长连接的心跳。服务器会断开长时间没有请求的连接，连接死了也要靠它发现。
const PING_INTERVAL: Duration = Duration::from_secs(60);
const PING_TIMEOUT: Duration = Duration::from_secs(15);

/// 把一条推送转给跟块任务。没有 id 的消息只有这两种。
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
            let owner = hash.and_then(|hash| owners.get(hash)).ok_or(Failed)?;
            Signal::Touched(vec![owner.clone()])
        }
        _ => return Ok(()),
    };
    signals.send(signal).await.map_err(|_| Failed)
}

impl Electrum {
    /// 一条长连接：订阅区块头和全部地址，把推送转给跟块任务。推送包含内存池里的交易。
    pub(super) async fn follow(
        &self,
        mut addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut session = self.open().await?;
        let tip = session.tip().await?;
        signals.send(Signal::Head(tip)).await.map_err(|_| Failed)?;
        let mut owners: HashMap<String, String> = HashMap::new();
        let mut known: HashSet<String> = HashSet::new();
        let mut ping = interval(PING_INTERVAL);
        ping.tick().await;
        let mut connected = true;
        loop {
            let fresh: Vec<String> = addresses
                .borrow_and_update()
                .iter()
                .filter(|address| !known.contains(*address))
                .cloned()
                .collect();
            if !fresh.is_empty() {
                let calls = by_address("blockchain.scripthash.subscribe", &fresh)?;
                for reply in session.call(&calls).await? {
                    reply.map_err(|_| Failed)?;
                }
                for address in &fresh {
                    owners.insert(address_hash(address)?, address.clone());
                }
                known.extend(fresh.iter().cloned());
                // 上一条连接断开期间可能错过推送，换连接后补一次。
                if connected {
                    signals.send(Signal::Touched(fresh)).await.map_err(|_| Failed)?;
                }
            }
            connected = false;
            let pushes = std::mem::take(&mut session.pushes);
            for push in &pushes {
                forward(push, &owners, &signals).await?;
            }
            tokio::select! {
                message = session.read_message() => forward(&message?, &owners, &signals).await?,
                changed = addresses.changed() => changed.map_err(|_| Failed)?,
                _ = ping.tick() => {
                    timeout(PING_TIMEOUT, session.one("server.ping", json!([])))
                        .await
                        .map_err(|_| Failed)??;
                }
            }
        }
    }
}

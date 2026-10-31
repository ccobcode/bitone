//! Solana 的跟块。

use super::adapter::Solana;
use crate::upstream::{Failed, Signal, Socket, next_message, open_socket, send_call};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, watch};

/// 长连接上的订阅。回复和推送都只有编号，要自己对应回地址。
#[derive(Default)]
struct Watching {
    next: u64,
    owners: HashSet<String>,
    accounts: HashSet<String>,
    requests: HashMap<u64, String>,
    subscriptions: HashMap<u64, String>,
}

impl Watching {
    /// 新的请求编号。1 留给 `slotSubscribe`。
    fn request(&mut self, owner: &str) -> u64 {
        self.next += 1;
        let id = self.next + 1;
        self.requests.insert(id, owner.to_string());
        id
    }
}

impl Solana {
    /// 订阅一个账户：账户本身的余额和提到它的交易。代币在独立的账户上，要分别订阅。
    /// 一个订阅只能写一个地址，公共节点也不开放 `blockSubscribe`，所以逐个订阅。
    async fn watch_owner(&self, socket: &mut Socket, state: &mut Watching, owner: &str) -> Result<(), Failed> {
        let mut accounts = vec![owner.to_string()];
        for key in self.token_accounts(0, owner).await? {
            accounts.extend(key.split(':').nth(1).map(str::to_string));
        }
        let options = json!({"commitment": "confirmed", "encoding": "base64"});
        for account in accounts {
            if state.accounts.insert(account.clone()) {
                let id = state.request(owner);
                send_call(socket, id, "accountSubscribe", json!([account, options])).await?;
            }
        }
        if state.owners.insert(owner.to_string()) {
            let id = state.request(owner);
            let filter = json!({"mentions": [owner]});
            send_call(
                socket,
                id,
                "logsSubscribe",
                json!([filter, {"commitment": "confirmed"}]),
            )
            .await?;
        }
        Ok(())
    }

    /// 跟块。`slotSubscribe` 给高度，账户和日志订阅由节点推送变化，不用查询。
    /// 新的代币账户创建时会提到所有者，所以收到日志时重新看一次它的代币账户。
    pub(super) async fn follow(
        &self,
        mut addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut socket = open_socket(self.http.socket_url()).await?;
        send_call(&mut socket, 1, "slotSubscribe", json!([])).await?;
        let mut state = Watching::default();
        addresses.mark_changed();
        loop {
            tokio::select! {
                message = next_message(&mut socket) => {
                    let message = message?;
                    if let (Some(id), Some(result)) = (
                        message.get("id").and_then(Value::as_u64),
                        message.get("result").and_then(Value::as_u64),
                    ) {
                        if let Some(owner) = state.requests.remove(&id) {
                            state.subscriptions.insert(result, owner);
                            // 全部生效后对账一次，订阅生效之前的变化节点不会推送。
                            if state.requests.is_empty() {
                                signals.send(Signal::Reconcile).await.map_err(|_| Failed)?;
                            }
                        }
                        continue;
                    }
                    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
                    let params = message.get("params");
                    if method == "slotNotification" {
                        let slot = params.and_then(|params| params.pointer("/result/slot")).and_then(Value::as_u64);
                        signals.send(Signal::Head(slot.ok_or(Failed)?)).await.map_err(|_| Failed)?;
                        continue;
                    }
                    let subscription = params.and_then(|params| params.get("subscription")).and_then(Value::as_u64);
                    let Some(owner) = subscription.and_then(|id| state.subscriptions.get(&id).cloned()) else {
                        continue;
                    };
                    signals.send(Signal::Touched(vec![owner.clone()])).await.map_err(|_| Failed)?;
                    if method == "logsNotification" {
                        self.watch_owner(&mut socket, &mut state, &owner).await?;
                    }
                }
                changed = addresses.changed() => {
                    changed.map_err(|_| Failed)?;
                    let list = addresses.borrow_and_update().clone();
                    for owner in &list {
                        if !state.owners.contains(owner) {
                            self.watch_owner(&mut socket, &mut state, owner).await?;
                        }
                    }
                }
            }
        }
    }
}

//! 跟踪 Solana 的新区块和地址变化.

use super::adapter::Solana;
use crate::upstream::{Failed, Signal, Socket, next_message, open_socket, send_call};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::future::pending;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior, interval};

/// 两次查代币账户的最短间隔.公共节点按 IP 限流,这个查询不能跟着日志走.
const LOOKUP_PACE: Duration = Duration::from_secs(2);
/// 还有代币账户没查到时,隔这么久也要对账.查询一直失败不能让地址等下去.
const RECONCILE_PACE: Duration = Duration::from_secs(30);

/// 一个订阅属于谁,退订用哪个方法.
struct Subscription {
    owner: String,
    unsubscribe: &'static str,
}

/// 长连接上的订阅.回复和推送都只有编号,要自己对应回地址.
#[derive(Default)]
struct Watching {
    next: u64,
    owners: HashSet<String>,
    /// 账户到所有者.
    accounts: HashMap<String, String>,
    requests: HashMap<u64, Subscription>,
    subscriptions: HashMap<u64, Subscription>,
    /// 要看一遍代币账户的所有者.
    stale: HashSet<String>,
    /// 有新的订阅还没对账.订阅生效之前的变化节点不会推送.
    unsettled: bool,
}

impl Watching {
    /// 新的请求编号.1 留给 `slotSubscribe`.
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next + 1
    }

    fn request(&mut self, owner: &str, unsubscribe: &'static str) -> u64 {
        let id = self.id();
        let owner = owner.to_string();
        self.requests.insert(id, Subscription { owner, unsubscribe });
        self.unsettled = true;
        id
    }

    async fn watch_account(&mut self, socket: &mut Socket, owner: &str, account: &str) -> Result<(), Failed> {
        if self.accounts.contains_key(account) {
            return Ok(());
        }
        self.accounts.insert(account.to_string(), owner.to_string());
        let id = self.request(owner, "accountUnsubscribe");
        let options = json!({"commitment": "confirmed", "encoding": "base64"});
        send_call(socket, id, "accountSubscribe", json!([account, options])).await
    }

    /// 订阅一个地址本身的余额和提到它的交易,代币账户之后再查.
    /// 一个订阅只能写一个地址,公共节点也不开放 `blockSubscribe`,所以逐个订阅.
    async fn watch_owner(&mut self, socket: &mut Socket, owner: &str) -> Result<(), Failed> {
        self.owners.insert(owner.to_string());
        self.stale.insert(owner.to_string());
        self.watch_account(socket, owner, owner).await?;
        let id = self.request(owner, "logsUnsubscribe");
        let filter = json!({"mentions": [owner]});
        let options = json!({"commitment": "confirmed"});
        send_call(socket, id, "logsSubscribe", json!([filter, options])).await
    }

    async fn forget(&mut self, socket: &mut Socket, list: &HashSet<String>) -> Result<(), Failed> {
        let gone: Vec<u64> = self
            .subscriptions
            .iter()
            .filter(|(_, subscription)| !list.contains(&subscription.owner))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(subscription) = self.subscriptions.remove(&id) {
                self.unsubscribe(socket, id, subscription).await?;
            }
        }
        self.owners.retain(|owner| list.contains(owner));
        self.stale.retain(|owner| list.contains(owner));
        self.accounts.retain(|_, owner| list.contains(owner));
        Ok(())
    }

    async fn unsubscribe(&mut self, socket: &mut Socket, id: u64, subscription: Subscription) -> Result<(), Failed> {
        let call = self.id();
        send_call(socket, call, subscription.unsubscribe, json!([id])).await
    }
}

type Lookup<'a> = BoxFuture<'a, (String, Result<Vec<String>, Failed>)>;

/// 等正在进行的查询.没有查询时一直等,`select!` 走其他分支.
async fn finish(lookup: &mut Option<Lookup<'_>>) -> (String, Result<Vec<String>, Failed>) {
    match lookup {
        Some(running) => running.await,
        None => pending().await,
    }
}

impl Solana {
    /// 跟踪新区块.`slotSubscribe` 给高度,账户和日志订阅由节点推送变化,不用查询.
    /// 新的代币账户创建时会提到所有者,所以收到日志时重新看一次它的代币账户.这个查询在后台一次
    /// 跑一个,既不能挡住读推送,失败了也只影响这个地址,下一拍再试.
    pub(super) async fn follow(
        &self,
        mut addresses: watch::Receiver<Vec<String>>,
        signals: mpsc::Sender<Signal>,
    ) -> Result<(), Failed> {
        let mut socket = open_socket(self.http.socket_url()).await?;
        send_call(&mut socket, 1, "slotSubscribe", json!([])).await?;
        let mut state = Watching::default();
        let mut lookup: Option<Lookup<'_>> = None;
        let mut pace = interval(LOOKUP_PACE);
        pace.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut reconciled = Instant::now();
        addresses.mark_changed();
        loop {
            let waited = state.stale.is_empty() || reconciled.elapsed() >= RECONCILE_PACE;
            if state.unsettled && state.requests.is_empty() && lookup.is_none() && waited {
                state.unsettled = false;
                reconciled = Instant::now();
                signals.send(Signal::Reconcile).await.map_err(|_| Failed)?;
            }
            tokio::select! {
                message = next_message(&mut socket) => {
                    let message = message?;
                    let id = message.get("id").and_then(Value::as_u64);
                    let rejected = message.get("error").is_some_and(|error| !error.is_null());
                    if rejected && id.is_some_and(|id| id == 1 || state.requests.contains_key(&id)) {
                        return Err(Failed);
                    }
                    if let (Some(id), Some(result)) = (id, message.get("result").and_then(Value::as_u64)) {
                        if let Some(subscription) = state.requests.remove(&id) {
                            // 等回复期间地址可能已经没有人看了.
                            if state.owners.contains(&subscription.owner) {
                                state.subscriptions.insert(result, subscription);
                            } else {
                                state.unsubscribe(&mut socket, result, subscription).await?;
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
                    let Some(owner) = subscription.and_then(|id| state.subscriptions.get(&id)) else {
                        continue;
                    };
                    let owner = owner.owner.clone();
                    signals.send(Signal::Touched(vec![owner.clone()])).await.map_err(|_| Failed)?;
                    if method == "logsNotification" {
                        state.stale.insert(owner);
                    }
                }
                changed = addresses.changed() => {
                    changed.map_err(|_| Failed)?;
                    let list: HashSet<String> = addresses.borrow_and_update().iter().cloned().collect();
                    state.forget(&mut socket, &list).await?;
                    for owner in &list {
                        if !state.owners.contains(owner) {
                            state.watch_owner(&mut socket, owner).await?;
                        }
                    }
                }
                (owner, found) = finish(&mut lookup) => {
                    lookup = None;
                    let Ok(keys) = found else {
                        state.stale.insert(owner);
                        continue;
                    };
                    if !state.owners.contains(&owner) {
                        continue;
                    }
                    for key in keys {
                        if let Some(account) = key.split(':').nth(1) {
                            state.watch_account(&mut socket, &owner, account).await?;
                        }
                    }
                }
                _ = pace.tick(), if lookup.is_none() && !state.stale.is_empty() => {
                    if let Some(owner) = state.stale.iter().next().cloned() {
                        state.stale.remove(&owner);
                        lookup = Some(Box::pin(async move {
                            let found = self.token_accounts(0, &owner).await;
                            (owner, found)
                        }));
                    }
                }
            }
        }
    }
}

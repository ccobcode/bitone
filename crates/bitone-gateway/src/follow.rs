//! 跟块。每个网络一个任务，所有订阅共用，第一个订阅到来时启动，最后一个离开时停止。
//!
//! 客户端只订阅网关。上游有长连接就等推送，没有就轮询高度，这些差异都留在 upstream 里。
//! 每轮只查一次全部被订阅的地址，余额变了或上游说地址有动静，才取这个订阅者的交易。
//! 所以订阅者再多，轮询上游的成本也只随不重复的地址数增长。
//!
//! 每条推送带上一轮的高度，客户端发现接不上就重新取快照。

use crate::AppState;
use crate::upstream::{Balance, HistoryItem, Pool, Signal};
use bitone_core::{Chain, Network};
use futures_util::future::join_all;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};

/// 每批查询的地址数，与 HTTP 接口的单次上限一致。
const CHUNK: usize = 100;
const SIGNAL_QUEUE: usize = 64;

/// 两轮之间的最短间隔。BSC 和 Solana 不到一秒一个块，没有必要每块都查。
fn floor(chain: Chain) -> Duration {
    match chain {
        Chain::Bitcoin | Chain::Evm => Duration::from_secs(3),
        // ponytail: 这两条链查余额要逐个地址调用，成本随地址数增长。
        // 用户多了以后 TRON 改成解析区块，Solana 改成 accountSubscribe。
        Chain::Tron | Chain::Solana => Duration::from_secs(6),
    }
}

/// 丢弃时中止任务。
pub struct AbortOnDrop(pub JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 分批查询当前余额，按地址分组。第一次失败就结束本轮，上游故障时不逐批等超时。
/// 没查到的地址不出现在结果里。
pub async fn current_balances(pool: &Pool, addresses: &[String]) -> HashMap<String, Vec<Balance>> {
    let mut grouped: HashMap<String, Vec<Balance>> = HashMap::new();
    for chunk in addresses.chunks(CHUNK) {
        let Ok(balances) = pool.balances(chunk).await else {
            break;
        };
        for balance in balances {
            grouped.entry(balance.address.clone()).or_default().push(balance);
        }
    }
    grouped
}

struct Progress {
    height: Option<u64>,
    balances: HashMap<String, Vec<Balance>>,
}

/// 一个订阅。基线要在回复订阅 ID 之前取好，客户端随后取的快照才不会漏掉变化。
/// 高度要在基线之前取，两者之间出块时下一轮一定会重查。
pub struct Watcher {
    subscription: String,
    addresses: Vec<String>,
    out: mpsc::Sender<String>,
    progress: Mutex<Progress>,
}

impl Watcher {
    pub fn new(
        subscription: String,
        addresses: Vec<String>,
        out: mpsc::Sender<String>,
        height: Option<u64>,
        balances: HashMap<String, Vec<Balance>>,
    ) -> Self {
        Self {
            subscription,
            addresses,
            out,
            progress: Mutex::new(Progress { height, balances }),
        }
    }

    fn update(&self, height: u64, previous: u64, balances: &[Balance], txs: Option<&[HistoryItem]>) -> String {
        let params = json!({
            "subscription": self.subscription,
            "height": height,
            "previous": previous,
            "balances": balances,
            "txs": txs,
        });
        json!({"jsonrpc": "2.0", "method": "update", "params": params}).to_string()
    }

    /// 给这个订阅发一轮结果。发不出去时什么都不记，下一轮连同这次的变化一起发。
    /// 交易没取到时 `txs` 是空，余额照常推送，客户端自己补取。
    async fn notify(
        &self,
        pool: &Pool,
        height: u64,
        current: &HashMap<String, Vec<Balance>>,
        touched: &HashSet<String>,
    ) {
        let (last, changed) = {
            let progress = self.progress.lock().expect("跟块状态被污染");
            let changed: Vec<&String> = self
                .addresses
                .iter()
                .filter(|address| current.get(*address) != progress.balances.get(*address))
                .collect();
            (progress.height, changed)
        };
        let hit = !changed.is_empty() || self.addresses.iter().any(|address| touched.contains(address));
        if !hit && last == Some(height) {
            return;
        }
        let previous = last.unwrap_or(height);
        let txs = if hit {
            pool.recent(&self.addresses, previous).await.ok()
        } else {
            Some(Vec::new())
        };
        let balances: Vec<Balance> = changed
            .iter()
            .filter_map(|address| current.get(*address))
            .flatten()
            .cloned()
            .collect();
        // 客户端读不过来时不能卡住其他订阅。
        let update = self.update(height, previous, &balances, txs.as_deref());
        if self.out.try_send(update).is_ok() {
            let mut progress = self.progress.lock().expect("跟块状态被污染");
            progress.height = Some(height);
            for address in changed {
                if let Some(balances) = current.get(address) {
                    progress.balances.insert(address.clone(), balances.clone());
                }
            }
        }
    }
}

#[derive(Default)]
struct Shared {
    watchers: HashMap<u64, Arc<Watcher>>,
    next: u64,
    task: Option<AbortOnDrop>,
}

pub struct Follower {
    network: Network,
    shared: Mutex<Shared>,
    /// 全部被订阅的地址，长连接据此增加上游订阅。
    watched: watch::Sender<Vec<String>>,
}

/// 一个订阅的成员资格，丢弃时退出，最后一个退出时跟块停止。
pub struct Member {
    follower: Arc<Follower>,
    key: u64,
}

impl Drop for Member {
    fn drop(&mut self) {
        let mut shared = self.follower.lock();
        shared.watchers.remove(&self.key);
        self.follower.publish(&shared);
        if shared.watchers.is_empty() {
            shared.task = None;
        }
    }
}

impl Follower {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            shared: Mutex::default(),
            watched: watch::Sender::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().expect("跟块状态被污染")
    }

    fn publish(&self, shared: &Shared) {
        let addresses: HashSet<&String> = shared.watchers.values().flat_map(|w| &w.addresses).collect();
        self.watched.send_replace(addresses.into_iter().cloned().collect());
    }

    pub fn join(self: &Arc<Self>, state: &Arc<AppState>, watcher: Watcher) -> Member {
        let mut shared = self.lock();
        let key = shared.next;
        shared.next += 1;
        shared.watchers.insert(key, Arc::new(watcher));
        self.publish(&shared);
        if shared.task.is_none() {
            shared.task = Some(AbortOnDrop(tokio::spawn(follow(state.clone(), self.clone()))));
        }
        Member {
            follower: self.clone(),
            key,
        }
    }

    /// 有订阅还没看过这个高度。
    fn behind(&self, head: u64) -> bool {
        let shared = self.lock();
        shared.watchers.values().any(|watcher| {
            let progress = watcher.progress.lock().expect("跟块状态被污染");
            progress.height != Some(head)
        })
    }

    /// 查一轮并通知全部订阅。全部地址都查到才推进，否则下一轮重查。
    async fn round(&self, pool: &Pool, height: u64, touched: &HashSet<String>) {
        let watchers: Vec<Arc<Watcher>> = self.lock().watchers.values().cloned().collect();
        let addresses = self.watched.borrow().clone();
        let current = current_balances(pool, &addresses).await;
        if current.len() != addresses.len() {
            return;
        }
        let notices = watchers
            .iter()
            .map(|watcher| watcher.notify(pool, height, &current, touched));
        join_all(notices).await;
    }
}

/// 一轮要处理的信号，处理之前到达的会合并。
#[derive(Default)]
struct Pending {
    head: Option<u64>,
    touched: HashSet<String>,
    live: bool,
}

impl Pending {
    fn take(&mut self, signal: Signal) {
        match signal {
            Signal::Head(height) => {
                self.head = Some(height);
                self.live = true;
            }
            Signal::Touched(addresses) => self.touched.extend(addresses),
            Signal::Lost => self.live = false,
        }
    }
}

async fn follow(state: Arc<AppState>, follower: Arc<Follower>) {
    let network = follower.network;
    let (signals, mut incoming) = mpsc::channel(SIGNAL_QUEUE);
    let watching = state.clone();
    let addresses = follower.watched.subscribe();
    let _watch = AbortOnDrop(tokio::spawn(async move {
        if let Some(pool) = watching.networks.get(&network) {
            pool.watch(addresses, signals).await;
        }
    }));
    let Some(pool) = state.networks.get(&network) else {
        return;
    };
    let pause = floor(network.chain()).min(state.poll_interval);
    let mut next = Instant::now();
    let mut pending = Pending::default();
    loop {
        sleep_until(next).await;
        match timeout(state.poll_interval, incoming.recv()).await {
            Ok(Some(signal)) => pending.take(signal),
            Ok(None) => return,
            // 长连接还在时安静就是没有新块，不用查。
            Err(_) if pending.live => continue,
            Err(_) => pending.head = pool.head().await.ok(),
        }
        while let Ok(signal) = incoming.try_recv() {
            pending.take(signal);
        }
        next = Instant::now() + pause;
        let Some(head) = pending.head else { continue };
        if pending.touched.is_empty() && !follower.behind(head) {
            continue;
        }
        follower.round(pool, head, &pending.touched).await;
        pending.touched.clear();
    }
}

//! 跟块.每个网络一个任务,所有订阅共用,第一个订阅到来时启动,最后一个离开时停止.设计见 docs/03-system.md §4.1.1.
//!
//! 这里只处理 `watch` 发来的信号:只有高度时不访问上游,地址有动静才查这些地址的余额和交易.
//! 上游没有长连接时退回轮询:每个新高度查一次全部地址,靠余额变化发现交易.
//! 每条推送带上一轮的高度,客户端发现接不上就重新取快照.

use crate::upstream::{Balance, HistoryItem, Networks, Pool, Signal};
use bitone_core::Network;
use futures_util::future::join_all;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

/// 每批查询的地址数,与 HTTP 接口的单次上限一致.
const CHUNK: usize = 100;
const SIGNAL_QUEUE: usize = 64;

/// 只有高度变化时两轮之间的最短间隔.BSC 和 Solana 不到一秒一个块,没有必要每块都通知.
/// 地址有动静时不等它.
const ROUND_INTERVAL: Duration = Duration::from_secs(3);

/// 丢弃时中止任务.
pub struct AbortOnDrop(pub JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 分批查询当前余额,按地址分组.第一次失败就结束本轮,上游故障时不逐批等超时.
/// 没查到的地址不出现在结果里.
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
    /// 上一条推送没发出去.下一条的 `txs` 是空,客户端补取这批地址.
    lost: bool,
}

/// 一个订阅.基线要在回复订阅 ID 之前取好,客户端随后取的快照才不会漏掉变化.
/// 高度要在基线之前取,两者之间出块时下一轮一定会重查.
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
            progress: Mutex::new(Progress {
                height,
                balances,
                lost: false,
            }),
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

    /// 给这个订阅发一轮结果.长连接下只有被触发的地址才会再查,发不出去的变化不会自己回来,
    /// 所以记下来,下一条的 `txs` 是空,客户端自己补取.交易没取到时也一样,余额照常推送.
    async fn notify(
        &self,
        pool: &Pool,
        height: u64,
        current: &HashMap<String, Vec<Balance>>,
        touched: &HashSet<String>,
    ) {
        let (last, changed, lost) = {
            let progress = self.progress.lock().expect("跟块状态被污染");
            let changed: Vec<&String> = self
                .addresses
                .iter()
                .filter(|address| {
                    current
                        .get(*address)
                        .is_some_and(|balances| progress.balances.get(*address) != Some(balances))
                })
                .collect();
            (progress.height, changed, progress.lost)
        };
        let hit = !changed.is_empty() || self.addresses.iter().any(|address| touched.contains(address));
        if !hit && !lost && last == Some(height) {
            return;
        }
        let previous = last.unwrap_or(height);
        let txs = if lost {
            None
        } else if hit {
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
        // 客户端读不过来时不能卡住其他订阅.
        let update = self.update(height, previous, &balances, txs.as_deref());
        let sent = self.out.try_send(update).is_ok();
        let mut progress = self.progress.lock().expect("跟块状态被污染");
        progress.lost = !sent;
        if sent {
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
    /// 查不到推送时轮询高度的间隔.
    poll_interval: Duration,
    shared: Mutex<Shared>,
    /// 全部被订阅的地址,长连接据此增加上游订阅.
    watched: watch::Sender<Vec<String>>,
}

/// 一个订阅的成员资格,丢弃时退出,最后一个退出时跟块停止.
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
    pub fn new(network: Network, poll_interval: Duration) -> Self {
        Self {
            network,
            poll_interval,
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

    pub fn join(self: &Arc<Self>, networks: &Arc<Networks>, watcher: Watcher) -> Member {
        let mut shared = self.lock();
        let key = shared.next;
        shared.next += 1;
        shared.watchers.insert(key, Arc::new(watcher));
        self.publish(&shared);
        if shared.task.is_none() {
            shared.task = Some(AbortOnDrop(tokio::spawn(follow(networks.clone(), self.clone()))));
        }
        Member {
            follower: self.clone(),
            key,
        }
    }

    /// 有订阅还没看过这个高度.
    fn behind(&self, head: u64) -> bool {
        let shared = self.lock();
        shared.watchers.values().any(|watcher| {
            let progress = watcher.progress.lock().expect("跟块状态被污染");
            progress.height != Some(head)
        })
    }

    /// 查一轮并通知全部订阅.`query` 里的地址都查到才推进,否则返回假,这些地址下一轮重查.
    async fn round(&self, pool: &Pool, height: u64, query: &[String], touched: &HashSet<String>) -> bool {
        let watchers: Vec<Arc<Watcher>> = self.lock().watchers.values().cloned().collect();
        let current = current_balances(pool, query).await;
        if current.len() != query.len() {
            return false;
        }
        let notices = watchers
            .iter()
            .map(|watcher| watcher.notify(pool, height, &current, touched));
        join_all(notices).await;
        true
    }
}

/// 一轮要处理的信号,处理之前到达的会合并.
#[derive(Default)]
struct Pending {
    head: Option<u64>,
    touched: HashSet<String>,
    reconcile: bool,
    live: bool,
}

impl Pending {
    fn take(&mut self, signal: Signal) {
        match signal {
            Signal::Head(height) => {
                self.head = Some(height);
                self.live = true;
            }
            Signal::Block(height, addresses) => {
                self.head = Some(height);
                self.live = true;
                self.touched.extend(addresses);
            }
            Signal::Touched(addresses) => self.touched.extend(addresses),
            Signal::Reconcile => self.reconcile = true,
            Signal::Lost => self.live = false,
        }
    }

    /// 这一轮要查余额的地址.长连接正常时只查有动静的,查不到推送时查全部.
    fn query(&self, watched: &[String]) -> Vec<String> {
        let all = !self.live || self.reconcile;
        let wanted = |address: &&String| all || self.touched.contains(*address);
        watched.iter().filter(wanted).cloned().collect()
    }
}

async fn follow(networks: Arc<Networks>, follower: Arc<Follower>) {
    let network = follower.network;
    let poll_interval = follower.poll_interval;
    let (signals, mut incoming) = mpsc::channel(SIGNAL_QUEUE);
    let watching = networks.clone();
    let addresses = follower.watched.subscribe();
    let _watch = AbortOnDrop(tokio::spawn(async move {
        if let Some(pool) = watching.pool(network) {
            pool.watch(addresses, signals).await;
        }
    }));
    let Some(pool) = networks.pool(network) else {
        return;
    };
    let pause = ROUND_INTERVAL.min(poll_interval);
    let mut next = Instant::now();
    // 查询失败后隔一段再试,节点不稳时不能每个块都查一遍.
    let mut retry = Instant::now();
    let mut pending = Pending::default();
    loop {
        match timeout(poll_interval, incoming.recv()).await {
            Ok(Some(signal)) => pending.take(signal),
            Ok(None) => return,
            // 长连接还在时安静就是没有动静,不用查.
            Err(_) if pending.live => {}
            Err(_) => pending.head = pool.head().await.ok().or(pending.head),
        }
        while let Ok(signal) = incoming.try_recv() {
            pending.take(signal);
        }
        if Instant::now() < retry {
            continue;
        }
        let urgent = !pending.touched.is_empty() || pending.reconcile;
        if !urgent && Instant::now() < next {
            continue;
        }
        let Some(head) = pending.head else { continue };
        if !urgent && !follower.behind(head) {
            continue;
        }
        next = Instant::now() + pause;
        let query = pending.query(&follower.watched.borrow());
        if follower.round(pool, head, &query, &pending.touched).await {
            pending.touched.clear();
            pending.reconcile = false;
        } else {
            retry = Instant::now() + poll_interval;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::{Broadcast, Cursor, HistoryPage, Query, RawTx, Tx, Upstream};
    use futures_util::future::BoxFuture;
    use serde_json::Value;

    struct Quiet;

    impl Upstream for Quiet {
        fn balances<'a>(&'a self, _: &'a [String]) -> Query<'a, Vec<Balance>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn height(&self) -> Query<'_, u64> {
            Box::pin(async { Ok(1) })
        }
        fn fees(&self) -> Query<'_, Value> {
            Box::pin(async { Ok(Value::Null) })
        }
        fn txs<'a>(&'a self, _: &'a [String]) -> Query<'a, Vec<Tx>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn history<'a>(&'a self, _: &'a [String], _: &'a Cursor, _: Option<u64>) -> Query<'a, HistoryPage> {
            Box::pin(async { Ok(HistoryPage::default()) })
        }
        fn broadcast<'a>(&'a self, _: &'a RawTx) -> BoxFuture<'a, Broadcast> {
            Box::pin(async { Broadcast::Unknown })
        }
    }

    /// 队列满时推送发不出去.长连接下这些地址不会再被查询,只能让客户端自己补.
    #[tokio::test]
    async fn an_update_the_queue_refused_makes_the_next_one_ask_for_a_refill() {
        let pool = Pool::new(vec![Box::new(Quiet)], true, Duration::from_secs(1));
        let (out, mut sent) = mpsc::channel(1);
        out.try_send(String::new()).unwrap();
        let watcher = Watcher::new("s".to_string(), vec!["a".to_string()], out, Some(10), HashMap::new());
        let touched = HashSet::from(["a".to_string()]);
        watcher.notify(&pool, 11, &HashMap::new(), &touched).await;
        sent.recv().await;

        watcher.notify(&pool, 12, &HashMap::new(), &HashSet::new()).await;
        let update: Value = serde_json::from_str(&sent.recv().await.unwrap()).unwrap();
        assert_eq!(update["params"]["txs"], Value::Null);
        assert_eq!(update["params"]["previous"], 10);

        watcher.notify(&pool, 13, &HashMap::new(), &HashSet::new()).await;
        let update: Value = serde_json::from_str(&sent.recv().await.unwrap()).unwrap();
        assert_eq!(update["params"]["txs"], json!([]), "补取一次就够了");
    }
}

//! 一个网络的全部上游，失败时切换下一个。

use super::{Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Query, RawTx, Signal, Tx, Upstream};
use crate::cache::Cached;
use crate::error::ApiError;
use bitone_core::{Chain, Network};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, timeout};

/// 手续费随区块变化，几秒的误差没有影响。
const FEES_TTL: Duration = Duration::from_secs(15);
/// 区块相关的公共数据，比出块间隔略短。
const BLOCK_TTL: Duration = Duration::from_secs(3);
/// 全部上游的长连接都断开后，隔这么久再试。其间网关轮询高度。
const WATCH_RETRY: Duration = Duration::from_secs(30);

pub struct Pool {
    upstreams: Vec<Box<dyn Upstream>>,
    has_history: bool,
    /// 从上次成功的上游开始，这样坏掉的上游不会让每次请求都白等一个超时。
    current: AtomicUsize,
    timeout: Duration,
    fees: Cached<Value>,
    height: Cached<u64>,
    /// 交易要引用的最近区块。每个网络只有一种，内容由链决定。
    pub anchor: Cached<Value>,
}

impl Pool {
    pub fn new(upstreams: Vec<Box<dyn Upstream>>, has_history: bool, timeout: Duration) -> Self {
        Self {
            upstreams,
            has_history,
            current: AtomicUsize::new(0),
            timeout,
            fees: Cached::new(FEES_TTL),
            height: Cached::new(BLOCK_TTL),
            anchor: Cached::new(BLOCK_TTL),
        }
    }

    async fn attempt<'a, T>(
        &'a self,
        query: impl Fn(&'a (dyn Upstream + 'static)) -> Query<'a, T>,
    ) -> Result<T, ApiError> {
        let start = self.current.load(Ordering::Relaxed);
        for offset in 0..self.upstreams.len() {
            let index = (start + offset) % self.upstreams.len();
            if let Ok(Ok(value)) = timeout(self.timeout, query(self.upstreams[index].as_ref())).await {
                self.current.store(index, Ordering::Relaxed);
                return Ok(value);
            }
        }
        Err(ApiError::UpstreamUnavailable)
    }

    /// 只属于一条链的查询。`adapter` 由这条链提供，这里不需要知道有哪些链。
    pub async fn special<'a, A: ?Sized + 'a, T>(
        &'a self,
        adapter: fn(&'a (dyn Upstream + 'static)) -> Option<&'a A>,
        query: impl Fn(&'a A) -> Query<'a, T>,
    ) -> Result<T, ApiError> {
        self.attempt(|upstream| match adapter(upstream) {
            Some(adapter) => query(adapter),
            None => Box::pin(async { Err(Failed) }),
        })
        .await
    }

    pub async fn balances(&self, addresses: &[String]) -> Result<Vec<Balance>, ApiError> {
        self.attempt(|upstream| upstream.balances(addresses)).await
    }

    pub async fn height(&self) -> Result<u64, ApiError> {
        self.height.get(self.attempt(|upstream| upstream.height())).await
    }

    /// 跟块用的高度，不走缓存。
    pub async fn head(&self) -> Result<u64, ApiError> {
        self.attempt(|upstream| upstream.height()).await
    }

    pub async fn fees(&self) -> Result<Value, ApiError> {
        self.fees.get(self.attempt(|upstream| upstream.fees())).await
    }

    pub async fn txs(&self, hashes: &[String]) -> Result<Vec<Tx>, ApiError> {
        self.attempt(|upstream| upstream.txs(hashes)).await
    }

    pub async fn history(
        &self,
        addresses: &[String],
        cursor: &Cursor,
        since: Option<u64>,
    ) -> Result<HistoryPage, ApiError> {
        self.attempt(|upstream| upstream.history(addresses, cursor, since))
            .await
    }

    pub async fn recent(&self, addresses: &[String], previous: u64) -> Result<Vec<HistoryItem>, ApiError> {
        self.attempt(|upstream| upstream.recent(addresses, previous)).await
    }

    /// 依次用每个上游保持长连接，断开就换下一个，直到跟块任务结束。
    pub async fn watch(&self, addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) {
        while !signals.is_closed() {
            for upstream in &self.upstreams {
                upstream.watch(addresses.clone(), signals.clone()).await;
                let _ = signals.send(Signal::Lost).await;
            }
            sleep(WATCH_RETRY).await;
        }
    }

    /// 查不到历史是配置决定的，不是临时故障。
    pub fn has_history(&self) -> bool {
        self.has_history
    }

    /// 广播是唯一会写链的接口，所以不走 `attempt`：只有确定请求没有发出去时才换上游。
    /// 超时和其他不明情况都就地返回，让网页端按结果不明处理，见 docs/03-system.md §7。
    pub async fn broadcast(&self, raw: &RawTx) -> Result<(), ApiError> {
        let start = self.current.load(Ordering::Relaxed);
        for offset in 0..self.upstreams.len() {
            let index = (start + offset) % self.upstreams.len();
            match timeout(self.timeout, self.upstreams[index].broadcast(raw)).await {
                Ok(Broadcast::Accepted) => {
                    self.current.store(index, Ordering::Relaxed);
                    return Ok(());
                }
                Ok(Broadcast::Rejected(message)) => return Err(ApiError::broadcast_rejected(&message)),
                Ok(Broadcast::Unreachable) => {}
                Ok(Broadcast::Unknown) | Err(_) => return Err(ApiError::UpstreamUnavailable),
            }
        }
        Err(ApiError::UpstreamUnavailable)
    }
}

/// 全部配置了上游的网络。
pub struct Networks(HashMap<Network, Pool>);

impl Networks {
    pub fn new(pools: HashMap<Network, Pool>) -> Self {
        Self(pools)
    }

    pub fn pool(&self, network: Network) -> Option<&Pool> {
        self.0.get(&network)
    }

    /// 未知网络和没有配置上游的网络对外没有区别。
    pub fn get(&self, id: &str) -> Result<(Network, &Pool), ApiError> {
        let network = Network::from_id(id).ok_or(ApiError::UnknownNetwork)?;
        Ok((network, self.pool(network).ok_or(ApiError::UnknownNetwork)?))
    }

    /// 只属于一条链的接口用这个，其他链的网络报 `WrongChain`。
    pub fn on_chain(&self, id: &str, chain: Chain) -> Result<(Network, &Pool), ApiError> {
        let (network, pool) = self.get(id)?;
        if network.chain() == chain {
            Ok((network, pool))
        } else {
            Err(ApiError::WrongChain)
        }
    }
}

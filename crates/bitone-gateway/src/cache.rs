//! 公共链上数据的短缓存,只用于手续费,区块和价格,见 docs/03-system.md §4.1.
//! 和地址,交易相关的查询不能放进来.

use crate::error::ApiError;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// 失败也缓存一小段时间.排队的请求不用逐个等上游超时.
const FAILURE_TTL: Duration = Duration::from_secs(1);

/// 单个值的缓存.到期时只有一个请求回源,其余的等它的结果.
pub struct Cached<T> {
    ttl: Duration,
    slot: Mutex<Option<(Instant, Result<T, ApiError>)>>,
}

impl<T: Clone> Cached<T> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slot: Mutex::new(None),
        }
    }

    pub async fn get(&self, load: impl Future<Output = Result<T, ApiError>>) -> Result<T, ApiError> {
        let mut slot = self.slot.lock().await;
        if let Some((stored, value)) = &*slot {
            let ttl = if value.is_ok() {
                self.ttl
            } else {
                FAILURE_TTL.min(self.ttl)
            };
            if stored.elapsed() < ttl {
                return value.clone();
            }
        }
        let value = load.await;
        *slot = Some((Instant::now(), value.clone()));
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn load(cache: &Cached<u32>, calls: &AtomicUsize, value: Result<u32, ApiError>) -> Result<u32, ApiError> {
        cache
            .get(async {
                calls.fetch_add(1, Ordering::SeqCst);
                value
            })
            .await
    }

    #[tokio::test]
    async fn serves_the_stored_value_until_it_expires() {
        let (cache, calls) = (Cached::new(Duration::from_millis(30)), AtomicUsize::new(0));
        assert_eq!(load(&cache, &calls, Ok(1)).await, Ok(1));
        assert_eq!(load(&cache, &calls, Ok(2)).await, Ok(1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(load(&cache, &calls, Ok(2)).await, Ok(2));
    }

    #[tokio::test]
    async fn failures_expire_sooner_than_values() {
        let (cache, calls) = (Cached::new(Duration::from_secs(60)), AtomicUsize::new(0));
        let failure = Err(ApiError::UpstreamUnavailable);
        assert_eq!(load(&cache, &calls, failure.clone()).await, failure);
        assert_eq!(load(&cache, &calls, Ok(1)).await, failure);
        tokio::time::sleep(FAILURE_TTL + Duration::from_millis(10)).await;
        assert_eq!(load(&cache, &calls, Ok(1)).await, Ok(1));
    }

    #[tokio::test]
    async fn concurrent_misses_load_once() {
        let (cache, calls) = (Cached::new(Duration::from_secs(60)), AtomicUsize::new(0));
        let results = futures_util::future::join_all((0..5).map(|_| load(&cache, &calls, Ok(1)))).await;
        assert!(results.iter().all(|r| *r == Ok(1)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

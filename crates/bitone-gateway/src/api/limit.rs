//! 按 IP 的令牌桶和 WebSocket 连接数上限.只用来控制成本.

use axum::http::HeaderMap;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SWEEP_PERIOD: Duration = Duration::from_secs(60);

pub struct RateLimiter {
    burst: f64,
    per_second: f64,
    inner: Mutex<Inner>,
}

struct Inner {
    buckets: HashMap<IpAddr, Bucket>,
    swept: Instant,
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    fn refilled(&self, now: Instant, per_second: f64) -> f64 {
        self.tokens + now.duration_since(self.updated).as_secs_f64() * per_second
    }
}

/// IPv6 用户通常独占一个 /64,按单个地址算等于没有限流.
fn bucket_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (!0 << 64))),
        v4 => v4,
    }
}

impl RateLimiter {
    pub fn new(burst: f64, per_second: f64) -> Self {
        let inner = Inner {
            buckets: HashMap::new(),
            swept: Instant::now(),
        };
        Self {
            burst,
            per_second,
            inner: Mutex::new(inner),
        }
    }

    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        let (burst, per_second) = (self.burst, self.per_second);
        let mut inner = self.inner.lock().unwrap();
        if now.duration_since(inner.swept) >= SWEEP_PERIOD {
            // 回满的桶等于不存在,及时丢掉,IP 不在内存里多留,表也不会无限增长.
            inner
                .buckets
                .retain(|_, bucket| bucket.refilled(now, per_second) < burst);
            inner.swept = now;
        }
        let new_bucket = Bucket {
            tokens: burst,
            updated: now,
        };
        let bucket = inner.buckets.entry(bucket_key(ip)).or_insert(new_bucket);
        bucket.tokens = bucket.refilled(now, per_second).min(burst);
        bucket.updated = now;
        let allowed = bucket.tokens >= 1.0;
        if allowed {
            bucket.tokens -= 1.0;
        }
        allowed
    }
}

/// 限流看到的客户端 IP.对端不是受信任代理时,转发头可以伪造,一律不读.
pub fn client_ip(peer: IpAddr, trusted: &HashSet<IpAddr>, headers: &HeaderMap) -> IpAddr {
    let mut client = peer.to_canonical();
    if !trusted.contains(&client) {
        return client;
    }
    // 从右往左走,右边离网关最近.第一个不受信任的地址就是客户端,再往左都可能是伪造的.
    for value in headers.get_all("x-forwarded-for").iter().rev() {
        let Ok(value) = value.to_str() else { return client };
        for hop in value.rsplit(',') {
            let Ok(ip) = hop.trim().parse::<IpAddr>() else {
                return client;
            };
            client = ip.to_canonical();
            if !trusted.contains(&client) {
                return client;
            }
        }
    }
    client
}

/// 每个 IP 同时打开的 WebSocket 连接数.计数归零就删除,IP 不在内存里多留.
pub struct ConnectionLimit {
    max: usize,
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

/// 连接占用的名额,丢弃时归还.
pub struct ConnectionSlot {
    key: IpAddr,
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl ConnectionLimit {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            counts: Arc::default(),
        }
    }

    pub fn acquire(&self, ip: IpAddr) -> Option<ConnectionSlot> {
        let key = bucket_key(ip);
        let mut counts = self.counts.lock().unwrap();
        let count = counts.entry(key).or_insert(0);
        if *count >= self.max {
            return None;
        }
        *count += 1;
        Some(ConnectionSlot {
            key,
            counts: self.counts.clone(),
        })
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap();
        if let Some(count) = counts.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn burst_then_refill() {
        let limiter = RateLimiter::new(2.0, 1.0);
        let now = Instant::now();
        assert!(limiter.allow_at(ip("1.1.1.1"), now));
        assert!(limiter.allow_at(ip("1.1.1.1"), now));
        assert!(!limiter.allow_at(ip("1.1.1.1"), now));
        assert!(limiter.allow_at(ip("2.2.2.2"), now));
        assert!(limiter.allow_at(ip("1.1.1.1"), now + Duration::from_secs(1)));
    }

    #[test]
    fn ipv6_shares_a_bucket_per_64() {
        let limiter = RateLimiter::new(1.0, 1.0);
        let now = Instant::now();
        assert!(limiter.allow_at(ip("2001:db8:1:2::1"), now));
        assert!(!limiter.allow_at(ip("2001:db8:1:2::2"), now));
        assert!(limiter.allow_at(ip("2001:db8:1:3::1"), now));
    }

    #[test]
    fn mapped_ipv4_is_the_same_client() {
        let limiter = RateLimiter::new(1.0, 1.0);
        let now = Instant::now();
        assert!(limiter.allow_at(ip("1.2.3.4"), now));
        assert!(!limiter.allow_at(ip("::ffff:1.2.3.4"), now));
    }

    #[test]
    fn full_buckets_are_swept() {
        let limiter = RateLimiter::new(1.0, 1.0);
        let now = Instant::now();
        limiter.allow_at(ip("1.1.1.1"), now);
        limiter.allow_at(ip("2.2.2.2"), now + SWEEP_PERIOD);
        let buckets = &limiter.inner.lock().unwrap().buckets;
        assert_eq!(buckets.keys().copied().collect::<Vec<_>>(), vec![ip("2.2.2.2")]);
    }

    fn forwarded(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn untrusted_peers_cannot_choose_their_ip() {
        let trusted = HashSet::from([ip("10.0.0.1")]);
        assert_eq!(
            client_ip(ip("9.9.9.9"), &trusted, &forwarded(&["1.1.1.1"])),
            ip("9.9.9.9")
        );
        assert_eq!(
            client_ip(ip("9.9.9.9"), &HashSet::new(), &forwarded(&["1.1.1.1"])),
            ip("9.9.9.9")
        );
    }

    #[test]
    fn takes_the_rightmost_untrusted_hop() {
        let trusted = HashSet::from([ip("10.0.0.1"), ip("10.0.0.2")]);
        let peer = ip("10.0.0.1");
        assert_eq!(client_ip(peer, &trusted, &forwarded(&["1.1.1.1"])), ip("1.1.1.1"));
        // 最左边的是客户端自己写的,不能采信.
        assert_eq!(
            client_ip(peer, &trusted, &forwarded(&["6.6.6.6, 1.1.1.1, 10.0.0.2"])),
            ip("1.1.1.1")
        );
        // 多个头按出现顺序拼接.
        assert_eq!(
            client_ip(peer, &trusted, &forwarded(&["6.6.6.6", "1.1.1.1"])),
            ip("1.1.1.1")
        );
        assert_eq!(
            client_ip(peer, &trusted, &forwarded(&["::ffff:1.1.1.1"])),
            ip("1.1.1.1")
        );
    }

    #[test]
    fn falls_back_to_the_last_trusted_hop() {
        let trusted = HashSet::from([ip("10.0.0.1"), ip("10.0.0.2")]);
        let peer = ip("10.0.0.1");
        assert_eq!(client_ip(peer, &trusted, &HeaderMap::new()), peer);
        assert_eq!(client_ip(peer, &trusted, &forwarded(&["不是地址"])), peer);
        assert_eq!(
            client_ip(peer, &trusted, &forwarded(&["1.1.1.1, x, 10.0.0.2"])),
            ip("10.0.0.2")
        );
        assert_eq!(client_ip(peer, &trusted, &forwarded(&["10.0.0.2"])), ip("10.0.0.2"));
    }

    #[test]
    fn connection_slots_are_returned_on_drop() {
        let limit = ConnectionLimit::new(2);
        let first = limit.acquire(ip("1.1.1.1")).unwrap();
        let second = limit.acquire(ip("1.1.1.1")).unwrap();
        assert!(limit.acquire(ip("1.1.1.1")).is_none());
        assert!(limit.acquire(ip("2.2.2.2")).is_some());
        drop(first);
        let third = limit.acquire(ip("1.1.1.1")).unwrap();
        drop((second, third));
        assert!(limit.counts.lock().unwrap().is_empty(), "IP 不能留在内存里");
    }

    #[test]
    fn connection_limit_shares_a_64_and_mapped_addresses() {
        let limit = ConnectionLimit::new(1);
        let _slot = limit.acquire(ip("2001:db8:1:2::1")).unwrap();
        assert!(limit.acquire(ip("2001:db8:1:2::2")).is_none());
        let _mapped = limit.acquire(ip("1.2.3.4")).unwrap();
        assert!(limit.acquire(ip("::ffff:1.2.3.4")).is_none());
    }
}

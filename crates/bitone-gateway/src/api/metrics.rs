//! 聚合指标.日志只有方法,状态码,次数和耗时,没有路径,IP,地址,也没有单次请求的时间点.

use axum::http::Method;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct Stat {
    count: u32,
    total: Duration,
    max: Duration,
}

#[derive(Default)]
pub struct Metrics(Mutex<BTreeMap<(&'static str, u16), Stat>>);

/// 方法可以由客户端任意构造,归并后键的数量才有上限.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

impl Metrics {
    pub fn record(&self, method: &Method, status: u16, elapsed: Duration) {
        let mut stats = self.0.lock().unwrap();
        let stat = stats.entry((method_label(method), status)).or_default();
        stat.count += 1;
        stat.total += elapsed;
        stat.max = stat.max.max(elapsed);
    }

    fn take_lines(&self) -> Vec<String> {
        let stats = std::mem::take(&mut *self.0.lock().unwrap());
        stats
            .into_iter()
            .map(|((method, status), stat)| {
                let avg = (stat.total / stat.count).as_millis();
                let max = stat.max.as_millis();
                format!(
                    "method={method} status={status} count={} avg_ms={avg} max_ms={max}",
                    stat.count
                )
            })
            .collect()
    }

    /// 打印上一周期的汇总并清零.
    pub fn flush(&self) {
        for line in self.take_lines() {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_and_resets() {
        let metrics = Metrics::default();
        metrics.record(&Method::POST, 200, Duration::from_millis(10));
        metrics.record(&Method::POST, 200, Duration::from_millis(30));
        metrics.record(&Method::from_bytes(b"PROPFIND").unwrap(), 405, Duration::ZERO);
        assert_eq!(
            metrics.take_lines(),
            vec![
                "method=OTHER status=405 count=1 avg_ms=0 max_ms=0",
                "method=POST status=200 count=2 avg_ms=20 max_ms=30"
            ],
        );
        assert!(metrics.take_lines().is_empty());
    }
}

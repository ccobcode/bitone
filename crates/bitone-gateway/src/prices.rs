//! 美元价格。上游是 CoinGecko 的 simple/price 格式，映射到 `网络:合约` 的键。
//! 价格只是展示，不参与任何判断。配置时只接受主网资产，见 config.rs。

use crate::cache::Cached;
use crate::config::PricesConfig;
use crate::error::ApiError;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::timeout;

const CACHE_TTL: Duration = Duration::from_secs(60);
const CURRENCY: &str = "usd";

pub type PriceMap = BTreeMap<String, String>;

pub struct Prices {
    client: Client,
    url: String,
    headers: HeaderMap,
    /// 价格键和资产 ID。
    ids: Vec<(String, String)>,
    timeout: Duration,
    cache: Cached<PriceMap>,
}

/// 上游没有的资产不出现在结果里，出现但不是正常数字的算上游异常。
fn parse(reply: &Value, ids: &[(String, String)]) -> Option<PriceMap> {
    let reply = reply.as_object()?;
    let mut prices = PriceMap::new();
    for (key, id) in ids {
        let Some(entry) = reply.get(id) else { continue };
        let price = entry
            .get(CURRENCY)?
            .as_f64()
            .filter(|price| price.is_finite() && *price >= 0.0)?;
        prices.insert(key.clone(), price.to_string());
    }
    Some(prices)
}

impl Prices {
    pub fn new(config: &PricesConfig, client: &Client, timeout: Duration) -> Self {
        let mut headers = HeaderMap::new();
        if let (Some(name), Some(key)) = (&config.api_key_header, &config.api_key)
            && let (Ok(name), Ok(mut key)) = (HeaderName::try_from(name.as_str()), HeaderValue::from_str(key))
        {
            key.set_sensitive(true);
            headers.insert(name, key);
        }
        Self {
            client: client.clone(),
            url: config.url.clone(),
            headers,
            ids: config.ids.iter().map(|(key, id)| (key.clone(), id.clone())).collect(),
            timeout,
            cache: Cached::new(CACHE_TTL),
        }
    }

    async fn fetch(&self) -> Option<PriceMap> {
        let mut ids: Vec<&str> = self.ids.iter().map(|(_, id)| id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        let query = [("ids", ids.join(",")), ("vs_currencies", CURRENCY.to_string())];
        let request = self.client.get(&self.url).headers(self.headers.clone()).query(&query);
        let response = request.send().await.ok()?.error_for_status().ok()?;
        parse(&response.json().await.ok()?, &self.ids)
    }

    pub async fn get(&self) -> Result<PriceMap, ApiError> {
        let fetch = timeout(self.timeout, self.fetch());
        self.cache
            .get(async { fetch.await.ok().flatten().ok_or(ApiError::UpstreamUnavailable) })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids() -> Vec<(String, String)> {
        vec![
            ("bitcoin:native".to_string(), "bitcoin".to_string()),
            ("ethereum:0xUSDT".to_string(), "tether".to_string()),
            ("bsc:0xUSDT".to_string(), "tether".to_string()),
            ("tron:native".to_string(), "tron".to_string()),
        ]
    }

    #[test]
    fn maps_ids_to_network_keys_and_skips_missing_assets() {
        let reply = json!({"bitcoin": {"usd": 77693}, "tether": {"usd": 0.999164}});
        let expected = PriceMap::from([
            ("bitcoin:native".to_string(), "77693".to_string()),
            ("ethereum:0xUSDT".to_string(), "0.999164".to_string()),
            ("bsc:0xUSDT".to_string(), "0.999164".to_string()),
        ]);
        assert_eq!(parse(&reply, &ids()), Some(expected));
    }

    #[test]
    fn small_prices_stay_in_plain_decimal() {
        let reply = json!({"bitcoin": {"usd": 0.00000012}});
        assert_eq!(parse(&reply, &ids()).unwrap()["bitcoin:native"], "0.00000012");
    }

    #[test]
    fn malformed_prices_are_an_upstream_failure() {
        for bad in [
            json!([]),
            json!({"bitcoin": {}}),
            json!({"bitcoin": {"usd": "1"}}),
            json!({"bitcoin": {"usd": -1}}),
        ] {
            assert_eq!(parse(&bad, &ids()), None, "{bad}");
        }
    }
}

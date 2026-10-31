//! TRON。余额和历史用 TronGrid 的 v1 接口，其余是全节点的 HTTP 接口。

mod adapter;
mod watch;

use super::request::{Body, Reply, Shared, addresses_from, checked_address, is_hex, parse, present};
use super::{ChainSpec, NetworkConfig, estimated, is_hex64};
use crate::error::ApiError;
use crate::upstream::{Http, Networks, Pool, Upstream, adapter};
use adapter::{Estimate, Tron};
use axum::extract::Path;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::routing::{get, post};
use axum::{Json, Router};
use bitcoin::base58;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::DisplayHex;
use bitone_core::{Chain, Network};
use futures_util::future::BoxFuture;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// TronGrid 的 API key 请求头。
const API_KEY: &str = "tron-pro-api-key";

pub struct Spec;

#[derive(Deserialize)]
struct EstimateRequest {
    from: String,
    to: String,
    #[serde(default)]
    contract: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

fn estimate_call(network: Network, body: Body) -> Result<Estimate, ApiError> {
    let request: EstimateRequest = parse(body)?;
    let contract = present(request.contract)
        .map(|c| checked_address(network, c))
        .transpose()?;
    let data = present(request.data);
    // 合约调用要有调用数据，普通转账不看这个字段。
    if contract.is_some() && !data.as_deref().is_some_and(is_hex) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(Estimate {
        from: checked_address(network, request.from)?,
        to: checked_address(network, request.to)?,
        contract,
        data,
    })
}

async fn resources(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Tron)?;
    let addresses = addresses_from(network, body)?;
    let resources = pool.special(adapter::<Tron>, |tron| tron.resources(&addresses)).await?;
    Ok(Json(json!({"resources": resources})))
}

async fn ref_block(networks: Shared, Path(network): Path<String>) -> Reply {
    let (_, pool) = networks.on_chain(&network, Chain::Tron)?;
    let query = pool.special(adapter::<Tron>, |tron| tron.ref_block());
    Ok(Json(pool.anchor.get(query).await?))
}

/// protobuf 的变长整数，返回值和剩余字节。
fn varint(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let mut value = 0usize;
    for (index, byte) in bytes.iter().enumerate().take(5) {
        value |= usize::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, &bytes[index + 1..]));
        }
    }
    None
}

impl ChainSpec for Spec {
    fn is_valid_address(&self, _network: Network, address: &str) -> bool {
        base58::decode_check(address).is_ok_and(|p| p.len() == 21 && p[0] == 0x41)
    }

    fn is_valid_hash(&self, hash: &str) -> bool {
        is_hex64(hash)
    }

    /// 交易 ID 是 raw_data 的 SHA-256。raw_data 是 Transaction 消息的第 1 个字段。
    fn raw_hash(&self, bytes: &[u8]) -> Option<String> {
        let (&tag, rest) = bytes.split_first()?;
        if tag != 0x0a {
            return None;
        }
        let (length, rest) = varint(rest)?;
        let raw_data = rest.get(..length)?;
        Some(sha256::Hash::hash(raw_data).to_byte_array().to_lower_hex_string())
    }

    fn upstream(&self, _network: Network, config: &NetworkConfig, url: &str, client: &Client) -> Box<dyn Upstream> {
        let mut headers = HeaderMap::new();
        if let Some(mut key) = config
            .tron_api_key
            .as_deref()
            .and_then(|key| HeaderValue::from_str(key).ok())
        {
            key.set_sensitive(true);
            headers.insert(HeaderName::from_static(API_KEY), key);
        }
        Box::new(Tron {
            http: Http::new(client, url, headers),
            tokens: config.tokens.clone(),
        })
    }

    fn options(&self) -> &'static [&'static str] {
        &["tokens", "tron_api_key"]
    }

    fn check(&self, network: Network, config: &NetworkConfig) -> Result<(), String> {
        match &config.tron_api_key {
            Some(key) if HeaderValue::from_str(key).is_err() => Err(format!("{} 的 tron_api_key 无效", network.id())),
            _ => Ok(()),
        }
    }

    fn routes(&self) -> Router<Arc<Networks>> {
        Router::new()
            .route("/{network}/resources", post(resources))
            .route("/{network}/ref-block", get(ref_block))
    }

    fn estimate<'a>(&self, network: Network, pool: &'a Pool, body: Body) -> BoxFuture<'a, Reply> {
        Box::pin(async move {
            let call = estimate_call(network, body)?;
            estimated(pool.special(adapter::<Tron>, |tron| tron.estimate(&call)).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::chains::{is_valid, raw_tx};
    use bitone_core::Network;

    const TRON: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const LEGACY: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";

    #[test]
    fn tron_rules() {
        assert!(is_valid(Network::Tron, TRON));
        assert!(is_valid(Network::TronNile, TRON));
        // 校验和错误。
        assert!(!is_valid(Network::Tron, "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6u"));
        // Bitcoin 的 base58check 地址，前缀不是 0x41。
        assert!(!is_valid(Network::Tron, LEGACY));
    }

    #[test]
    fn tron_hashes_raw_data_only() {
        // 主网的一笔真实转账，交易 ID 取自 TronGrid。
        let raw_data = "0a02507e2208db1b9b7d595deec340b095e5e091345a65080112610a2d747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e73666572436f6e747261637412300a154192689ecfbfcdffa3444961571f2b822d3967d0aa12154188403ac26730e33164eeac403291b48f300b782f180270b0f3cfcf9134";
        let length = raw_data.len() / 2;
        let signed = format!(
            "0a{:02x}{:02x}{raw_data}1241{}",
            (length & 0x7f) | 0x80,
            length >> 7,
            "ab".repeat(65)
        );
        let raw = raw_tx(Network::Tron, &signed).unwrap();
        assert_eq!(
            raw.hash,
            "22a58240f0260c4df379060e67dde86e14324d15a64738b7b5fb69c8e30dff50"
        );
        for bad in ["", "0a", "0a7f00", "1241ab"] {
            assert!(raw_tx(Network::Tron, bad).is_err(), "{bad}");
        }
    }
}

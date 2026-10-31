//! EVM。普通 JSON-RPC 节点。交易历史节点查不到，要配置索引服务。

mod adapter;
mod etherscan;
mod hypersync;
mod watch;

use super::request::{Body, Reply, Shared, addresses_from, checked_address, is_hex, parse, present};
use super::{ChainSpec, NetworkConfig, check_url, estimated, is_hex64};
use crate::error::ApiError;
use crate::upstream::{Http, Networks, Pool, Upstream, adapter, decimal_to_hex};
use adapter::{Evm, Indexer};
use axum::extract::Path;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderValue};
use axum::routing::post;
use axum::{Json, Router};
use bitcoin::hex::DisplayHex;
use bitone_core::{Chain, Network};
use futures_util::future::BoxFuture;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};
use std::sync::Arc;

pub struct Spec;

#[derive(Deserialize)]
struct EstimateRequest {
    from: String,
    to: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

fn estimate_call(network: Network, body: Body) -> Result<Value, ApiError> {
    let request: EstimateRequest = parse(body)?;
    let value = present(request.value).unwrap_or_else(|| "0".to_string());
    let data = present(request.data).unwrap_or_else(|| "0x".to_string());
    if !data.strip_prefix("0x").is_some_and(is_hex) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(json!({
        "from": checked_address(network, request.from)?,
        "to": checked_address(network, request.to)?,
        "value": decimal_to_hex(&value).ok_or(ApiError::InvalidRequest)?,
        "data": data,
    }))
}

async fn nonce(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Evm)?;
    let addresses = addresses_from(network, body)?;
    let nonces = pool.special(adapter::<Evm>, |evm| evm.nonces(&addresses)).await?;
    Ok(Json(json!({"nonces": nonces})))
}

fn bearer(token: &str) -> Option<HeaderMap> {
    let mut value = HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
    value.set_sensitive(true);
    Some(HeaderMap::from_iter([(AUTHORIZATION, value)]))
}

impl ChainSpec for Spec {
    fn is_valid_address(&self, _network: Network, address: &str) -> bool {
        // ponytail: 不校验 EIP-55 大小写，只读查询写错也花不了钱。转账校验在网页端和冷钱包。
        address
            .strip_prefix("0x")
            .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    fn is_valid_hash(&self, hash: &str) -> bool {
        hash.strip_prefix("0x").is_some_and(is_hex64)
    }

    /// 交易类型字节在 0x01 到 0x7f 之间，旧式交易是 RLP 列表，首字节至少 0xc0。
    fn raw_hash(&self, bytes: &[u8]) -> Option<String> {
        match bytes.first()? {
            0x01..=0x7f | 0xc0.. => Some(format!("0x{}", Keccak256::digest(bytes).to_lower_hex_string())),
            _ => None,
        }
    }

    fn upstream(&self, _network: Network, config: &NetworkConfig, url: &str, client: &Client) -> Box<dyn Upstream> {
        let indexer = config
            .indexer
            .as_deref()
            .map(|url| match config.indexer_token.as_deref().and_then(bearer) {
                Some(auth) => Indexer::HyperSync(Http::new(client, url, auth)),
                None => Indexer::Etherscan(Http::new(client, url, HeaderMap::new())),
            });
        Box::new(Evm {
            http: Http::new(client, url, HeaderMap::new()),
            tokens: config.tokens.clone(),
            indexer,
        })
    }

    fn options(&self) -> &'static [&'static str] {
        &["tokens", "indexer", "indexer_token"]
    }

    fn check(&self, network: Network, config: &NetworkConfig) -> Result<(), String> {
        let id = network.id();
        if let Some(url) = &config.indexer {
            check_url(url, id)?;
        }
        if let Some(token) = &config.indexer_token {
            if config.indexer.is_none() {
                return Err(format!("{id} 配了 indexer_token，没有 indexer"));
            }
            if HeaderValue::from_str(token).is_err() {
                return Err(format!("{id} 的 indexer_token 无效"));
            }
        }
        Ok(())
    }

    /// 没有索引服务时查不到历史。
    fn has_history(&self, config: &NetworkConfig) -> bool {
        config.indexer.is_some()
    }

    fn routes(&self) -> Router<Arc<Networks>> {
        Router::new().route("/{network}/nonce", post(nonce))
    }

    fn estimate<'a>(&self, network: Network, pool: &'a Pool, body: Body) -> BoxFuture<'a, Reply> {
        Box::pin(async move {
            let call = estimate_call(network, body)?;
            estimated(pool.special(adapter::<Evm>, |evm| evm.estimate(&call)).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::chains::{is_valid, raw_tx};
    use bitone_core::Network;

    const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";

    #[test]
    fn evm_rules() {
        assert!(is_valid(Network::Ethereum, EVM));
        assert!(is_valid(Network::Sepolia, &EVM.to_lowercase()));
        assert!(!is_valid(Network::Ethereum, &EVM[2..]));
        assert!(!is_valid(Network::Ethereum, &EVM[..41]));
        assert!(!is_valid(
            Network::Ethereum,
            "0xZ858EfFD232B4033E47d90003D41EC34EcaEda94"
        ));
    }

    #[test]
    fn evm_hashes_the_whole_encoding() {
        // EIP-155 文档里的旧式交易示例。
        let raw = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";
        let expected = "0x33469b22e9f636356c4160a87eb19df52b7412e8eac32a4a55ffe88ea8350788";
        assert_eq!(raw_tx(Network::Ethereum, raw).unwrap().hash, expected);
        assert_eq!(raw_tx(Network::Sepolia, &format!("0x{raw}")).unwrap().hash, expected);
        assert!(raw_tx(Network::Ethereum, "80").is_err());
        assert!(raw_tx(Network::Ethereum, "").is_err());
    }
}

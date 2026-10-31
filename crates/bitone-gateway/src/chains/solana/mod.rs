//! Solana。标准 JSON-RPC 节点。

mod adapter;
mod watch;

use super::request::{Body, Reply, Shared, checked_address, parse, present};
use super::{ChainSpec, NetworkConfig};
use crate::upstream::{Http, Networks, Pool, Upstream, adapter};
use adapter::Solana;
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use bitcoin::base58;
use bitone_core::{Chain, Network};
use futures_util::future::BoxFuture;
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;

/// 一个签名的长度。
const SIGNATURE: usize = 64;
/// 签名的 base58 文本不超过 88 个字符。
const MAX_SIGNATURE_LENGTH: usize = 88;

pub struct Spec;

#[derive(Deserialize)]
struct AddressRequest {
    address: String,
}

#[derive(Deserialize)]
struct EstimateRequest {
    recipient: String,
    #[serde(default)]
    mint: Option<String>,
}

async fn nonce_account(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Solana)?;
    let request: AddressRequest = parse(body)?;
    let address = checked_address(network, request.address)?;
    let account = pool.special(adapter::<Solana>, |solana| solana.nonce_account(&address));
    Ok(Json(account.await?))
}

async fn blockhash(networks: Shared, Path(network): Path<String>) -> Reply {
    let (_, pool) = networks.on_chain(&network, Chain::Solana)?;
    let query = pool.special(adapter::<Solana>, |solana| solana.blockhash());
    Ok(Json(pool.anchor.get(query).await?))
}

impl ChainSpec for Spec {
    fn is_valid_address(&self, _network: Network, address: &str) -> bool {
        base58::decode(address).is_ok_and(|key| key.len() == 32)
    }

    fn is_valid_hash(&self, hash: &str) -> bool {
        hash.len() <= MAX_SIGNATURE_LENGTH && base58::decode(hash).is_ok_and(|signature| signature.len() == 64)
    }

    /// 交易哈希是第一个签名。开头是紧凑编码的签名数量，这里只接受 127 个以内。
    fn raw_hash(&self, bytes: &[u8]) -> Option<String> {
        let (&count, rest) = bytes.split_first()?;
        if count == 0 || count > 0x7f || rest.len() <= usize::from(count) * SIGNATURE {
            return None;
        }
        Some(base58::encode(&rest[..SIGNATURE]))
    }

    fn upstream(&self, _network: Network, config: &NetworkConfig, url: &str, client: &Client) -> Box<dyn Upstream> {
        Box::new(Solana {
            http: Http::new(client, url, HeaderMap::new()),
            tokens: config.tokens.clone(),
        })
    }

    fn options(&self) -> &'static [&'static str] {
        &["tokens"]
    }

    fn routes(&self) -> Router<Arc<Networks>> {
        Router::new()
            .route("/{network}/nonce-account", post(nonce_account))
            .route("/{network}/blockhash", get(blockhash))
    }

    fn estimate<'a>(&self, network: Network, pool: &'a Pool, body: Body) -> BoxFuture<'a, Reply> {
        Box::pin(async move {
            let request: EstimateRequest = parse(body)?;
            let recipient = checked_address(network, request.recipient)?;
            let mint = present(request.mint)
                .map(|mint| checked_address(network, mint))
                .transpose()?;
            let query = pool.special(adapter::<Solana>, |solana| solana.estimate(&recipient, mint.as_deref()));
            Ok(Json(query.await?))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SIGNATURE;
    use crate::chains::{is_valid, raw_tx};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use bitcoin::hex::DisplayHex;
    use bitone_core::Network;

    const SOLANA: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    #[test]
    fn solana_rules() {
        assert!(is_valid(Network::Solana, SOLANA));
        assert!(is_valid(Network::Solana, "11111111111111111111111111111111"));
        assert!(!is_valid(Network::Solana, "abc"));
        assert!(!is_valid(Network::Solana, "0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl"));
    }

    #[test]
    fn solana_takes_the_first_signature() {
        let mut bytes = vec![1u8];
        bytes.extend([7u8; SIGNATURE]);
        bytes.extend([0x80, 0x01, 0x02]);
        let raw = raw_tx(Network::Solana, &bytes.to_lower_hex_string()).unwrap();
        assert_eq!(raw.hash, bitcoin::base58::encode(&[7u8; SIGNATURE]));
        assert_eq!(raw.base64(), STANDARD.encode(&bytes));
        // 没有签名，或者长度不够。
        assert!(raw_tx(Network::Solana, "00").is_err());
        assert!(raw_tx(Network::Solana, &[1u8; SIGNATURE].to_lower_hex_string()).is_err());
    }
}

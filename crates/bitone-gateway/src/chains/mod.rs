//! 每条链一个目录,目录之间互不引用.链的差异都收在目录里:地址和哈希的格式,已签名交易的哈希,
//! 配置,上游适配器,跟块,以及只属于这条链的接口.网关其余部分只通过 `ChainSpec` 接触它们.
//!
//! 加一条链:在 `bitone_core::Chain` 里加一项,在这里加一个目录,再填上 `spec` 的分支.

mod bitcoin;
mod config;
mod evm;
pub mod request;
mod solana;
mod tron;

use crate::error::ApiError;
use crate::upstream::{Networks, Pool, RawTx, Upstream};
use ::bitcoin::hex::FromHex;
use axum::Router;
use axum::response::Json;
use bitone_core::{Chain, Network};
pub use config::{NetworkConfig, check_url};
use futures_util::future::BoxFuture;
use request::{Body, Reply};
use reqwest::Client;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// 最长的合法地址是 bech32 的 90 个字符.base58 解码是平方复杂度,先挡住超长输入.
const MAX_LENGTH: usize = 90;

/// 一条链接入网关要提供的全部内容.
pub trait ChainSpec: Sync {
    /// 地址格式.校验通过后才会拼进上游 URL,这也是防注入的边界.
    fn is_valid_address(&self, network: Network, address: &str) -> bool;
    /// 交易哈希格式.哈希也会拼进上游 URL,同样是防注入的边界.
    fn is_valid_hash(&self, hash: &str) -> bool;
    /// 已签名交易的哈希,格式不对时为空.节点报"已知"时响应里没有哈希,幂等返回要用这里算出的值,
    /// 格式错误的数据也不用发给上游.
    fn raw_hash(&self, bytes: &[u8]) -> Option<String>;
    fn upstream(&self, network: Network, config: &NetworkConfig, url: &str, client: &Client) -> Box<dyn Upstream>;

    /// 配置里这条链用得到的选项,其余选项写了就报错,写错网络的配置不能悄悄失效.
    fn options(&self) -> &'static [&'static str] {
        &[]
    }
    fn check_upstream(&self, network: Network, url: &str) -> Result<(), String> {
        check_url(url, network.id())
    }
    /// 这条链自己的配置选项.
    fn check(&self, _network: Network, _config: &NetworkConfig) -> Result<(), String> {
        Ok(())
    }
    fn has_history(&self, _config: &NetworkConfig) -> bool {
        true
    }
    /// 只属于这条链的接口.
    fn routes(&self) -> Router<Arc<Networks>> {
        Router::new()
    }
    /// 估算费用.请求和结果都因链而异,字段的校验也在这里,都在调用上游之前.
    fn estimate<'a>(&self, _network: Network, _pool: &'a Pool, _body: Body) -> BoxFuture<'a, Reply> {
        Box::pin(async { Err(ApiError::WrongChain) })
    }
}

pub fn spec(chain: Chain) -> &'static dyn ChainSpec {
    match chain {
        Chain::Bitcoin => &bitcoin::Spec,
        Chain::Evm => &evm::Spec,
        Chain::Tron => &tron::Spec,
        Chain::Solana => &solana::Spec,
    }
}

/// 全部链专属的接口.
pub fn routes() -> Router<Arc<Networks>> {
    let chains: HashSet<Chain> = Network::ALL.iter().map(|network| network.chain()).collect();
    chains
        .into_iter()
        .fold(Router::new(), |routes, chain| routes.merge(spec(chain).routes()))
}

pub fn networks(configs: &HashMap<Network, NetworkConfig>, client: &Client, timeout: Duration) -> Networks {
    let pool = |network: Network, config: &NetworkConfig| {
        let spec = spec(network.chain());
        let upstreams = config.upstreams.iter();
        let upstreams = upstreams.map(|url| spec.upstream(network, config, url, client));
        Pool::new(upstreams.collect(), spec.has_history(config), timeout)
    };
    Networks::new(
        configs
            .iter()
            .map(|(network, config)| (*network, pool(*network, config)))
            .collect(),
    )
}

pub fn is_valid(network: Network, address: &str) -> bool {
    address.len() <= MAX_LENGTH && spec(network.chain()).is_valid_address(network, address)
}

pub fn is_valid_hash(network: Network, hash: &str) -> bool {
    spec(network.chain()).is_valid_hash(hash)
}

pub fn raw_tx(network: Network, text: &str) -> Result<RawTx, ApiError> {
    let text = text.strip_prefix("0x").unwrap_or(text);
    let bytes = Vec::<u8>::from_hex(text).map_err(|_| ApiError::InvalidRawTx)?;
    let hash = spec(network.chain()).raw_hash(&bytes);
    Ok(RawTx {
        hash: hash.ok_or(ApiError::InvalidRawTx)?,
        bytes,
    })
}

/// 64 位十六进制,几条链的交易哈希都是这个形状.
fn is_hex64(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 节点拒绝估算时带节点原文,换上游也不会有不同结果.
fn estimated(reply: Result<Value, String>) -> Reply {
    Ok(Json(reply.map_err(|text| ApiError::call_rejected(&text))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
    const TRON: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";

    #[test]
    fn chains_do_not_mix() {
        assert!(!is_valid(Network::Ethereum, TRON));
        assert!(!is_valid(Network::Tron, EVM));
        assert!(!is_valid(Network::Solana, EVM));
        assert!(!is_valid(Network::Bitcoin, EVM));
    }

    #[test]
    fn overlong_input_is_rejected_before_decoding() {
        assert!(!is_valid(Network::Solana, &"1".repeat(60_000)));
    }

    #[test]
    fn hashes_follow_the_chain_format() {
        let hex = "ab".repeat(32);
        let signature = ::bitcoin::base58::encode(&[7u8; 64]);
        assert!(is_valid_hash(Network::Bitcoin, &hex));
        assert!(is_valid_hash(Network::Tron, &hex));
        assert!(is_valid_hash(Network::Ethereum, &format!("0x{hex}")));
        assert!(is_valid_hash(Network::Solana, &signature));

        assert!(!is_valid_hash(Network::Ethereum, &hex));
        assert!(!is_valid_hash(Network::Bitcoin, &format!("0x{hex}")));
        assert!(!is_valid_hash(Network::Bitcoin, "../../address/x"));
        assert!(!is_valid_hash(Network::Bitcoin, &"zz".repeat(32)));
        assert!(!is_valid_hash(Network::Solana, &hex));
        assert!(!is_valid_hash(Network::Solana, &"1".repeat(60_000)));
    }

    #[test]
    fn rejects_non_hex() {
        for network in [Network::Bitcoin, Network::Ethereum, Network::Tron, Network::Solana] {
            assert_eq!(raw_tx(network, "zz").err(), Some(ApiError::InvalidRawTx));
        }
    }
}

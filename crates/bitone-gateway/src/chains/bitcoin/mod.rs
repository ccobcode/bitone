//! Bitcoin.上游写成 `tcp://` 或 `ssl://` 是 Electrum 协议,写成 `http(s)://` 是 Esplora 接口.

mod electrum;
mod esplora;
mod shape;

use super::request::{Body, MAX_HASHES, Reply, Shared, addresses_from, checked_hashes, parse};
use super::{ChainSpec, NetworkConfig, check_url, is_hex64};
use crate::upstream::{Http, Networks, Query, Upstream, adapter};
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use bitcoin::address::NetworkUnchecked;
use bitone_core::{Chain, Mode, Network};
use electrum::{Electrum, Transport, transport};
use esplora::Esplora;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Spec;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Utxo {
    pub address: String,
    pub txid: String,
    pub vout: u32,
    pub amount: String,
    /// 0 表示还在内存池.
    pub height: u64,
}

/// 只有 Bitcoin 有的查询,两种上游都要实现.
trait Queries: Send + Sync {
    /// 这批地址里有过交易的.
    fn used<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<String>>;
    fn utxos<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Utxo>>;
    /// 上游没有的交易不出现在结果里.
    fn raw_txs<'a>(&'a self, txids: &'a [String]) -> Query<'a, BTreeMap<String, String>>;
}

fn queries<'a>(upstream: &'a (dyn Upstream + 'static)) -> Option<&'a (dyn Queries + 'a)> {
    match adapter::<Electrum>(upstream) {
        Some(electrum) => Some(electrum),
        None => adapter::<Esplora>(upstream).map(|esplora| esplora as &dyn Queries),
    }
}

#[derive(Deserialize)]
struct TxidsRequest {
    txids: Vec<String>,
}

async fn used(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Bitcoin)?;
    let addresses = addresses_from(network, body)?;
    let used = pool.special(queries, |bitcoin| bitcoin.used(&addresses)).await?;
    Ok(Json(json!({"used": used})))
}

async fn utxos(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Bitcoin)?;
    let addresses = addresses_from(network, body)?;
    let utxos = pool.special(queries, |bitcoin| bitcoin.utxos(&addresses)).await?;
    Ok(Json(json!({"utxos": utxos})))
}

async fn raw_txs(networks: Shared, Path(network): Path<String>, body: Body) -> Reply {
    let (network, pool) = networks.on_chain(&network, Chain::Bitcoin)?;
    let request: TxidsRequest = parse(body)?;
    let txids = checked_hashes(network, request.txids, MAX_HASHES)?;
    let txs = pool.special(queries, |bitcoin| bitcoin.raw_txs(&txids)).await?;
    Ok(Json(json!({"txs": txs})))
}

fn has_host_and_port(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| url.host_str().is_some() && url.port().is_some())
}

impl ChainSpec for Spec {
    fn is_valid_address(&self, network: Network, address: &str) -> bool {
        // Testnet4,Signet,Testnet3 的地址格式相同,都按测试网校验.
        let expected = match network {
            Network::Bitcoin => bitcoin::Network::Bitcoin,
            _ => bitcoin::Network::Testnet,
        };
        address
            .parse::<bitcoin::Address<NetworkUnchecked>>()
            .is_ok_and(|parsed| parsed.is_valid_for_network(expected))
    }

    fn is_valid_hash(&self, hash: &str) -> bool {
        is_hex64(hash)
    }

    fn raw_hash(&self, bytes: &[u8]) -> Option<String> {
        let tx: bitcoin::Transaction = bitcoin::consensus::encode::deserialize(bytes).ok()?;
        Some(tx.compute_txid().to_string())
    }

    fn upstream(&self, network: Network, _config: &NetworkConfig, url: &str, client: &Client) -> Box<dyn Upstream> {
        match Electrum::new(url, network) {
            Some(electrum) => Box::new(electrum),
            None => {
                let http = Http::new(client, url, HeaderMap::new());
                Box::new(Esplora { http, network })
            }
        }
    }

    fn check_upstream(&self, network: Network, url: &str) -> Result<(), String> {
        let id = network.id();
        match transport(url) {
            Some(Transport::TlsUnverified) if network.mode() == Mode::Mainnet => {
                Err(format!("{id} 是主网,上游必须验证证书"))
            }
            Some(_) if !has_host_and_port(url) => Err(format!("{id} 的 Electrum 上游要写主机和端口")),
            Some(_) => Ok(()),
            None => check_url(url, id),
        }
    }

    fn routes(&self) -> Router<Arc<Networks>> {
        Router::new()
            .route("/{network}/used", post(used))
            .route("/{network}/utxos", post(utxos))
            .route("/{network}/raw-txs", post(raw_txs))
    }
}

#[cfg(test)]
mod tests {
    use crate::chains::{is_valid, raw_tx};
    use bitone_core::Network;

    const SEGWIT: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
    const TAPROOT: &str = "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr";
    const LEGACY: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";
    const TESTNET: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
    const GENESIS: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";

    #[test]
    fn bitcoin_networks_do_not_mix() {
        for address in [SEGWIT, TAPROOT, LEGACY] {
            assert!(is_valid(Network::Bitcoin, address));
            assert!(!is_valid(Network::BtcTestnet4, address));
        }
        assert!(!is_valid(Network::Bitcoin, TESTNET));
        for network in [Network::BtcTestnet4, Network::BtcSignet, Network::BtcTestnet3] {
            assert!(is_valid(network, TESTNET));
        }
        assert!(!is_valid(
            Network::Bitcoin,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
        ));
    }

    #[test]
    fn bitcoin_uses_the_display_txid() {
        let raw = raw_tx(Network::Bitcoin, GENESIS).unwrap();
        assert_eq!(
            raw.hash,
            "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        );
        assert_eq!(raw.hex(), GENESIS);
        // 尾部多余的字节不是合法交易.
        assert!(raw_tx(Network::Bitcoin, &format!("{GENESIS}00")).is_err());
    }
}

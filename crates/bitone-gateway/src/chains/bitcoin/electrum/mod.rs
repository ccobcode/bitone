//! Bitcoin 的 Electrum 协议上游。
//!
//! Esplora 的 HTTP 接口要逐个地址查询，公共节点按 IP 限流，一个钱包几十个地址很快用完额度。
//! Electrum 在一个连接上批量查询，主流轻钱包都用它。公共服务器和自建的 electrs、Fulcrum
//! 是同一个协议，换上游只改配置。查询结果转成 Esplora 的形状，解析和归纳规则只有一份。

use super::shape::{
    FAST_TARGET, FINAL_CONFIRMATIONS, MEDIUM_TARGET, SLOW_TARGET, fee_rates, history_item, is_already_known, parse_tx,
};
use super::{Queries, Utxo};
use crate::upstream::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, NATIVE, Query, RawTx, Signal, Tx, Upstream, balance,
    rejected,
};
use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::{Address, OutPoint, Script, Transaction, TxOut, Txid};
use bitone_core::{Mode, Network};
use futures_util::future::BoxFuture;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use serde_json::{Map, Value, json};
use session::{Call, RpcError, Session, Stream, tls_config};
pub use session::{Transport, transport};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::sync::{self, mpsc};
use tokio_rustls::TlsConnector;

mod session;
mod watch;

const PROTOCOL: &str = "1.4";
/// 历史每页的交易数。每笔还要取它花费的前序交易，太多会碰到上游超时。
const PAGE: usize = 25;
/// `blockchain.estimatefee` 的单位是 BTC/kB，网页端用 sat/vB。
const SAT_PER_VB: f64 = 100_000.0;
/// ElectrumX 和 Fulcrum 拒绝交易时都用这个错误码，其他错误码是服务自身的问题。
const REJECTED: i64 = 1;
/// electrs 系的错误码不区分拒绝和节点故障，要看它转述的节点错误码，这三个是交易本身的问题。
const NODE_REJECTIONS: [&str; 3] = ["-25", "-26", "-27"];

fn is_rejection(error: &RpcError) -> bool {
    error.code == REJECTED
        || (error.message.contains("RPC error") && NODE_REJECTIONS.iter().any(|code| error.message.contains(code)))
}

fn not_found(error: &RpcError) -> bool {
    let message = error.message.to_lowercase();
    message.contains("no such") || message.contains("not found")
}

/// 协议用脚本的 SHA-256 倒序作为地址的索引。
fn script_hash(script: &Script) -> String {
    let mut hash = sha256::Hash::hash(script.as_bytes()).to_byte_array();
    hash.reverse();
    hash.to_lower_hex_string()
}

fn address_hash(address: &str) -> Result<String, Failed> {
    let address = Address::from_str(address).map_err(|_| Failed)?.assume_checked();
    Ok(script_hash(&address.script_pubkey()))
}

/// 高度是 0 和 -1 都表示还在内存池。
fn history_row(row: &Value) -> Result<(Txid, Option<u64>), Failed> {
    let txid = row.get("tx_hash").and_then(Value::as_str).ok_or(Failed)?;
    let height = row.get("height").and_then(Value::as_i64).ok_or(Failed)?;
    let txid = Txid::from_str(txid).map_err(|_| Failed)?;
    Ok((txid, u64::try_from(height).ok().filter(|height| *height > 0)))
}

fn by_address(method: &'static str, addresses: &[String]) -> Result<Vec<Call>, Failed> {
    addresses
        .iter()
        .map(|address| Ok((method, json!([address_hash(address)?]))))
        .collect()
}

fn output_of(prevs: &HashMap<Txid, Transaction>, outpoint: OutPoint) -> Result<&TxOut, Failed> {
    prevs
        .get(&outpoint.txid)
        .and_then(|tx| tx.output.get(outpoint.vout as usize))
        .ok_or(Failed)
}

fn fee_of(tx: &Transaction, prevs: &HashMap<Txid, Transaction>) -> Result<u64, Failed> {
    if tx.is_coinbase() {
        return Ok(0);
    }
    let spent = tx.input.iter().try_fold(0u64, |sum, input| {
        let value = output_of(prevs, input.previous_output)?.value.to_sat();
        sum.checked_add(value).ok_or(Failed)
    })?;
    let paid = tx
        .output
        .iter()
        .try_fold(0u64, |sum, output| sum.checked_add(output.value.to_sat()).ok_or(Failed))?;
    spent.checked_sub(paid).ok_or(Failed)
}

pub struct Electrum {
    host: String,
    port: u16,
    network: Network,
    tls: Option<Arc<ClientConfig>>,
}

impl Electrum {
    pub fn new(url: &str, network: Network) -> Option<Self> {
        let transport = transport(url)?;
        let parsed = reqwest::Url::parse(url).ok()?;
        Some(Self {
            host: parsed.host_str()?.to_string(),
            port: parsed.port()?,
            network,
            tls: tls_config(transport),
        })
    }

    fn params(&self) -> bitcoin::Network {
        match self.network {
            Network::Bitcoin => bitcoin::Network::Bitcoin,
            _ => bitcoin::Network::Testnet,
        }
    }

    // ponytail: 每次查询开一个连接。公共服务器开始限制连接数时，改成每个上游一条长连接。
    async fn open(&self) -> Result<Session, Failed> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|_| Failed)?;
        let stream: Box<dyn Stream> = match &self.tls {
            None => Box::new(tcp),
            Some(config) => {
                let name = ServerName::try_from(self.host.clone()).map_err(|_| Failed)?;
                let connector = TlsConnector::from(config.clone());
                Box::new(connector.connect(name, tcp).await.map_err(|_| Failed)?)
            }
        };
        let mut session = Session {
            stream: BufReader::new(stream),
            next: 1,
            line: Vec::new(),
            pushes: Vec::new(),
        };
        session.one("server.version", json!(["bitone", PROTOCOL])).await?;
        Ok(session)
    }

    fn address_of(&self, output: &TxOut) -> Option<String> {
        Address::from_script(&output.script_pubkey, self.params())
            .ok()
            .map(|address| address.to_string())
    }

    /// 转成 Esplora 的形状，归纳规则只有一份。
    fn esplora_shape(
        &self,
        tx: &Transaction,
        height: Option<u64>,
        time: Option<u32>,
        prevs: &HashMap<Txid, Transaction>,
    ) -> Result<Value, Failed> {
        let output = |out: &TxOut| json!({"scriptpubkey_address": self.address_of(out), "value": out.value.to_sat()});
        let inputs = tx
            .input
            .iter()
            .map(|input| {
                // coinbase 没有前序交易。
                if input.previous_output.is_null() {
                    return Ok(json!({"prevout": null}));
                }
                Ok(json!({"prevout": output(output_of(prevs, input.previous_output)?)}))
            })
            .collect::<Result<Vec<_>, Failed>>()?;
        Ok(json!({
            "txid": tx.compute_txid().to_string(),
            "status": {"confirmed": height.is_some(), "block_height": height, "block_time": time},
            "vin": inputs,
            "vout": tx.output.iter().map(output).collect::<Vec<_>>(),
        }))
    }

    async fn page(&self, addresses: &[String], cursor: &Cursor, since: Option<u64>) -> Result<HistoryPage, Failed> {
        let mut session = self.open().await?;
        let tip = session.tip().await?;
        let checkpoint = (tip + 1).saturating_sub(FINAL_CONFIRMATIONS);
        if cursor.is_empty() && since == Some(checkpoint) {
            return Ok(HistoryPage {
                checkpoint,
                unchanged: true,
                ..Default::default()
            });
        }

        // 同一笔交易可能出现在多个地址的历史里。
        let mut heights: BTreeMap<Txid, Option<u64>> = BTreeMap::new();
        for reply in session
            .call(&by_address("blockchain.scripthash.get_history", addresses)?)
            .await?
        {
            for row in reply.map_err(|_| Failed)?.as_array().ok_or(Failed)? {
                let (txid, height) = history_row(row)?;
                heights.insert(txid, height);
            }
        }
        let mut rows: Vec<(Txid, Option<u64>)> = heights
            .into_iter()
            .filter(|(_, height)| match (height, since) {
                (Some(height), Some(since)) => *height > since,
                _ => true,
            })
            .collect();
        // 内存池的排最前，其余新的在前。
        // ponytail: 用偏移翻页，翻页途中内存池交易消失会漏一条。真遇到时改成按高度和 txid 定位。
        rows.sort_by_key(|(_, height)| std::cmp::Reverse(height.unwrap_or(u64::MAX)));

        let offset: usize = cursor
            .get("0")
            .map_or(Ok(0), |token| token.parse().map_err(|_| Failed))?;
        let end = rows.len().min(offset + PAGE);
        let page = rows.get(offset..end).unwrap_or_default();
        let txids: BTreeSet<Txid> = page.iter().map(|(txid, _)| *txid).collect();
        let txs = session.transactions(&txids).await?;
        let prevs = session.previous(&txs.values().collect::<Vec<_>>()).await?;
        let blocks: BTreeSet<u64> = page.iter().filter_map(|(_, height)| *height).collect();
        let times = session.block_times(&blocks).await?;

        let own: HashSet<&str> = addresses.iter().map(String::as_str).collect();
        let items = page
            .iter()
            .map(|(txid, height)| {
                let tx = txs.get(txid).ok_or(Failed)?;
                let time = height.and_then(|height| times.get(&height).copied());
                history_item(&self.esplora_shape(tx, *height, time, &prevs)?, &own, tip)
            })
            .collect::<Result<Vec<_>, Failed>>()?;
        let mut next = Cursor::new();
        if end < rows.len() {
            next.insert("0".to_string(), end.to_string());
        }
        Ok(HistoryPage {
            items,
            cursor: next,
            checkpoint,
            unchanged: false,
        })
    }

    async fn statuses(&self, hashes: &[String]) -> Result<Vec<Tx>, Failed> {
        let txids = hashes
            .iter()
            .map(|hash| Txid::from_str(hash).map_err(|_| Failed))
            .collect::<Result<Vec<_>, _>>()?;
        let mut session = self.open().await?;
        let tip = session.tip().await?;
        let txs = session.transactions(&txids.iter().copied().collect()).await?;
        let prevs = session.previous(&txs.values().collect::<Vec<_>>()).await?;
        let heights = session.heights(&txs, &prevs).await?;
        hashes
            .iter()
            .zip(&txids)
            .map(|(hash, txid)| {
                let Some(tx) = txs.get(txid) else {
                    return Ok(Tx::unknown(hash));
                };
                let Some(block) = *heights.get(txid).ok_or(Failed)? else {
                    return Ok(Tx::pending(hash));
                };
                let shape = json!({"status": {"confirmed": true, "block_height": block}, "fee": fee_of(tx, &prevs)?});
                parse_tx(hash, &shape, tip)
            })
            .collect()
    }

    async fn balances_of(&self, addresses: &[String]) -> Result<Vec<Balance>, Failed> {
        let calls = by_address("blockchain.scripthash.get_balance", addresses)?;
        let replies = self.open().await?.call(&calls).await?;
        addresses
            .iter()
            .zip(replies)
            .map(|(address, reply)| {
                let reply = reply.map_err(|_| Failed)?;
                let confirmed = reply.get("confirmed").and_then(Value::as_u64).ok_or(Failed)?;
                Ok(balance(address, NATIVE, confirmed.to_string()))
            })
            .collect()
    }

    async fn utxos_of(&self, addresses: &[String]) -> Result<Vec<Utxo>, Failed> {
        let calls = by_address("blockchain.scripthash.listunspent", addresses)?;
        let replies = self.open().await?.call(&calls).await?;
        let mut utxos = Vec::new();
        for (address, reply) in addresses.iter().zip(replies) {
            for row in reply.map_err(|_| Failed)?.as_array().ok_or(Failed)? {
                utxos.push(Utxo {
                    address: address.clone(),
                    txid: row.get("tx_hash").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                    vout: row
                        .get("tx_pos")
                        .and_then(Value::as_u64)
                        .ok_or(Failed)?
                        .try_into()
                        .map_err(|_| Failed)?,
                    amount: row.get("value").and_then(Value::as_u64).ok_or(Failed)?.to_string(),
                    height: row.get("height").and_then(Value::as_u64).unwrap_or(0),
                });
            }
        }
        Ok(utxos)
    }

    async fn fees_now(&self) -> Result<Value, Failed> {
        let targets = [FAST_TARGET, MEDIUM_TARGET, SLOW_TARGET];
        let calls: Vec<Call> = targets
            .iter()
            .map(|target| ("blockchain.estimatefee", json!([target])))
            .collect();
        let replies = self.open().await?.call(&calls).await?;
        let mut rates = Map::new();
        for (target, reply) in targets.iter().zip(replies) {
            // 节点没有足够的数据时返回 -1 或者报错，测试网经常这样。
            let Ok(rate) = reply else {
                continue;
            };
            let rate = rate.as_f64().ok_or(Failed)?;
            if rate >= 0.0 {
                rates.insert(target.to_string(), json!(rate * SAT_PER_VB));
            }
        }
        let mode: Mode = self.network.mode();
        fee_rates(&Value::Object(rates), mode)
    }

    async fn raw(&self, txids: &[String]) -> Result<BTreeMap<String, String>, Failed> {
        let calls: Vec<Call> = txids
            .iter()
            .map(|txid| ("blockchain.transaction.get", json!([txid, false])))
            .collect();
        let mut found = BTreeMap::new();
        for (txid, reply) in txids.iter().zip(self.open().await?.call(&calls).await?) {
            match reply {
                Ok(hex) => {
                    let hex = hex.as_str().ok_or(Failed)?;
                    Vec::<u8>::from_hex(hex).map_err(|_| Failed)?;
                    found.insert(txid.clone(), hex.to_string());
                }
                Err(error) if not_found(&error) => {}
                Err(_) => return Err(Failed),
            }
        }
        Ok(found)
    }

    async fn submit(&self, raw: &RawTx) -> Broadcast {
        let Ok(mut session) = self.open().await else {
            return Broadcast::Unreachable;
        };
        let replies = session
            .call(&[("blockchain.transaction.broadcast", json!([raw.hex()]))])
            .await;
        match replies.map(|mut replies| replies.pop()) {
            Ok(Some(Ok(_))) => Broadcast::Accepted,
            Ok(Some(Err(error))) if is_already_known(&error.message) => Broadcast::Accepted,
            Ok(Some(Err(error))) if is_rejection(&error) => rejected(self, raw, error.message).await,
            // 请求可能已经到达节点，不能换上游重发，也不能当成拒绝。
            _ => Broadcast::Unknown,
        }
    }
}

impl Upstream for Electrum {
    fn watch(&self, addresses: sync::watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.follow(addresses, signals).await;
        })
    }

    fn recent<'a>(&'a self, addresses: &'a [String], previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(async move {
            // 从不可回滚的位置之前开始，这样不会命中「高度没变」的捷径，内存池的交易也在里面。
            let since = previous.saturating_sub(FINAL_CONFIRMATIONS);
            let cursor = Cursor::new();
            Ok(self.page(addresses, &cursor, Some(since)).await?.items)
        })
    }

    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>> {
        Box::pin(self.balances_of(addresses))
    }

    fn height(&self) -> Query<'_, u64> {
        Box::pin(async { self.open().await?.tip().await })
    }

    fn fees(&self) -> Query<'_, Value> {
        Box::pin(self.fees_now())
    }

    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>> {
        Box::pin(self.statuses(hashes))
    }

    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(self.page(addresses, cursor, since))
    }

    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast> {
        Box::pin(self.submit(raw))
    }
}

impl Queries for Electrum {
    fn used<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<String>> {
        Box::pin(async move {
            let calls = by_address("blockchain.scripthash.get_history", addresses)?;
            let mut used = Vec::new();
            for (address, reply) in addresses.iter().zip(self.open().await?.call(&calls).await?) {
                if !reply.map_err(|_| Failed)?.as_array().ok_or(Failed)?.is_empty() {
                    used.push(address.clone());
                }
            }
            Ok(used)
        })
    }

    fn utxos<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Utxo>> {
        Box::pin(self.utxos_of(addresses))
    }

    fn raw_txs<'a>(&'a self, txids: &'a [String]) -> Query<'a, BTreeMap<String, String>> {
        Box::pin(self.raw(txids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_hash_matches_the_protocol_example() {
        assert_eq!(
            address_hash("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa").unwrap(),
            "8b01df4e368ea28f8dc0423bcf7a4923e3a12d307c875e47a0cfbf90b5c39161"
        );
    }

    #[test]
    fn only_the_three_schemes_are_electrum() {
        assert_eq!(transport("tcp://a:1"), Some(Transport::Plain));
        assert_eq!(transport("ssl://a:1"), Some(Transport::Tls));
        assert_eq!(transport("ssl-insecure://a:1"), Some(Transport::TlsUnverified));
        assert_eq!(transport("https://a"), None);
        assert_eq!(transport("a:1"), None);
    }
}

//! 上游的公共部分:适配器要实现的接口,统一的数据结构,连接池和传输.这里不出现具体的链,
//! 各条链的实现在 `chains` 里.
//!
//! 适配器只报告"这个上游这次不可用".reqwest 的错误文本带 URL,
//! URL 里可能有查询地址或 API key,所以错误内容一律丢弃,不传出也不记录.
//!
//! 余额都是已上链的状态.最终性由各条链判断:每条链都有一个纯函数
//! 把上游数据映射成 `Tx`,执行失败的交易永远是 `failed`,不能因为区块已经最终确定就报成 `finalized`.

mod cursor;
mod http;
mod number;
mod pool;
mod socket;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bitcoin::hex::DisplayHex;
pub use cursor::{Cursor, StreamPage, merge_pages, parse_cursor, stream_index, streams};
use futures_util::future::BoxFuture;
pub use http::{Http, run_all};
pub use number::{decimal, decimal_to_hex, hex_to_decimal, hex_u64, hex_u128};
pub use pool::{Networks, Pool};
use serde::Serialize;
use serde_json::Value;
pub use socket::{Socket, next_message, open_socket, send_call};
use std::any::Any;
use tokio::sync::{mpsc, watch};

pub const NATIVE: &str = "native";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Balance {
    pub address: String,
    pub asset: String,
    pub amount: String,
}

pub fn balance(address: &str, asset: &str, amount: String) -> Balance {
    Balance {
        address: address.to_string(),
        asset: asset.to_string(),
        amount,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TxStatus {
    Unknown,
    Pending,
    Included,
    Finalized,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tx {
    pub hash: String,
    pub status: TxStatus,
    pub confirmations: u64,
    pub stage: Option<&'static str>,
    pub block: Option<u64>,
    pub fee: Option<String>,
}

impl Tx {
    pub fn without_block(hash: &str, status: TxStatus) -> Self {
        Self {
            hash: hash.to_string(),
            status,
            confirmations: 0,
            stage: None,
            block: None,
            fee: None,
        }
    }

    pub fn unknown(hash: &str) -> Self {
        Self::without_block(hash, TxStatus::Unknown)
    }

    pub fn pending(hash: &str) -> Self {
        Self::without_block(hash, TxStatus::Pending)
    }

    /// 状态不会再变化,可以停止轮询.失败的交易在最终确定之前仍可能被重组.
    pub fn settled(&self) -> bool {
        match self.status {
            TxStatus::Finalized => true,
            TxStatus::Failed => self.stage == Some("finalized"),
            _ => false,
        }
    }

    /// 只有 Bitcoin 看确认数,其余链看阶段,确认数随区块增长不算状态变化.
    pub fn progressed_from(&self, last: &Tx) -> bool {
        let key = |tx: &Tx| {
            let confirmations = if tx.stage.is_none() { tx.confirmations } else { 0 };
            (tx.status, tx.stage, tx.block, tx.fee.clone(), confirmations)
        };
        key(self) != key(last)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HistoryItem {
    pub hash: String,
    /// 还没有上链时为空.
    pub time: Option<u64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub asset: String,
    pub amount: String,
    pub status: TxStatus,
    /// UTXO 链的输入输出.这里的 `from`,`to`,`amount` 只是按这批地址归纳的,钱包的地址可能分在
    /// 几批里,所以把事实一起给客户端,由它按整个钱包归纳.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows: Option<Flows>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Flows {
    pub inputs: Vec<String>,
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Output {
    /// 数据输出没有地址.
    pub address: Option<String>,
    pub amount: String,
}

#[derive(Debug, Default)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub cursor: Cursor,
    /// 已经不可回滚的位置.下次带着它查询,只返回这之后的记录.单位由链决定,客户端原样传回.
    pub checkpoint: u64,
    /// 上次之后没有任何变化,这一页没有查询地址.
    pub unchanged: bool,
}

/// 上游发给跟块任务的信号.
pub enum Signal {
    Head(u64),
    /// 解析完一批区块:新高度,以及这批区块里有动静的地址.两者要在同一轮处理,
    /// 否则客户端先收到一条没有内容的高度.
    Block(u64, Vec<String>),
    /// 这些地址有新动静,包括还在内存池的交易.
    Touched(Vec<String>),
    /// 查一次全部地址的余额,有变化才算地址有动静.推送覆盖不到的变化靠它补上.
    Reconcile,
    /// 长连接断了,改回轮询高度.
    Lost,
}

/// 一次广播的结果.
pub enum Broadcast {
    /// 节点接收了,或者这笔交易已经在内存池或链上.
    Accepted,
    /// 节点明确拒绝,带节点原文.
    Rejected(String),
    /// 连接失败,请求没有发出去,可以换下一个上游.
    Unreachable,
    /// 请求可能已经到达节点,结果不明.不能换上游重试,也不能当成功或失败.
    Unknown,
}

impl From<reqwest::Error> for Broadcast {
    fn from(error: reqwest::Error) -> Self {
        if error.is_connect() {
            Broadcast::Unreachable
        } else {
            Broadcast::Unknown
        }
    }
}

/// 已签名的原始交易,哈希由各条链算好.
pub struct RawTx {
    pub hash: String,
    pub bytes: Vec<u8>,
}

impl RawTx {
    pub fn hex(&self) -> String {
        self.bytes.to_lower_hex_string()
    }

    pub fn base64(&self) -> String {
        STANDARD.encode(&self.bytes)
    }
}

/// 节点拒绝后,这笔交易可能其实已经上链:有的链报 nonce 太低或输入已花费,有的报已过期.
/// 按哈希查到了就算成功,查询失败时仍然按节点的话报拒绝.
pub async fn rejected(upstream: &dyn Upstream, raw: &RawTx, message: String) -> Broadcast {
    match upstream.txs(std::slice::from_ref(&raw.hash)).await {
        Ok(txs) if txs.iter().any(|tx| tx.status != TxStatus::Unknown) => Broadcast::Accepted,
        _ => Broadcast::Rejected(message),
    }
}

#[derive(Debug)]
pub struct Failed;

impl From<reqwest::Error> for Failed {
    fn from(_: reqwest::Error) -> Self {
        Failed
    }
}

pub type Query<'a, T> = BoxFuture<'a, Result<T, Failed>>;

/// 每条链都有的能力.只属于一条链的查询不在这里,定义在这条链的目录里,见 `Pool::special`.
pub trait Upstream: Any + Send + Sync {
    /// 查一批地址.每个地址都返回原生币和清单里的全部代币,余额为 0 也返回,顺序固定.
    /// 上游数据有任何异常都算失败,不能把解析不了的内容当成 0.
    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>>;
    /// 最新区块高度.Solana 是 slot.
    fn height(&self) -> Query<'_, u64>;
    fn fees(&self) -> Query<'_, Value>;
    /// 一批交易的状态,顺序和请求相同.
    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>>;
    fn history<'a>(&'a self, addresses: &'a [String], cursor: &'a Cursor, since: Option<u64>)
    -> Query<'a, HistoryPage>;
    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast>;

    /// 跟块.发来新高度和有动静的地址,出错时返回,网关换下一个上游.
    /// 不支持时立刻返回,网关改为轮询高度,每个新高度查一次全部地址.
    fn watch(&self, _addresses: watch::Receiver<Vec<String>>, _signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    /// 跟块时取这批地址最近的交易,`previous` 是上一轮的高度.默认取最新一页.
    fn recent<'a>(&'a self, addresses: &'a [String], _previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(async move {
            let cursor = Cursor::new();
            Ok(self.history(addresses, &cursor, None).await?.items)
        })
    }
}

/// 取回这条链自己的适配器.
pub fn adapter<'a, A: Upstream>(upstream: &'a (dyn Upstream + 'static)) -> Option<&'a A> {
    (upstream as &dyn Any).downcast_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(status: TxStatus, stage: Option<&'static str>, confirmations: u64) -> Tx {
        Tx {
            hash: "h".into(),
            status,
            confirmations,
            stage,
            block: Some(1),
            fee: None,
        }
    }

    #[test]
    fn only_finalized_or_settled_failures_stop_polling() {
        assert!(tx(TxStatus::Finalized, None, 6).settled());
        assert!(tx(TxStatus::Failed, Some("finalized"), 1).settled());
        // 还能被重组.
        assert!(!tx(TxStatus::Failed, Some("latest"), 1).settled());
        assert!(!tx(TxStatus::Failed, Some("confirmed"), 1).settled());
        assert!(!tx(TxStatus::Included, Some("safe"), 1).settled());
        assert!(!tx(TxStatus::Unknown, None, 0).settled());
    }

    #[test]
    fn confirmations_only_count_as_progress_on_bitcoin() {
        let bitcoin = tx(TxStatus::Included, None, 1);
        assert!(tx(TxStatus::Included, None, 2).progressed_from(&bitcoin));
        let evm = tx(TxStatus::Included, Some("latest"), 1);
        assert!(!tx(TxStatus::Included, Some("latest"), 2).progressed_from(&evm));
        assert!(tx(TxStatus::Included, Some("safe"), 2).progressed_from(&evm));
    }
}

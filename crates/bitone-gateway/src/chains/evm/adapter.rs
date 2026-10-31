//! EVM 的上游适配器.

use crate::upstream::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, RawTx, Signal, Tx, TxStatus,
    Upstream, balance, hex_to_decimal, hex_u64, hex_u128, rejected, run_all,
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::iter::once;
use tokio::sync::{mpsc, watch};

/// ERC-20 的 balanceOf(address).
const BALANCE_OF: &str = "70a08231";
/// 单个 JSON-RPC 批次的调用数.服务商对批次大小都有上限.
pub(super) const BATCH_SIZE: usize = 100;
/// 查询一笔交易要两个调用.
const TX_BATCH: usize = BATCH_SIZE / 2;
/// 手续费按最近这么多个区块统计.
const FEE_BLOCKS: &str = "0x14";
/// 慢,中,快三档的优先费分位.
const FEE_PERCENTILES: [u64; 3] = [25, 50, 75];
/// 三档最高单价相对基础费的倍数,分子分母.基础费每个区块最多涨 12.5%,给足余量.
const BASE_FEE_MARGIN: [(u128, u128); 3] = [(5, 4), (3, 2), (2, 1)];
/// 节点报出这些内容时,交易已经在内存池里.各家客户端的写法不同,有的是错误名,
/// 所以先去掉空格和下划线再比较.
const ALREADY_KNOWN: [&str; 3] = ["alreadyknown", "knowntransaction", "alreadyimported"];
/// ERC-20 的 Transfer 事件.
pub(super) const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// 查交易历史的服务.节点查不到地址的历史.
pub enum Indexer {
    Etherscan(Http),
    HyperSync(Http),
}

pub struct Evm {
    pub http: Http,
    pub tokens: Vec<String>,
    pub indexer: Option<Indexer>,
}

pub(super) fn evm_call(address: &str, asset: &str) -> (&'static str, Value) {
    if asset == NATIVE {
        return ("eth_getBalance", json!([address, "latest"]));
    }
    let owner = address[2..].to_ascii_lowercase();
    let data = format!("0x{BALANCE_OF}{owner:0>64}");
    ("eth_call", json!([{"to": asset, "data": data}, "latest"]))
}

/// 链上的三个位置.
pub(super) struct Tips {
    pub(super) latest: u64,
    /// 有些节点不支持 `safe`,比如 Polygon.它只是中间阶段,缺了不影响完成标准.
    pub(super) safe: Option<u64>,
    pub(super) finalized: u64,
}

impl Tips {
    /// 先看 `finalized`:BSC 的 `safe` 可能比 `finalized` 还低.
    fn stage(&self, block: u64) -> &'static str {
        if block <= self.finalized {
            "finalized"
        } else if self.safe.is_some_and(|safe| block <= safe) {
            "safe"
        } else {
            "latest"
        }
    }
}

/// 回执映射成交易状态.回执里的 status 决定成败,和区块是否最终确定无关.
fn parse_tx(hash: &str, receipt: &Value, tx: &Value, tips: &Tips) -> Result<Tx, Failed> {
    if receipt.is_null() {
        return Ok(if tx.is_null() {
            Tx::unknown(hash)
        } else {
            Tx::pending(hash)
        });
    }
    // 有的节点对待打包的交易也返回回执,这时区块号是空的.
    let block = match receipt.get("blockNumber") {
        Some(Value::Null) | None => return Ok(Tx::pending(hash)),
        Some(block) => hex_u64(block)?,
    };
    let succeeded = match hex_u64(receipt.get("status").ok_or(Failed)?)? {
        1 => true,
        0 => false,
        _ => return Err(Failed),
    };
    let gas = hex_u128(receipt.get("gasUsed").ok_or(Failed)?)?;
    let price = hex_u128(
        receipt
            .get("effectiveGasPrice")
            .or_else(|| tx.get("gasPrice"))
            .ok_or(Failed)?,
    )?;
    let stage = tips.stage(block);
    let status = match (succeeded, stage) {
        (false, _) => TxStatus::Failed,
        (true, "finalized") => TxStatus::Finalized,
        (true, _) => TxStatus::Included,
    };
    Ok(Tx {
        hash: hash.to_string(),
        status,
        confirmations: tips.latest.saturating_sub(block) + 1,
        stage: Some(stage),
        block: Some(block),
        fee: Some(gas.checked_mul(price).ok_or(Failed)?.to_string()),
    })
}

/// 非零优先费的中位数.空块的优先费是 0,算进去会把估算压低.
fn median_reward(rewards: &[Value], percentile: usize) -> Result<Option<u128>, Failed> {
    let mut values = Vec::new();
    for block in rewards {
        if let Some(value) = block.get(percentile) {
            values.push(hex_u128(value)?);
        }
    }
    values.retain(|value| *value > 0);
    values.sort_unstable();
    Ok(values.get(values.len() / 2).copied())
}

fn fee_tiers(history: &Value, suggested: &Value) -> Result<Value, Failed> {
    // 最后一项是下一个区块的基础费.
    let base = hex_u128(
        history
            .get("baseFeePerGas")
            .and_then(Value::as_array)
            .and_then(|fees| fees.last())
            .ok_or(Failed)?,
    )?;
    let empty = Vec::new();
    let rewards = history.get("reward").and_then(Value::as_array).unwrap_or(&empty);
    let mut tiers = Vec::new();
    let mut floor = 0;
    for (index, (numerator, denominator)) in BASE_FEE_MARGIN.into_iter().enumerate() {
        let priority = match median_reward(rewards, index)? {
            Some(priority) => priority,
            // 最近的区块都是空的,节点的建议值就是下限,低于它可能不被打包.
            None => hex_u128(suggested)?,
        }
        .max(floor);
        floor = priority;
        let max_fee = base.checked_mul(numerator).ok_or(Failed)? / denominator + priority;
        tiers.push(json!({"maxFee": max_fee.to_string(), "priority": priority.to_string()}));
    }
    let [slow, medium, fast] = <[Value; 3]>::try_from(tiers).map_err(|_| Failed)?;
    Ok(json!({"kind": "evm", "baseFee": base.to_string(), "slow": slow, "medium": medium, "fast": fast}))
}

fn is_already_known(message: &str) -> bool {
    let message = message.to_lowercase().replace([' ', '_'], "");
    ALREADY_KNOWN.iter().any(|known| message.contains(known))
}

impl Evm {
    /// 发一批调用,结果按请求顺序返回,每项的错误单独保留.批量响应不保证顺序,要按 id 归位.
    async fn batch_items(&self, calls: &[(&str, Value)]) -> Result<Vec<Option<Value>>, Failed> {
        let body: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(id, (method, params))| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .collect();
        let reply = self.http.post(&Value::Array(body)).await?;
        // 被限流时有的服务商只回一个错误对象,不是数组.
        let items = reply.as_array().ok_or(Failed)?;
        let mut results = vec![None; calls.len()];
        for item in items {
            let id = item["id"].as_u64().ok_or(Failed)? as usize;
            let failed = item.get("error").is_some_and(|error| !error.is_null());
            *results.get_mut(id).ok_or(Failed)? = Some(if failed { None } else { item.get("result").cloned() });
        }
        results.into_iter().map(|result| result.ok_or(Failed)).collect()
    }

    /// 一批调用,任何一项出错都算失败.
    pub(super) async fn batch(&self, calls: &[(&str, Value)]) -> Result<Vec<Value>, Failed> {
        self.batch_items(calls)
            .await?
            .into_iter()
            .map(|item| item.ok_or(Failed))
            .collect()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, Failed> {
        self.http.rpc(method, params).await?.map_err(|_| Failed)
    }

    pub(super) async fn tips(&self) -> Result<Tips, Failed> {
        let calls = [
            ("eth_blockNumber", json!([])),
            ("eth_getBlockByNumber", json!(["safe", false])),
            ("eth_getBlockByNumber", json!(["finalized", false])),
        ];
        let items = self.batch_items(&calls).await?;
        let number = |index: usize| {
            let block = items[index].as_ref().ok_or(Failed)?;
            hex_u64(block.get("number").ok_or(Failed)?)
        };
        let latest = items[0].as_ref().ok_or(Failed).and_then(hex_u64)?;
        Ok(Tips {
            latest,
            safe: number(1).ok(),
            finalized: number(2)?,
        })
    }

    async fn tx_batch(&self, hashes: &[String], tips: &Tips) -> Result<Vec<Tx>, Failed> {
        let calls: Vec<(&str, Value)> = hashes
            .iter()
            .flat_map(|hash| {
                [
                    ("eth_getTransactionReceipt", json!([hash])),
                    ("eth_getTransactionByHash", json!([hash])),
                ]
            })
            .collect();
        let results = self.batch(&calls).await?;
        hashes
            .iter()
            .zip(results.chunks(2))
            .map(|(hash, pair)| parse_tx(hash, &pair[0], &pair[1], tips))
            .collect()
    }
}

/// 日志的地址参数和结果都是补零到 32 字节的小写地址.
pub(super) fn log_topic(address: &str) -> String {
    format!("0x{:0>64}", address[2..].to_ascii_lowercase())
}

pub(super) fn topic_address(topic: Option<&Value>) -> Result<String, Failed> {
    let topic = topic.and_then(Value::as_str).ok_or(Failed)?;
    let hex = topic.get(topic.len().saturating_sub(40)..).ok_or(Failed)?;
    Ok(format!("0x{hex}"))
}

pub(super) fn item_status(failed: bool, block: u64, tips: &Tips) -> TxStatus {
    match (failed, block <= tips.finalized) {
        (true, _) => TxStatus::Failed,
        (false, true) => TxStatus::Finalized,
        (false, false) => TxStatus::Included,
    }
}

impl Upstream for Evm {
    fn watch(&self, addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.follow(addresses, signals).await;
        })
    }

    fn recent<'a>(&'a self, addresses: &'a [String], previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(self.scan(addresses, previous))
    }

    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>> {
        Box::pin(async move {
            let assets: Vec<&str> = once(NATIVE).chain(self.tokens.iter().map(String::as_str)).collect();
            let rows: Vec<(&str, &str)> = addresses
                .iter()
                .flat_map(|a| assets.iter().map(move |t| (a.as_str(), *t)))
                .collect();
            let calls: Vec<(&str, Value)> = rows.iter().map(|(a, t)| evm_call(a, t)).collect();
            let results = run_all(calls.chunks(BATCH_SIZE).map(|chunk| self.batch(chunk)).collect()).await?;
            rows.iter()
                .zip(&results)
                .map(|((address, asset), hex)| {
                    Ok(balance(
                        address,
                        asset,
                        hex_to_decimal(hex.as_str().ok_or(Failed)?).ok_or(Failed)?,
                    ))
                })
                .collect()
        })
    }

    fn height(&self) -> Query<'_, u64> {
        Box::pin(async { hex_u64(&self.call("eth_blockNumber", json!([])).await?) })
    }

    fn fees(&self) -> Query<'_, Value> {
        Box::pin(async {
            let calls = [
                ("eth_feeHistory", json!([FEE_BLOCKS, "latest", FEE_PERCENTILES])),
                ("eth_maxPriorityFeePerGas", json!([])),
            ];
            let results = self.batch(&calls).await?;
            fee_tiers(&results[0], &results[1])
        })
    }

    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>> {
        Box::pin(async move {
            let tips = self.tips().await?;
            run_all(
                hashes
                    .chunks(TX_BATCH)
                    .map(|chunk| self.tx_batch(chunk, &tips))
                    .collect(),
            )
            .await
        })
    }

    /// 没有配置索引服务时明确报错.返回空列表会让用户以为没有交易记录.
    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(async move {
            match self.indexer.as_ref().ok_or(Failed)? {
                Indexer::Etherscan(indexer) => self.etherscan_page(indexer, addresses, cursor, since).await,
                Indexer::HyperSync(indexer) => self.hypersync_page(indexer, addresses, cursor, since).await,
            }
        })
    }

    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast> {
        Box::pin(async move {
            let params = json!([format!("0x{}", raw.hex())]);
            match self.http.rpc_send("eth_sendRawTransaction", params).await {
                Ok(Ok(_)) => Broadcast::Accepted,
                Ok(Err(error)) if is_already_known(&error.message) => Broadcast::Accepted,
                Ok(Err(error)) if error.is_rejection() => rejected(self, raw, error.message).await,
                Ok(Err(_)) => Broadcast::Unknown,
                Err(outcome) => outcome,
            }
        })
    }
}

impl Evm {
    /// 内层的 `Err` 是节点拒绝这次调用,带节点原文.
    pub fn estimate<'a>(&'a self, call: &'a Value) -> Query<'a, Result<Value, String>> {
        Box::pin(async move {
            match self.http.rpc("eth_estimateGas", json!([call])).await? {
                Ok(gas) => {
                    let gas = hex_to_decimal(gas.as_str().ok_or(Failed)?).ok_or(Failed)?;
                    Ok(Ok(json!({"gas": gas})))
                }
                Err(error) if error.is_rejection() => Ok(Err(error.message)),
                Err(_) => Err(Failed),
            }
        })
    }

    pub fn nonces<'a>(&'a self, addresses: &'a [String]) -> Query<'a, BTreeMap<String, Value>> {
        Box::pin(async move {
            let calls: Vec<(&str, Value)> = addresses
                .iter()
                .flat_map(|address| ["latest", "pending"].map(|tag| ("eth_getTransactionCount", json!([address, tag]))))
                .collect();
            let results = run_all(calls.chunks(BATCH_SIZE).map(|chunk| self.batch(chunk)).collect()).await?;
            addresses
                .iter()
                .zip(results.chunks(2))
                .map(|(address, pair)| {
                    Ok((
                        address.clone(),
                        json!({"latest": hex_u64(&pair[0])?, "pending": hex_u64(&pair[1])?}),
                    ))
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0xabc";

    fn tips() -> Tips {
        Tips {
            latest: 100,
            safe: Some(90),
            finalized: 80,
        }
    }

    fn receipt(block: u64, status: &str) -> Value {
        json!({
            "blockNumber": format!("{block:#x}"),
            "status": status,
            "gasUsed": "0x5208",
            "effectiveGasPrice": "0x3b9aca00",
        })
    }

    #[test]
    fn balance_of_call_pads_the_lowercase_owner() {
        let (method, params) = evm_call("0x9858EfFD232B4033E47d90003D41EC34EcaEda94", "0xToken");
        assert_eq!(method, "eth_call");
        assert_eq!(
            params[0]["data"],
            "0x70a082310000000000000000000000009858effd232b4033e47d90003d41ec34ecaeda94"
        );
    }

    #[test]
    fn stage_follows_the_block_tags() {
        let at = |block| parse_tx(HASH, &receipt(block, "0x1"), &json!({}), &tips()).unwrap();
        let tx = at(100);
        assert_eq!(
            (tx.status, tx.stage, tx.confirmations),
            (TxStatus::Included, Some("latest"), 1)
        );
        let tx = at(91);
        assert_eq!((tx.status, tx.stage), (TxStatus::Included, Some("latest")));
        let tx = at(90);
        assert_eq!(
            (tx.status, tx.stage, tx.confirmations),
            (TxStatus::Included, Some("safe"), 11)
        );
        let tx = at(81);
        assert_eq!((tx.status, tx.stage), (TxStatus::Included, Some("safe")));
        let tx = at(80);
        assert_eq!((tx.status, tx.stage), (TxStatus::Finalized, Some("finalized")));
        assert_eq!(tx.fee.as_deref(), Some("21000000000000"));
        assert_eq!(tx.block, Some(80));
    }

    #[test]
    fn reverted_receipts_are_failed_at_every_stage() {
        for block in [100, 90, 80, 1] {
            let tx = parse_tx(HASH, &receipt(block, "0x0"), &json!({}), &tips()).unwrap();
            assert_eq!(tx.status, TxStatus::Failed, "第 {block} 块");
            assert!(tx.fee.is_some(), "失败的交易也扣了手续费");
        }
        let tx = parse_tx(HASH, &receipt(80, "0x0"), &json!({}), &tips()).unwrap();
        assert_eq!(tx.stage, Some("finalized"), "阶段照常报告");
    }

    #[test]
    fn a_receipt_without_a_clear_status_is_never_a_success() {
        // 拜占庭分叉之前的回执只有 root,没有 status.
        let mut old = receipt(1, "0x1");
        old.as_object_mut().unwrap().remove("status");
        assert!(parse_tx(HASH, &old, &json!({}), &tips()).is_err());
        assert!(parse_tx(HASH, &receipt(1, "0x2"), &json!({}), &tips()).is_err());
        assert!(parse_tx(HASH, &receipt(1, "1"), &json!({}), &tips()).is_err());
    }

    #[test]
    fn safe_below_finalized_does_not_downgrade_the_stage() {
        // BSC 上观察到的情况.
        let tips = Tips {
            latest: 100,
            safe: Some(70),
            finalized: 80,
        };
        assert_eq!(tips.stage(75), "finalized");
        assert_eq!(tips.stage(80), "finalized");
        assert_eq!(tips.stage(81), "latest");
    }

    #[test]
    fn missing_safe_tag_skips_the_stage() {
        let tips = Tips {
            latest: 100,
            safe: None,
            finalized: 80,
        };
        assert_eq!(tips.stage(90), "latest");
        assert_eq!(tips.stage(80), "finalized");
    }

    #[test]
    fn no_receipt_means_pending_or_unknown() {
        let pending = parse_tx(HASH, &Value::Null, &json!({"hash": HASH, "blockNumber": null}), &tips()).unwrap();
        assert_eq!(pending.status, TxStatus::Pending);
        assert_eq!(
            parse_tx(HASH, &Value::Null, &Value::Null, &tips()).unwrap().status,
            TxStatus::Unknown
        );
        let unmined = json!({"blockNumber": null, "status": "0x1"});
        assert_eq!(
            parse_tx(HASH, &unmined, &json!({}), &tips()).unwrap().status,
            TxStatus::Pending
        );
    }

    #[test]
    fn fee_falls_back_to_the_transaction_gas_price() {
        let mut old = receipt(100, "0x1");
        old.as_object_mut().unwrap().remove("effectiveGasPrice");
        let tx = parse_tx(HASH, &old, &json!({"gasPrice": "0x2"}), &tips()).unwrap();
        assert_eq!(tx.fee.as_deref(), Some("42000"));
        assert!(parse_tx(HASH, &old, &json!({}), &tips()).is_err());
    }

    #[test]
    fn fee_tiers_use_the_next_base_fee_and_nonzero_rewards() {
        let history = json!({
            "baseFeePerGas": ["0x1", "0x64"],
            "reward": [["0x0", "0x0", "0x0"], ["0xa", "0x14", "0x1e"], ["0xc", "0x16", "0x20"], ["0xe", "0x18", "0x22"]],
        });
        assert_eq!(
            fee_tiers(&history, &json!("0x1")).unwrap(),
            json!({
                "kind": "evm",
                "baseFee": "100",
                "slow": {"maxFee": "137", "priority": "12"},
                "medium": {"maxFee": "172", "priority": "22"},
                "fast": {"maxFee": "232", "priority": "32"},
            }),
        );
    }

    #[test]
    fn empty_blocks_fall_back_to_the_node_suggestion() {
        let history = json!({"baseFeePerGas": ["0x0"], "reward": [["0x0", "0x0", "0x0"]]});
        let fees = fee_tiers(&history, &json!("0x2faf080")).unwrap();
        assert_eq!(fees["baseFee"], "0");
        assert_eq!(fees["slow"], json!({"maxFee": "50000000", "priority": "50000000"}));
        // 新链没有 reward 字段.
        let fees = fee_tiers(&json!({"baseFeePerGas": ["0x4"]}), &json!("0x1")).unwrap();
        assert_eq!(fees["fast"], json!({"maxFee": "9", "priority": "1"}));
    }

    #[test]
    fn priorities_never_decrease_across_tiers() {
        let history = json!({"baseFeePerGas": ["0x0"], "reward": [["0x5", "0x0", "0x1"]]});
        let fees = fee_tiers(&history, &json!("0x3")).unwrap();
        assert_eq!(fees["slow"]["priority"], "5");
        assert_eq!(fees["medium"]["priority"], "5");
        assert_eq!(fees["fast"]["priority"], "5");
    }

    #[test]
    fn malformed_fee_history_is_an_upstream_failure() {
        for bad in [json!({}), json!({"baseFeePerGas": []}), json!({"baseFeePerGas": ["x"]})] {
            assert!(fee_tiers(&bad, &json!("0x1")).is_err(), "{bad}");
        }
    }

    #[test]
    fn already_known_covers_the_node_wording() {
        for message in [
            "already known",
            "AlreadyKnown",
            "ALREADY_KNOWN",
            "Known transaction",
            "transaction already imported",
        ] {
            assert!(is_already_known(message), "{message}");
        }
        for message in [
            "nonce too low",
            "replacement transaction underpriced",
            "insufficient funds",
        ] {
            assert!(!is_already_known(message), "{message}");
        }
    }
}

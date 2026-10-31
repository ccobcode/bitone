//! EVM。普通 JSON-RPC 节点。交易历史节点查不到，要配置一个 Etherscan 兼容的索引服务。

use super::{
    Balance, Broadcast, Cursor, Estimate, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, Signal, StreamPage,
    Tx, TxStatus, Upstream, balance, decimal, hex_to_decimal, hex_u64, hex_u128, merge_pages, rejected, run_all,
    stream_index, streams, watch_heads,
};
use crate::raw::RawTx;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::iter::once;
use tokio::sync::{mpsc, watch};

/// ERC-20 的 balanceOf(address)。
const BALANCE_OF: &str = "70a08231";
/// 单个 JSON-RPC 批次的调用数。服务商对批次大小都有上限。
const BATCH_SIZE: usize = 100;
/// 查询一笔交易要两个调用。
const TX_BATCH: usize = BATCH_SIZE / 2;
/// 手续费按最近这么多个区块统计。
const FEE_BLOCKS: &str = "0x14";
/// 慢、中、快三档的优先费分位。
const FEE_PERCENTILES: [u64; 3] = [25, 50, 75];
/// 三档最高单价相对基础费的倍数，分子分母。基础费每个区块最多涨 12.5%，给足余量。
const BASE_FEE_MARGIN: [(u128, u128); 3] = [(5, 4), (3, 2), (2, 1)];
/// 节点报出这些内容时，交易已经在内存池里。各家客户端的写法不同，有的是错误名，
/// 所以先去掉空格和下划线再比较。
const ALREADY_KNOWN: [&str; 3] = ["alreadyknown", "knowntransaction", "alreadyimported"];
/// 索引服务的三类记录：普通交易、内部转账、代币转账。
const ACTIONS: [&str; 3] = ["txlist", "txlistinternal", "tokentx"];
const TOKEN_ACTION: &str = "tokentx";
const PAGE_SIZE: usize = 50;
/// ERC-20 的 Transfer 事件。
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// 跟块时最多回看的区块数。这里要取完整区块，只在地址有变化时才查。
const MAX_SCAN: u64 = 32;

pub struct Evm {
    pub http: Http,
    pub tokens: Vec<String>,
    pub indexer: Option<Http>,
    /// `indexer` 是 HyperSync，不是 Etherscan 兼容的接口。
    pub hypersync: bool,
}

fn evm_call(address: &str, asset: &str) -> (&'static str, Value) {
    if asset == NATIVE {
        return ("eth_getBalance", json!([address, "latest"]));
    }
    let owner = address[2..].to_ascii_lowercase();
    let data = format!("0x{BALANCE_OF}{owner:0>64}");
    ("eth_call", json!([{"to": asset, "data": data}, "latest"]))
}

/// 链上的三个位置。
struct Tips {
    latest: u64,
    /// 有些节点不支持 `safe`，比如 Polygon。它只是中间阶段，缺了不影响完成标准。
    safe: Option<u64>,
    finalized: u64,
}

impl Tips {
    /// 先看 `finalized`：BSC 的 `safe` 可能比 `finalized` 还低。
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

/// 回执映射成交易状态。回执里的 status 决定成败，和区块是否最终确定无关。
fn parse_tx(hash: &str, receipt: &Value, tx: &Value, tips: &Tips) -> Result<Tx, Failed> {
    if receipt.is_null() {
        return Ok(if tx.is_null() {
            Tx::unknown(hash)
        } else {
            Tx::pending(hash)
        });
    }
    // 有的节点对待打包的交易也返回回执，这时区块号是空的。
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

/// 非零优先费的中位数。空块的优先费是 0，算进去会把估算压低。
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
    // 最后一项是下一个区块的基础费。
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
            // 最近的区块都是空的，节点的建议值就是下限，低于它可能不被打包。
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

fn number(row: &Value, key: &str) -> Result<u64, Failed> {
    row.get(key)
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
        .ok_or(Failed)
}

/// 索引服务的一行记录。代币只保留清单里的，普通交易里金额为 0 的成功调用不是转账。
fn history_item(row: &Value, action: &str, tokens: &[String], tips: &Tips) -> Result<Option<HistoryItem>, Failed> {
    let hash = row
        .get("hash")
        .or_else(|| row.get("transactionHash"))
        .and_then(Value::as_str)
        .ok_or(Failed)?;
    let text = |key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let failed = row.get("isError").and_then(Value::as_str) == Some("1")
        || row.get("txreceipt_status").and_then(Value::as_str) == Some("0");
    let amount = decimal(row.get("value").ok_or(Failed)?)?;
    let asset = if action == TOKEN_ACTION {
        let contract = row.get("contractAddress").and_then(Value::as_str).ok_or(Failed)?;
        match tokens.iter().find(|token| token.eq_ignore_ascii_case(contract)) {
            Some(token) => token.clone(),
            None => return Ok(None),
        }
    } else {
        if amount == "0" && !failed {
            return Ok(None);
        }
        NATIVE.to_string()
    };
    let status = match (failed, number(row, "blockNumber")? <= tips.finalized) {
        (true, _) => TxStatus::Failed,
        (false, true) => TxStatus::Finalized,
        (false, false) => TxStatus::Included,
    };
    Ok(Some(HistoryItem {
        hash: hash.to_string(),
        time: Some(number(row, "timeStamp")?),
        from: text("from"),
        to: text("to"),
        asset,
        amount,
        status,
    }))
}

impl Evm {
    /// 发一批调用，结果按请求顺序返回，每项的错误单独保留。批量响应不保证顺序，要按 id 归位。
    async fn batch_items(&self, calls: &[(&str, Value)]) -> Result<Vec<Option<Value>>, Failed> {
        let body: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(id, (method, params))| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .collect();
        let reply = self.http.post(&Value::Array(body)).await?;
        // 被限流时有的服务商只回一个错误对象，不是数组。
        let items = reply.as_array().ok_or(Failed)?;
        let mut results = vec![None; calls.len()];
        for item in items {
            let id = item["id"].as_u64().ok_or(Failed)? as usize;
            let failed = item.get("error").is_some_and(|error| !error.is_null());
            *results.get_mut(id).ok_or(Failed)? = Some(if failed { None } else { item.get("result").cloned() });
        }
        results.into_iter().map(|result| result.ok_or(Failed)).collect()
    }

    /// 一批调用，任何一项出错都算失败。
    async fn batch(&self, calls: &[(&str, Value)]) -> Result<Vec<Value>, Failed> {
        self.batch_items(calls)
            .await?
            .into_iter()
            .map(|item| item.ok_or(Failed))
            .collect()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, Failed> {
        self.http.rpc(method, params).await?.map_err(|_| Failed)
    }

    async fn tips(&self) -> Result<Tips, Failed> {
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

    async fn stream(
        &self,
        indexer: &Http,
        key: String,
        token: Option<String>,
        addresses: &[String],
        tips: &Tips,
        since: Option<u64>,
    ) -> Result<Vec<StreamPage>, Failed> {
        let address = stream_index(&key)
            .and_then(|index| addresses.get(index))
            .ok_or(Failed)?;
        let action = key
            .split(':')
            .nth(1)
            .filter(|action| ACTIONS.contains(action))
            .ok_or(Failed)?;
        let page: u32 = token.as_deref().map_or(Ok(1), str::parse).map_err(|_| Failed)?;
        let start = since.map_or(0, |since| since + 1).to_string();
        let query = [
            ("module", "account"),
            ("action", action),
            ("address", address),
            ("page", &page.to_string()),
            ("offset", &PAGE_SIZE.to_string()),
            ("sort", "desc"),
            ("startblock", &start),
        ];
        let reply = indexer.get_query("", &query).await?;
        // 没有记录时 status 是 0 而 result 是空数组，限流时 result 是一段文本。
        let rows = reply.get("result").and_then(Value::as_array).ok_or(Failed)?;
        let items = rows
            .iter()
            .filter_map(|row| history_item(row, action, &self.tokens, tips).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        let next = (rows.len() >= PAGE_SIZE).then(|| (page + 1).to_string());
        Ok(vec![StreamPage { key, next, items }])
    }
}

/// HyperSync 返回的字段。
fn hypersync_fields() -> Value {
    json!({
        "block": ["number", "timestamp"],
        "transaction": ["hash", "from", "to", "value", "status", "block_number"],
        "log": ["address", "topic0", "topic1", "topic2", "data", "transaction_hash", "block_number"],
    })
}

/// 金额为 0 时可能是空的 `0x`，不能当成格式错误。
fn hex_amount(value: Option<&Value>) -> Result<String, Failed> {
    match value.and_then(Value::as_str).ok_or(Failed)? {
        "0x" => Ok("0".to_string()),
        hex => hex_to_decimal(hex).ok_or(Failed),
    }
}

/// 日志的地址参数和结果都是补零到 32 字节的小写地址。
fn log_topic(address: &str) -> String {
    format!("0x{:0>64}", address[2..].to_ascii_lowercase())
}

fn topic_address(topic: Option<&Value>) -> Result<String, Failed> {
    let topic = topic.and_then(Value::as_str).ok_or(Failed)?;
    let hex = topic.get(topic.len().saturating_sub(40)..).ok_or(Failed)?;
    Ok(format!("0x{hex}"))
}

fn item_status(failed: bool, block: u64, tips: &Tips) -> TxStatus {
    match (failed, block <= tips.finalized) {
        (true, _) => TxStatus::Failed,
        (false, true) => TxStatus::Finalized,
        (false, false) => TxStatus::Included,
    }
}

impl Evm {
    /// HyperSync 的一页，一个请求算一页。它只能从旧到新扫，所以游标是下一个要扫的区块。
    /// 免费额度是每分钟 15 次，跟块不用它，只用来补历史。
    // ponytail: 记录多到一个请求扫不完时，最早的在前，中间的要翻页才看得到。
    // 这类地址多了以后改成从最新的区块往回分段扫。
    async fn hypersync_page(
        &self,
        indexer: &Http,
        addresses: &[String],
        cursor: &Cursor,
        since: Option<u64>,
    ) -> Result<HistoryPage, Failed> {
        let tips = self.tips().await?;
        let from = match cursor.get("0") {
            Some(token) => token.parse().map_err(|_| Failed)?,
            None => since.map_or(0, |since| since + 1),
        };
        let own: Vec<String> = addresses.iter().map(|a| a.to_ascii_lowercase()).collect();
        let tokens: Vec<String> = self.tokens.iter().map(|t| t.to_ascii_lowercase()).collect();
        let topics: Vec<String> = addresses.iter().map(|address| log_topic(address)).collect();
        let logs = if tokens.is_empty() {
            json!([])
        } else {
            json!([
                {"address": tokens, "topics": [[TRANSFER], topics, [], []]},
                {"address": tokens, "topics": [[TRANSFER], [], topics, []]},
            ])
        };
        let body = json!({
            "from_block": from,
            "transactions": [{"from": own}, {"to": own}],
            "logs": logs,
            "field_selection": hypersync_fields(),
        });
        let reply = indexer.post_to("/query", &body).await?;
        let next = reply.get("next_block").and_then(Value::as_u64).ok_or(Failed)?;
        let archive = reply.get("archive_height").and_then(Value::as_u64).ok_or(Failed)?;
        let chunks = reply.get("data").and_then(Value::as_array).ok_or(Failed)?;
        let rows = |key: &'static str| {
            chunks
                .iter()
                .flat_map(move |chunk| chunk.get(key).and_then(Value::as_array).into_iter().flatten())
        };

        let mut times = HashMap::new();
        for block in rows("blocks") {
            let number = block.get("number").and_then(Value::as_u64).ok_or(Failed)?;
            times.insert(number, hex_u64(block.get("timestamp").ok_or(Failed)?)?);
        }
        let mut items = Vec::new();
        for tx in rows("transactions") {
            let block = tx.get("block_number").and_then(Value::as_u64).ok_or(Failed)?;
            let failed = tx.get("status").and_then(Value::as_u64) == Some(0);
            let amount = hex_amount(tx.get("value"))?;
            if amount == "0" && !failed {
                continue;
            }
            let party = |key: &str| tx.get(key).and_then(Value::as_str).map(str::to_string);
            items.push(HistoryItem {
                hash: tx.get("hash").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                time: times.get(&block).copied(),
                from: party("from"),
                to: party("to"),
                asset: NATIVE.to_string(),
                amount,
                status: item_status(failed, block, &tips),
            });
        }
        for log in rows("logs") {
            let contract = log.get("address").and_then(Value::as_str).ok_or(Failed)?;
            let Some(asset) = self.tokens.iter().find(|token| token.eq_ignore_ascii_case(contract)) else {
                continue;
            };
            let block = log.get("block_number").and_then(Value::as_u64).ok_or(Failed)?;
            items.push(HistoryItem {
                hash: log
                    .get("transaction_hash")
                    .and_then(Value::as_str)
                    .ok_or(Failed)?
                    .to_string(),
                time: times.get(&block).copied(),
                from: Some(topic_address(log.get("topic1"))?),
                to: Some(topic_address(log.get("topic2"))?),
                asset: asset.clone(),
                amount: hex_amount(log.get("data"))?,
                status: item_status(false, block, &tips),
            });
        }
        let page = StreamPage {
            key: "0".to_string(),
            next: (next < archive).then(|| next.to_string()),
            items,
        };
        Ok(HistoryPage {
            // 它可能比节点慢几个块，水位不能超过它扫到的位置。
            checkpoint: tips.finalized.min(archive),
            ..merge_pages(vec![page])
        })
    }

    /// 直接从节点取最近的交易。索引服务有延迟，BSC 和 Polygon 也没有免费的，所以跟块不用它。
    // ponytail: 合约内部转入的原生币在区块里看不到，只更新余额，交易记录要等索引服务。
    // 没有索引服务的网络要显示这部分时接 trace 接口。
    async fn scan(&self, addresses: &[String], previous: u64) -> Result<Vec<HistoryItem>, Failed> {
        let tips = self.tips().await?;
        let first = (previous + 1)
            .max(tips.latest.saturating_sub(MAX_SCAN - 1))
            .min(tips.latest);
        let own: HashSet<String> = addresses.iter().map(|a| a.to_ascii_lowercase()).collect();
        let hex = |block: u64| format!("{block:#x}");

        let mut calls: Vec<(&str, Value)> = (first..=tips.latest)
            .map(|block| ("eth_getBlockByNumber", json!([hex(block), true])))
            .collect();
        let blocks = calls.len();
        if !self.tokens.is_empty() {
            let topics: Vec<String> = addresses.iter().map(|address| log_topic(address)).collect();
            let sent = json!([TRANSFER, topics, null]);
            let received = json!([TRANSFER, null, topics]);
            for topics in [sent, received] {
                let filter = json!({
                    "fromBlock": hex(first),
                    "toBlock": hex(tips.latest),
                    "address": self.tokens,
                    "topics": topics,
                });
                calls.push(("eth_getLogs", json!([filter])));
            }
        }
        let results = self.batch(&calls).await?;
        let (blocks, logs) = results.split_at(blocks);

        let mut times = HashMap::new();
        let mut native = Vec::new();
        for block in blocks {
            let number = hex_u64(block.get("number").ok_or(Failed)?)?;
            times.insert(number, hex_u64(block.get("timestamp").ok_or(Failed)?)?);
            let rows = block.get("transactions").and_then(Value::as_array).ok_or(Failed)?;
            for tx in rows {
                let party = |key: &str| tx.get(key).and_then(Value::as_str).map(str::to_ascii_lowercase);
                let (from, to) = (party("from"), party("to"));
                let mine = |party: &Option<String>| party.as_ref().is_some_and(|party| own.contains(party));
                if mine(&from) || mine(&to) {
                    native.push((number, tx, from, to));
                }
            }
        }
        let receipts: Vec<(&str, Value)> = native
            .iter()
            .map(|(_, tx, ..)| ("eth_getTransactionReceipt", json!([tx.get("hash")])))
            .collect();
        let receipts = if receipts.is_empty() {
            Vec::new()
        } else {
            self.batch(&receipts).await?
        };

        let mut items = Vec::new();
        for ((block, tx, from, to), receipt) in native.into_iter().zip(&receipts) {
            let failed = receipt.get("status").and_then(Value::as_str) == Some("0x0");
            let amount = hex_to_decimal(tx.get("value").and_then(Value::as_str).ok_or(Failed)?).ok_or(Failed)?;
            // 和索引服务的规则一样：金额为 0 的成功调用不是转账。
            if amount == "0" && !failed {
                continue;
            }
            items.push(HistoryItem {
                hash: tx.get("hash").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                time: times.get(&block).copied(),
                from,
                to,
                asset: NATIVE.to_string(),
                amount,
                status: item_status(failed, block, &tips),
            });
        }
        for log in logs.iter().flat_map(|logs| logs.as_array().into_iter().flatten()) {
            let contract = log.get("address").and_then(Value::as_str).ok_or(Failed)?;
            let Some(asset) = self.tokens.iter().find(|token| token.eq_ignore_ascii_case(contract)) else {
                continue;
            };
            let block = hex_u64(log.get("blockNumber").ok_or(Failed)?)?;
            items.push(HistoryItem {
                hash: log
                    .get("transactionHash")
                    .and_then(Value::as_str)
                    .ok_or(Failed)?
                    .to_string(),
                time: times.get(&block).copied(),
                from: Some(topic_address(log.pointer("/topics/1"))?),
                to: Some(topic_address(log.pointer("/topics/2"))?),
                asset: asset.clone(),
                amount: hex_to_decimal(log.get("data").and_then(Value::as_str).ok_or(Failed)?).ok_or(Failed)?,
                status: item_status(false, block, &tips),
            });
        }
        // 自己转给自己的代币会同时出现在转出和转入两次查询里。
        let mut seen = HashSet::new();
        items.retain(|item| seen.insert(item.clone()));
        Ok(items)
    }
}

impl Upstream for Evm {
    fn watch(&self, _addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        let subscribe = json!({"jsonrpc": "2.0", "id": 1, "method": "eth_subscribe", "params": ["newHeads"]});
        let number = |result: &Value| hex_u64(result.get("number")?).ok();
        Box::pin(watch_heads(self.http.socket_url(), subscribe, number, signals))
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

    /// 没有配置索引服务时明确报错。返回空列表会让用户以为没有交易记录。
    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(async move {
            let indexer = self.indexer.as_ref().ok_or(Failed)?;
            if self.hypersync {
                return self.hypersync_page(indexer, addresses, cursor, since).await;
            }
            let tips = self.tips().await?;
            let tips = &tips;
            let keys =
                (0..addresses.len()).flat_map(|index| ACTIONS.iter().map(move |action| format!("{index}:{action}")));
            let queries = streams(keys, cursor)
                .into_iter()
                .map(|(key, token)| self.stream(indexer, key, token, addresses, tips, since))
                .collect();
            let page = merge_pages(run_all(queries).await?);
            Ok(HistoryPage {
                checkpoint: tips.finalized,
                ..page
            })
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

    fn estimate<'a>(&'a self, request: &'a Estimate) -> Query<'a, Result<Value, String>> {
        Box::pin(async move {
            let Estimate::Evm { from, to, value, data } = request else {
                return Err(Failed);
            };
            let call = json!({"from": from, "to": to, "value": value, "data": data});
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

    fn nonces<'a>(&'a self, addresses: &'a [String]) -> Query<'a, BTreeMap<String, Value>> {
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
        // 拜占庭分叉之前的回执只有 root，没有 status。
        let mut old = receipt(1, "0x1");
        old.as_object_mut().unwrap().remove("status");
        assert!(parse_tx(HASH, &old, &json!({}), &tips()).is_err());
        assert!(parse_tx(HASH, &receipt(1, "0x2"), &json!({}), &tips()).is_err());
        assert!(parse_tx(HASH, &receipt(1, "1"), &json!({}), &tips()).is_err());
    }

    #[test]
    fn safe_below_finalized_does_not_downgrade_the_stage() {
        // BSC 上观察到的情况。
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
        // 新链没有 reward 字段。
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

    fn row(value: &str, is_error: &str) -> Value {
        json!({
            "hash": HASH, "from": "0xa", "to": "0xb", "value": value, "isError": is_error,
            "blockNumber": "85", "timeStamp": "1700000000", "contractAddress": "",
        })
    }

    #[test]
    fn history_maps_status_from_tags_and_receipt_flags() {
        let item = history_item(&row("5", "0"), "txlist", &[], &tips()).unwrap().unwrap();
        assert_eq!(
            (item.status, item.amount.as_str(), item.asset.as_str()),
            (TxStatus::Included, "5", NATIVE)
        );
        let mut settled = row("5", "0");
        settled["blockNumber"] = json!("80");
        assert_eq!(
            history_item(&settled, "txlist", &[], &tips()).unwrap().unwrap().status,
            TxStatus::Finalized
        );
        settled["txreceipt_status"] = json!("0");
        assert_eq!(
            history_item(&settled, "txlist", &[], &tips()).unwrap().unwrap().status,
            TxStatus::Failed
        );
    }

    #[test]
    fn zero_value_calls_are_skipped_unless_they_failed() {
        assert!(history_item(&row("0", "0"), "txlist", &[], &tips()).unwrap().is_none());
        let failed = history_item(&row("0", "1"), "txlist", &[], &tips()).unwrap().unwrap();
        assert_eq!(failed.status, TxStatus::Failed);
    }

    #[test]
    fn tokens_outside_the_list_are_dropped_and_listed_ones_use_our_spelling() {
        let tokens = vec!["0xAbC".to_string()];
        let mut transfer = row("7", "0");
        transfer["contractAddress"] = json!("0xabc");
        let item = history_item(&transfer, TOKEN_ACTION, &tokens, &tips())
            .unwrap()
            .unwrap();
        assert_eq!((item.asset.as_str(), item.amount.as_str()), ("0xAbC", "7"));
        transfer["contractAddress"] = json!("0xdef");
        assert!(
            history_item(&transfer, TOKEN_ACTION, &tokens, &tips())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn contract_creation_has_no_recipient() {
        let mut creation = row("1", "0");
        creation["to"] = json!("");
        assert_eq!(
            history_item(&creation, "txlist", &[], &tips()).unwrap().unwrap().to,
            None
        );
    }
}

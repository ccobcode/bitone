//! HyperSync 索引服务.

use super::adapter::{Evm, TRANSFER, item_status, log_topic, topic_address};
use crate::upstream::{
    Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, StreamPage, hex_to_decimal, hex_u64, merge_pages,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// HyperSync 返回的字段.
fn hypersync_fields() -> Value {
    json!({
        "block": ["number", "timestamp"],
        "transaction": ["hash", "from", "to", "value", "status", "block_number"],
        "log": ["address", "topic0", "topic1", "topic2", "data", "transaction_hash", "block_number"],
    })
}

/// 金额为 0 时可能是空的 `0x`,不能当成格式错误.
fn hex_amount(value: Option<&Value>) -> Result<String, Failed> {
    match value.and_then(Value::as_str).ok_or(Failed)? {
        "0x" => Ok("0".to_string()),
        hex => hex_to_decimal(hex).ok_or(Failed),
    }
}

impl Evm {
    /// HyperSync 的一页,一个请求算一页.它只能从旧到新扫,所以游标是下一个要扫的区块.
    /// 免费额度是每分钟 15 次,跟踪新区块时不用它,只用来补历史.
    // ponytail: 记录多到一个请求扫不完时,最早的在前,中间的要翻页才看得到.
    // 这类地址多了以后改成从最新的区块往回分段扫.
    pub(super) async fn hypersync_page(
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
                flows: None,
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
                flows: None,
            });
        }
        let page = StreamPage {
            key: "0".to_string(),
            next: (next < archive).then(|| next.to_string()),
            items,
        };
        Ok(HistoryPage {
            // 它可能比节点慢几个块,`checkpoint` 不能超过它扫到的位置.
            checkpoint: tips.finalized.min(archive),
            ..merge_pages(vec![page])
        })
    }
}

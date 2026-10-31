//! Etherscan 兼容的索引服务。

use super::adapter::{Evm, Tips};
use crate::upstream::{
    Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, StreamPage, TxStatus, decimal, merge_pages, run_all,
    stream_index, streams,
};
use serde_json::Value;

/// 索引服务的三类记录：普通交易、内部转账、代币转账。
const ACTIONS: [&str; 3] = ["txlist", "txlistinternal", "tokentx"];
const TOKEN_ACTION: &str = "tokentx";
const PAGE_SIZE: usize = 50;

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
        flows: None,
    }))
}

impl Evm {
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

    pub(super) async fn etherscan_page(
        &self,
        indexer: &Http,
        addresses: &[String],
        cursor: &Cursor,
        since: Option<u64>,
    ) -> Result<HistoryPage, Failed> {
        let tips = self.tips().await?;
        let tips = &tips;
        let keys = (0..addresses.len()).flat_map(|index| ACTIONS.iter().map(move |action| format!("{index}:{action}")));
        let queries = streams(keys, cursor)
            .into_iter()
            .map(|(key, token)| self.stream(indexer, key, token, addresses, tips, since))
            .collect();
        let page = merge_pages(run_all(queries).await?);
        Ok(HistoryPage {
            checkpoint: tips.finalized,
            ..page
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HASH: &str = "0xabc";

    fn tips() -> Tips {
        Tips {
            latest: 100,
            safe: Some(90),
            finalized: 80,
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

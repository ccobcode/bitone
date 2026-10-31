//! Solana 的上游适配器。

use crate::upstream::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, RawTx, Signal, StreamPage, Tx,
    TxStatus, Upstream, balance, merge_pages, rejected, run_all, stream_index,
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, watch};

/// 主网已有版本 1 的交易，不声明这里的版本号时 getTransaction 会直接报错。
const MAX_TX_VERSION: u64 = 1;
/// 代币账户的大小，用来算创建租金。Token-2022 带扩展的账户会更大。
const TOKEN_ACCOUNT_SIZE: u64 = 165;
/// 全网最低优先费经常是 0，只看分位数时三档都是 0，所以中和快两档有下限。
const MEDIUM_FLOOR: u64 = 1000;
const FAST_FLOOR: u64 = 100_000;
/// 一次历史查询里每条记录流的条数。getTransaction 要逐笔调用，这个值不能大。
const PAGE_SIZE: usize = 25;
/// 跟块时往回看的 slot 数，约两分钟。
const RECENT_SLOTS: u64 = 300;

pub struct Solana {
    pub http: Http,
    pub tokens: Vec<String>,
}

/// `getSignatureStatuses` 的一项映射成交易状态。带 err 就是失败，提交等级只决定阶段。
fn parse_tx(hash: &str, value: &Value, slot: u64, fee: Option<String>) -> Result<Tx, Failed> {
    if value.is_null() {
        return Ok(Tx::unknown(hash));
    }
    let err = value.get("err").ok_or(Failed)?;
    let stage = match value.get("confirmationStatus").and_then(Value::as_str).ok_or(Failed)? {
        "processed" => "processed",
        "confirmed" => "confirmed",
        "finalized" => "finalized",
        _ => return Err(Failed),
    };
    let status = match (!err.is_null(), stage) {
        (true, _) => TxStatus::Failed,
        (false, "finalized") => TxStatus::Finalized,
        (false, _) => TxStatus::Included,
    };
    let block = value.get("slot").and_then(Value::as_u64).ok_or(Failed)?;
    // 最终确定后节点不再给确认数，用 slot 差代替，这个字段始终是数字。
    let confirmations = value
        .get("confirmations")
        .and_then(Value::as_u64)
        .unwrap_or(slot.saturating_sub(block));
    Ok(Tx {
        hash: hash.to_string(),
        status,
        confirmations,
        stage: Some(stage),
        block: Some(block),
        fee,
    })
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() - 1) * percent / 100]
}

fn fee_tiers(reply: &Value) -> Result<Value, Failed> {
    let rows = reply.as_array().ok_or(Failed)?;
    let mut fees = rows
        .iter()
        .map(|row| row.get("prioritizationFee").and_then(Value::as_u64).ok_or(Failed))
        .collect::<Result<Vec<_>, _>>()?;
    fees.sort_unstable();
    let slow = percentile(&fees, 25);
    let medium = percentile(&fees, 50).max(MEDIUM_FLOOR);
    let fast = percentile(&fees, 90).max(FAST_FLOOR);
    Ok(json!({"kind": "solana", "slow": slow, "medium": medium, "fast": fast}))
}

/// 账户存在但不是初始化过的 nonce 账户时，对调用方来说也是没有可用的 nonce。
fn nonce_account(value: &Value) -> Result<Value, Failed> {
    let program = value.pointer("/data/program").and_then(Value::as_str);
    let kind = value.pointer("/data/parsed/type").and_then(Value::as_str);
    if program != Some("nonce") || kind != Some("initialized") {
        return Ok(json!({"exists": false}));
    }
    let nonce = value
        .pointer("/data/parsed/info/blockhash")
        .and_then(Value::as_str)
        .ok_or(Failed)?;
    let lamports = value.get("lamports").and_then(Value::as_u64).ok_or(Failed)?;
    Ok(json!({"exists": true, "nonce": nonce, "lamports": lamports.to_string()}))
}

fn is_already_processed(message: &str, data: &Value) -> bool {
    message.to_lowercase().contains("already been processed")
        || data.get("err").and_then(Value::as_str) == Some("AlreadyProcessed")
}

/// 一笔交易给这批地址带来的净变化，和对手方。
struct Change<'a> {
    owner: &'a str,
    delta: i128,
    asset: &'a str,
}

/// 与变化方向相反、绝对值最大的账户就是对手方。多方交易里这只是近似值。
fn counterparty<'a>(deltas: &'a [(&'a str, i128)], delta: i128) -> Option<&'a str> {
    deltas
        .iter()
        .filter(|(_, other)| other.signum() == -delta.signum())
        .max_by_key(|(_, other)| other.unsigned_abs())
        .map(|(account, _)| *account)
}

fn item(signature: &str, time: Option<u64>, change: &Change, deltas: &[(&str, i128)], status: TxStatus) -> HistoryItem {
    let other = counterparty(deltas, change.delta).map(str::to_string);
    let owner = Some(change.owner.to_string());
    let (from, to) = if change.delta > 0 {
        (other, owner)
    } else {
        (owner, other)
    };
    HistoryItem {
        hash: signature.to_string(),
        time,
        from,
        to,
        asset: change.asset.to_string(),
        amount: change.delta.unsigned_abs().to_string(),
        status,
    }
}

fn account_keys(tx: &Value) -> Result<Vec<&str>, Failed> {
    tx.pointer("/transaction/message/accountKeys")
        .and_then(Value::as_array)
        .ok_or(Failed)?
        .iter()
        .map(|key| key.get("pubkey").and_then(Value::as_str).ok_or(Failed))
        .collect()
}

fn lamports(meta: &Value, key: &str) -> Result<Vec<i128>, Failed> {
    meta.get(key)
        .and_then(Value::as_array)
        .ok_or(Failed)?
        .iter()
        .map(|value| value.as_u64().map(i128::from).ok_or(Failed))
        .collect()
}

/// 所有者和代币维度的余额变化。
fn token_deltas(meta: &Value) -> Result<HashMap<(&str, &str), i128>, Failed> {
    let mut deltas = HashMap::new();
    for (key, sign) in [("preTokenBalances", -1), ("postTokenBalances", 1)] {
        // 老交易没有这两个字段。
        let Some(rows) = meta.get(key).and_then(Value::as_array) else {
            continue;
        };
        for row in rows {
            let Some(owner) = row.get("owner").and_then(Value::as_str) else {
                continue;
            };
            let mint = row.get("mint").and_then(Value::as_str).ok_or(Failed)?;
            let amount = row
                .pointer("/uiTokenAmount/amount")
                .and_then(Value::as_str)
                .ok_or(Failed)?;
            let amount: i128 = amount.parse().map_err(|_| Failed)?;
            *deltas.entry((owner, mint)).or_insert(0) += sign * amount;
        }
    }
    Ok(deltas)
}

/// 用前后余额差还原一笔交易对这批地址的影响，这样内部指令和新版程序都能覆盖。
/// 失败的交易只有手续费支付方有记录，金额为 0，否则用户看不到这笔扣费。
fn history_items(
    signature: &str,
    tx: &Value,
    own: &HashSet<&str>,
    tokens: &[String],
    finalized: bool,
) -> Result<Vec<HistoryItem>, Failed> {
    let time = tx.get("blockTime").and_then(Value::as_u64);
    let meta = tx.get("meta").filter(|meta| meta.is_object()).ok_or(Failed)?;
    let failed = !meta.get("err").ok_or(Failed)?.is_null();
    let status = match (failed, finalized) {
        (true, _) => TxStatus::Failed,
        (false, true) => TxStatus::Finalized,
        (false, false) => TxStatus::Included,
    };
    let keys = account_keys(tx)?;
    let fee = i128::from(meta.get("fee").and_then(Value::as_u64).ok_or(Failed)?);
    let (pre, post) = (lamports(meta, "preBalances")?, lamports(meta, "postBalances")?);
    if keys.len() != pre.len() || keys.len() != post.len() {
        return Err(Failed);
    }

    let native: Vec<(&str, i128)> = keys
        .iter()
        .zip(pre.iter().zip(&post))
        .enumerate()
        .map(|(index, (key, (before, after)))| (*key, after - before + if index == 0 { fee } else { 0 }))
        .filter(|(_, delta)| *delta != 0)
        .collect();

    let mut items = Vec::new();
    if failed {
        if let Some(payer) = keys.first().filter(|payer| own.contains(**payer)) {
            let change = Change {
                owner: payer,
                delta: 0,
                asset: NATIVE,
            };
            items.push(item(signature, time, &change, &[], status));
        }
        return Ok(items);
    }
    for (owner, delta) in native.iter().filter(|(owner, _)| own.contains(owner)) {
        let change = Change {
            owner,
            delta: *delta,
            asset: NATIVE,
        };
        items.push(item(signature, time, &change, &native, status));
    }
    let deltas = token_deltas(meta)?;
    for mint in tokens {
        let holders: Vec<(&str, i128)> = deltas
            .iter()
            .filter(|((_, token), delta)| *token == mint && **delta != 0)
            .map(|((owner, _), delta)| (*owner, *delta))
            .collect();
        for (owner, delta) in holders.iter().filter(|(owner, _)| own.contains(owner)) {
            let change = Change {
                owner,
                delta: *delta,
                asset: mint,
            };
            items.push(item(signature, time, &change, &holders, status));
        }
    }
    Ok(items)
}

/// 一页里的一个签名。
struct SignatureRow {
    signature: String,
    finalized: bool,
}

impl Solana {
    /// 带 `context` 和 `value` 的方法。
    async fn rpc(&self, method: &str, params: Value) -> Result<Value, Failed> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        self.http
            .post(&body)
            .await?
            .pointer("/result/value")
            .cloned()
            .ok_or(Failed)
    }

    async fn result(&self, method: &str, params: Value) -> Result<Value, Failed> {
        self.http.rpc(method, params).await?.map_err(|_| Failed)
    }

    /// 一个地址在某个代币下的总额。同一代币可以有多个代币账户，要加总。
    async fn token(&self, address: &str, mint: &str) -> Result<u128, Failed> {
        let params = json!([address, {"mint": mint}, {"encoding": "jsonParsed", "commitment": "confirmed"}]);
        let accounts = self.rpc("getTokenAccountsByOwner", params).await?;
        let mut total = 0u128;
        for account in accounts.as_array().ok_or(Failed)? {
            let info = account.pointer("/account/data/parsed/info").ok_or(Failed)?;
            if info.get("mint").and_then(Value::as_str).ok_or(Failed)? != mint {
                continue;
            }
            let amount = info
                .pointer("/tokenAmount/amount")
                .and_then(Value::as_str)
                .ok_or(Failed)?;
            total += u128::from(amount.parse::<u64>().map_err(|_| Failed)?);
        }
        Ok(total)
    }

    async fn address(&self, address: &str) -> Result<Vec<Balance>, Failed> {
        let params = json!([address, {"commitment": "confirmed"}]);
        let lamports = self.rpc("getBalance", params).await?.as_u64().ok_or(Failed)?;
        let mut out = vec![balance(address, NATIVE, lamports.to_string())];
        for mint in &self.tokens {
            out.push(balance(address, mint, self.token(address, mint).await?.to_string()));
        }
        Ok(out)
    }

    /// 手续费只用于展示。getTransaction 对 processed 不可用，公共节点也常常限流，
    /// 取不到时留空，不影响状态。
    async fn fee(&self, hash: &str) -> Option<String> {
        let params = json!([hash, {
            "encoding": "json",
            "commitment": "confirmed",
            "maxSupportedTransactionVersion": MAX_TX_VERSION,
        }]);
        let reply = self.result("getTransaction", params).await.ok()?;
        reply
            .pointer("/meta/fee")
            .and_then(Value::as_u64)
            .map(|fee| fee.to_string())
    }

    async fn tx(&self, hash: &str, value: &Value, slot: u64) -> Result<Vec<Tx>, Failed> {
        let visible = !value.is_null() && value.get("confirmationStatus").and_then(Value::as_str) != Some("processed");
        let fee = if visible { self.fee(hash).await } else { None };
        Ok(vec![parse_tx(hash, value, slot, fee)?])
    }

    /// 代币转入不会出现在所有者地址的签名列表里，要单独查它的代币账户。
    pub(super) async fn token_accounts(&self, index: usize, address: &str) -> Result<Vec<String>, Failed> {
        let mut keys = Vec::new();
        for mint in &self.tokens {
            let params = json!([address, {"mint": mint}, {"encoding": "jsonParsed", "commitment": "confirmed"}]);
            let accounts = self.rpc("getTokenAccountsByOwner", params).await?;
            for account in accounts.as_array().ok_or(Failed)? {
                let pubkey = account.get("pubkey").and_then(Value::as_str).ok_or(Failed)?;
                keys.push(format!("{index}:{pubkey}"));
            }
        }
        Ok(keys)
    }

    async fn signatures(
        &self,
        key: String,
        before: Option<String>,
        addresses: &[String],
        since: Option<u64>,
    ) -> Result<Vec<(StreamPage, Vec<SignatureRow>)>, Failed> {
        let address = stream_index(&key)
            .and_then(|index| addresses.get(index))
            .ok_or(Failed)?;
        let target = key.split(':').nth(1).unwrap_or(address);
        let mut options = json!({"limit": PAGE_SIZE, "commitment": "confirmed"});
        if let Some(before) = before {
            options["before"] = json!(before);
        }
        let reply = self.result("getSignaturesForAddress", json!([target, options])).await?;
        let rows = reply.as_array().ok_or(Failed)?;
        // 新的在前，碰到水位之前的签名就不用再往后翻，也不用逐笔取交易详情。
        let slot = |row: &Value| row.get("slot").and_then(Value::as_u64);
        let fresh: Vec<&Value> = rows
            .iter()
            .filter(|row| since.is_none_or(|since| slot(row).is_none_or(|slot| slot > since)))
            .collect();
        let signatures = fresh
            .iter()
            .map(|row| {
                Ok(SignatureRow {
                    signature: row.get("signature").and_then(Value::as_str).ok_or(Failed)?.to_string(),
                    finalized: row.get("confirmationStatus").and_then(Value::as_str) == Some("finalized"),
                })
            })
            .collect::<Result<Vec<_>, Failed>>()?;
        let next = (fresh.len() == rows.len() && rows.len() >= PAGE_SIZE)
            .then(|| signatures.last().map(|row| row.signature.clone()))
            .flatten();
        Ok(vec![(
            StreamPage {
                key,
                next,
                items: Vec::new(),
            },
            signatures,
        )])
    }

    async fn transfers(&self, row: SignatureRow, own: &HashSet<&str>) -> Result<Vec<HistoryItem>, Failed> {
        let params = json!([row.signature, {
            "encoding": "jsonParsed",
            "commitment": "confirmed",
            "maxSupportedTransactionVersion": MAX_TX_VERSION,
        }]);
        let tx = self.result("getTransaction", params).await?;
        history_items(&row.signature, &tx, own, &self.tokens, row.finalized)
    }

    async fn page(&self, addresses: &[String], cursor: &Cursor, since: Option<u64>) -> Result<HistoryPage, Failed> {
        let streams: Vec<(String, Option<String>)> = if cursor.is_empty() {
            let mut keys: Vec<String> = (0..addresses.len()).map(|index| index.to_string()).collect();
            let tokens = addresses
                .iter()
                .enumerate()
                .map(|(index, address)| self.token_accounts(index, address));
            keys.extend(run_all(tokens.collect()).await?);
            keys.into_iter().map(|key| (key, None)).collect()
        } else {
            cursor
                .iter()
                .map(|(key, token)| (key.clone(), Some(token.clone())))
                .collect()
        };
        let queries = streams
            .into_iter()
            .map(|(key, before)| self.signatures(key, before, addresses, since))
            .collect();
        let pages = run_all(queries).await?;

        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        let mut stream_pages = Vec::new();
        for (page, signatures) in pages {
            stream_pages.push(page);
            rows.extend(signatures.into_iter().filter(|row| seen.insert(row.signature.clone())));
        }
        let own: HashSet<&str> = addresses.iter().map(String::as_str).collect();
        let own = &own;
        let items = run_all(rows.into_iter().map(|row| self.transfers(row, own)).collect()).await?;
        stream_pages.push(StreamPage {
            key: String::new(),
            next: None,
            items,
        });
        Ok(merge_pages(stream_pages))
    }
}

impl Upstream for Solana {
    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>> {
        Box::pin(run_all(addresses.iter().map(|a| self.address(a)).collect()))
    }

    fn height(&self) -> Query<'_, u64> {
        Box::pin(async {
            self.result("getSlot", json!([{"commitment": "confirmed"}]))
                .await?
                .as_u64()
                .ok_or(Failed)
        })
    }

    fn fees(&self) -> Query<'_, Value> {
        Box::pin(async { fee_tiers(&self.result("getRecentPrioritizationFees", json!([])).await?) })
    }

    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>> {
        Box::pin(async move {
            let params = json!([hashes, {"searchTransactionHistory": true}]);
            let reply = self.result("getSignatureStatuses", params).await?;
            let slot = reply.pointer("/context/slot").and_then(Value::as_u64).ok_or(Failed)?;
            let values = reply.get("value").and_then(Value::as_array).ok_or(Failed)?;
            if values.len() != hashes.len() {
                return Err(Failed);
            }
            run_all(
                hashes
                    .iter()
                    .zip(values)
                    .map(|(hash, value)| self.tx(hash, value, slot))
                    .collect(),
            )
            .await
        })
    }

    fn watch(&self, addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.follow(addresses, signals).await;
        })
    }

    fn recent<'a>(&'a self, addresses: &'a [String], previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(async move {
            // 推送比签名列表略快，往回多看一段，重复的记录由客户端按标识合并。
            let since = previous.saturating_sub(RECENT_SLOTS);
            let cursor = Cursor::new();
            Ok(self.page(addresses, &cursor, Some(since)).await?.items)
        })
    }

    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(async move {
            let params = json!([{"commitment": "finalized"}]);
            let checkpoint = self.result("getSlot", params).await?.as_u64().ok_or(Failed)?;
            let page = self.page(addresses, cursor, since).await?;
            Ok(HistoryPage { checkpoint, ..page })
        })
    }

    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast> {
        Box::pin(async move {
            // durable nonce 刚推进时用 confirmed 才模拟得过，默认的 finalized 看到的是旧值。
            let options = json!({"encoding": "base64", "preflightCommitment": "confirmed"});
            match self
                .http
                .rpc_send("sendTransaction", json!([raw.base64(), options]))
                .await
            {
                Ok(Ok(_)) => Broadcast::Accepted,
                Ok(Err(error)) if is_already_processed(&error.message, &error.data) => Broadcast::Accepted,
                Ok(Err(error)) if error.is_rejection() => rejected(self, raw, error.message).await,
                Ok(Err(_)) => Broadcast::Unknown,
                Err(outcome) => outcome,
            }
        })
    }
}

impl Solana {
    pub fn estimate<'a>(&'a self, recipient: &'a str, mint: Option<&'a str>) -> Query<'a, Value> {
        Box::pin(async move {
            let Some(mint) = mint else {
                return Ok(json!({"recipientTokenAccount": true, "rent": "0"}));
            };
            let options = json!({"encoding": "jsonParsed", "commitment": "confirmed"});
            let accounts = self
                .rpc("getTokenAccountsByOwner", json!([recipient, {"mint": mint}, options]))
                .await?;
            if !accounts.as_array().ok_or(Failed)?.is_empty() {
                return Ok(json!({"recipientTokenAccount": true, "rent": "0"}));
            }
            let rent = self
                .result("getMinimumBalanceForRentExemption", json!([TOKEN_ACCOUNT_SIZE]))
                .await?;
            let rent = rent.as_u64().ok_or(Failed)?;
            Ok(json!({"recipientTokenAccount": false, "rent": rent.to_string()}))
        })
    }

    pub fn nonce_account<'a>(&'a self, address: &'a str) -> Query<'a, Value> {
        Box::pin(async move {
            // 上一笔转账推进 nonce 后要马上能读到新值，所以不用默认的 finalized。
            let params = json!([address, {"encoding": "jsonParsed", "commitment": "confirmed"}]);
            nonce_account(&self.rpc("getAccountInfo", params).await?)
        })
    }

    pub fn blockhash(&self) -> Query<'_, Value> {
        Box::pin(async {
            let value = self
                .rpc("getLatestBlockhash", json!([{"commitment": "confirmed"}]))
                .await?;
            let blockhash = value.get("blockhash").and_then(Value::as_str).ok_or(Failed)?;
            Ok(json!({"blockhash": blockhash}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "owner";
    const OTHER: &str = "other";
    const MINT: &str = "mint";

    fn status(err: Value, level: &str, confirmations: Value) -> Value {
        json!({"slot": 90, "confirmations": confirmations, "err": err, "confirmationStatus": level})
    }

    #[test]
    fn commitment_sets_the_stage() {
        for (level, expected) in [
            ("processed", TxStatus::Included),
            ("confirmed", TxStatus::Included),
            ("finalized", TxStatus::Finalized),
        ] {
            let tx = parse_tx("h", &status(Value::Null, level, json!(3)), 100, None).unwrap();
            assert_eq!((tx.status, tx.stage), (expected, Some(level)));
        }
    }

    #[test]
    fn err_is_failed_at_every_commitment() {
        let err = json!({"InstructionError": [1, {"Custom": 6001}]});
        for level in ["processed", "confirmed", "finalized"] {
            let tx = parse_tx("h", &status(err.clone(), level, json!(null)), 100, Some("5000".into())).unwrap();
            assert_eq!((tx.status, tx.stage), (TxStatus::Failed, Some(level)), "{level}");
            assert_eq!(tx.fee.as_deref(), Some("5000"));
        }
    }

    #[test]
    fn unknown_signature_has_no_block() {
        let tx = parse_tx("h", &Value::Null, 100, None).unwrap();
        assert_eq!((tx.status, tx.block), (TxStatus::Unknown, None));
    }

    #[test]
    fn finalized_entries_have_null_confirmations() {
        let tx = parse_tx("h", &status(Value::Null, "finalized", Value::Null), 100, None).unwrap();
        assert_eq!((tx.confirmations, tx.block), (10, Some(90)));
    }

    #[test]
    fn a_missing_err_field_is_never_a_success() {
        let mut bad = status(Value::Null, "finalized", json!(1));
        bad.as_object_mut().unwrap().remove("err");
        assert!(parse_tx("h", &bad, 100, None).is_err());
        assert!(parse_tx("h", &status(Value::Null, "rooted", json!(1)), 100, None).is_err());
        assert!(parse_tx("h", &json!({"err": null, "confirmationStatus": "finalized"}), 100, None).is_err());
    }

    #[test]
    fn fee_tiers_have_floors_for_a_quiet_network() {
        let rows: Vec<Value> = (0..150).map(|_| json!({"slot": 1, "prioritizationFee": 0})).collect();
        assert_eq!(
            fee_tiers(&json!(rows)).unwrap(),
            json!({"kind": "solana", "slow": 0, "medium": 1000, "fast": 100000}),
        );
        assert_eq!(fee_tiers(&json!([])).unwrap()["fast"], 100000);
    }

    #[test]
    fn fee_tiers_follow_percentiles_above_the_floors() {
        let rows: Vec<Value> = (0..=100).map(|i| json!({"prioritizationFee": i * 10_000})).collect();
        assert_eq!(
            fee_tiers(&json!(rows)).unwrap(),
            json!({"kind": "solana", "slow": 250000, "medium": 500000, "fast": 900000}),
        );
        assert!(fee_tiers(&json!({})).is_err());
        assert!(fee_tiers(&json!([{"slot": 1}])).is_err());
    }

    #[test]
    fn nonce_accounts_must_be_initialized() {
        let account = |kind: &str, program: &str| {
            json!({"lamports": 1447680, "owner": "11111111111111111111111111111111", "data": {
                "program": program,
                "space": 80,
                "parsed": {
                    "type": kind,
                    "info": {"authority": "a", "blockhash": "nonce", "feeCalculator": {"lamportsPerSignature": "5000"}},
                },
            }})
        };
        assert_eq!(
            nonce_account(&account("initialized", "nonce")).unwrap(),
            json!({"exists": true, "nonce": "nonce", "lamports": "1447680"}),
        );
        assert_eq!(
            nonce_account(&account("uninitialized", "nonce")).unwrap(),
            json!({"exists": false})
        );
        assert_eq!(
            nonce_account(&account("initialized", "spl-token")).unwrap(),
            json!({"exists": false})
        );
        assert_eq!(nonce_account(&Value::Null).unwrap(), json!({"exists": false}));
        // 普通系统账户。
        assert_eq!(
            nonce_account(&json!({"data": ["", "base64"], "lamports": 1})).unwrap(),
            json!({"exists": false})
        );
    }

    #[test]
    fn already_processed_covers_message_and_data() {
        assert!(is_already_processed(
            "Transaction simulation failed: This transaction has already been processed",
            &Value::Null
        ));
        assert!(is_already_processed("x", &json!({"err": "AlreadyProcessed"})));
        assert!(!is_already_processed(
            "Blockhash not found",
            &json!({"err": "BlockhashNotFound"})
        ));
    }

    fn sample(err: Value, keys: &[&str], pre: &[u64], post: &[u64], tokens: (Value, Value)) -> Value {
        let keys: Vec<Value> = keys.iter().map(|key| json!({"pubkey": key})).collect();
        json!({
            "blockTime": 1700000000,
            "transaction": {"message": {"accountKeys": keys}},
            "meta": {
                "err": err, "fee": 5000, "preBalances": pre, "postBalances": post,
                "preTokenBalances": tokens.0, "postTokenBalances": tokens.1,
            },
        })
    }

    fn no_tokens() -> (Value, Value) {
        (json!([]), json!([]))
    }

    #[test]
    fn native_transfer_adds_the_fee_back() {
        let own = HashSet::from([OWNER]);
        let tx = sample(
            Value::Null,
            &[OWNER, OTHER],
            &[1_000_000, 0],
            &[894_000, 100_000 + 1_000],
            no_tokens(),
        );
        let items = history_items("s", &tx, &own, &[], true).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!((item.from.as_deref(), item.to.as_deref()), (Some(OWNER), Some(OTHER)));
        assert_eq!((item.amount.as_str(), item.status), ("101000", TxStatus::Finalized));
        assert_eq!(item.time, Some(1700000000));
    }

    #[test]
    fn incoming_native_transfer_points_at_the_sender() {
        let own = HashSet::from([OWNER]);
        let tx = sample(
            Value::Null,
            &[OTHER, OWNER],
            &[1_000_000, 0],
            &[895_000, 100_000],
            no_tokens(),
        );
        let items = history_items("s", &tx, &own, &[], false).unwrap();
        assert_eq!(
            (items[0].from.as_deref(), items[0].to.as_deref()),
            (Some(OTHER), Some(OWNER))
        );
        assert_eq!(items[0].status, TxStatus::Included);
    }

    #[test]
    fn token_changes_are_tracked_by_owner_and_mint() {
        let row = |owner: &str, mint: &str, amount: &str| json!({"accountIndex": 1, "owner": owner, "mint": mint, "uiTokenAmount": {"amount": amount}});
        let tokens = (
            json!([row(OWNER, MINT, "500"), row(OTHER, MINT, "0"), row(OWNER, "spam", "9")]),
            json!([
                row(OWNER, MINT, "200"),
                row(OTHER, MINT, "300"),
                row(OWNER, "spam", "1")
            ]),
        );
        let own = HashSet::from([OWNER]);
        let tx = sample(Value::Null, &[OWNER, OTHER], &[1_000_000, 0], &[995_000, 0], tokens);
        let items = history_items("s", &tx, &own, &[MINT.to_string()], true).unwrap();
        assert_eq!(items.len(), 1, "清单外的代币不返回，手续费不算转账");
        assert_eq!(items[0].asset, MINT);
        assert_eq!(
            (items[0].from.as_deref(), items[0].to.as_deref()),
            (Some(OWNER), Some(OTHER))
        );
        assert_eq!(items[0].amount, "300");
    }

    #[test]
    fn failed_transactions_show_only_for_the_fee_payer() {
        let err = json!({"InstructionError": [0, "Custom"]});
        let tx = sample(err, &[OWNER, OTHER], &[1_000_000, 0], &[995_000, 0], no_tokens());
        let items = history_items("s", &tx, &HashSet::from([OWNER]), &[], true).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].status, items[0].amount.as_str()), (TxStatus::Failed, "0"));
        assert!(
            history_items("s", &tx, &HashSet::from([OTHER]), &[], true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn mismatched_balance_lists_are_an_upstream_failure() {
        let tx = sample(Value::Null, &[OWNER], &[1, 2], &[1, 2], no_tokens());
        assert!(history_items("s", &tx, &HashSet::new(), &[], true).is_err());
        assert!(history_items("s", &Value::Null, &HashSet::new(), &[], true).is_err());
        assert!(history_items("s", &json!({"meta": null}), &HashSet::new(), &[], true).is_err());
    }
}

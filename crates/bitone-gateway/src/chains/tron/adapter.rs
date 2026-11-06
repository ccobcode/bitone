//! TRON 的上游适配器.

use crate::upstream::{
    Balance, Broadcast, Cursor, Failed, HistoryItem, HistoryPage, Http, NATIVE, Query, RawTx, Signal, StreamPage, Tx,
    TxStatus, Upstream, balance, decimal, merge_pages, read_json, rejected, run_all, stream_index, streams,
};
use bitcoin::base58;
use bitcoin::hex::FromHex;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::{mpsc, watch};

const SUCCESS: &str = "SUCCESS";
/// 交易已经被节点收到.
const DUPLICATE: &str = "DUP_TRANSACTION_ERROR";
/// 这些返回码说的是节点状态,不是这笔交易不合法.
const NODE_BUSY: [&str; 4] = [
    "SERVER_BUSY",
    "NO_CONNECTION",
    "NOT_ENOUGH_EFFECTIVE_CONNECTION",
    "BLOCK_UNSOLIDIFIED",
];
/// 普通转账的带宽.
const TRANSFER_BANDWIDTH: u64 = 268;
/// 合约调用在 raw_data 之外的带宽:`fee_limit` 6 字节,外层长度前缀 3 字节,
/// 签名 67 字节,结果预留 64 字节.按主网 USDT 转账的 345 校准.
const CALL_OVERHEAD: u64 = 140;
const PAGE_SIZE: &str = "50";
/// 原生币记录流的键,代币流用清单下标.
const NATIVE_STREAM: &str = "native";

pub struct Tron {
    pub http: Http,
    pub tokens: Vec<String>,
}

/// 估算费用的请求,字段都已校验.
pub struct Estimate {
    pub from: String,
    pub to: String,
    pub contract: Option<String>,
    pub data: Option<String>,
}

/// 查不到时节点返回空对象.
fn found(reply: &Value) -> bool {
    reply.as_object().is_some_and(|fields| !fields.is_empty())
}

fn block_number(block: &Value) -> Result<u64, Failed> {
    block
        .pointer("/block_header/raw_data/number")
        .and_then(Value::as_u64)
        .ok_or(Failed)
}

/// `gettransactioninfobyid` 和 `gettransactionbyid` 映射成交易状态.
/// 合约结果不是 SUCCESS 就是失败,普通转账也有这个字段.
fn parse_tx(hash: &str, info: &Value, tx: &Value, confirmed: bool, tip: u64) -> Result<Tx, Failed> {
    let block = info.get("blockNumber").and_then(Value::as_u64).ok_or(Failed)?;
    let result = tx.pointer("/ret/0/contractRet").and_then(Value::as_str).ok_or(Failed)?;
    let failed = result != SUCCESS || info.get("result").and_then(Value::as_str) == Some("FAILED");
    // 手续费为 0 时没有这个字段.
    let fee = info.get("fee").map_or(Ok("0".to_string()), decimal)?;
    let status = match (failed, confirmed) {
        (true, _) => TxStatus::Failed,
        (false, true) => TxStatus::Finalized,
        (false, false) => TxStatus::Included,
    };
    Ok(Tx {
        hash: hash.to_string(),
        status,
        confirmations: tip.saturating_sub(block) + 1,
        stage: Some(if confirmed { "finalized" } else { "included" }),
        block: Some(block),
        fee: Some(fee),
    })
}

/// 引用区块.取高度的低 2 字节和区块 ID 的第 8 到 16 字节.
fn ref_block(block: &Value) -> Result<Value, Failed> {
    let number = block_number(block)?;
    let id = block
        .get("blockID")
        .and_then(Value::as_str)
        .filter(|id| id.len() == 64)
        .ok_or(Failed)?;
    // 区块 ID 的前 8 字节就是高度,对不上说明数据有问题.
    if u64::from_str_radix(id.get(..16).ok_or(Failed)?, 16).map_err(|_| Failed)? != number {
        return Err(Failed);
    }
    let timestamp = block
        .pointer("/block_header/raw_data/timestamp")
        .and_then(Value::as_u64)
        .ok_or(Failed)?;
    Ok(json!({
        "refBlockBytes": format!("{:04x}", number & 0xffff),
        "refBlockHash": id.get(16..32).ok_or(Failed)?,
        "timestamp": timestamp,
    }))
}

/// 节点的 JSON 省略值为 0 的字段,所以键在而值缺失就是 0,键不在才是异常.
fn chain_parameter(reply: &Value, key: &str) -> Result<u64, Failed> {
    let parameters = reply.get("chainParameter").and_then(Value::as_array).ok_or(Failed)?;
    let parameter = parameters
        .iter()
        .find(|p| p.get("key").and_then(Value::as_str) == Some(key))
        .ok_or(Failed)?;
    parameter
        .get("value")
        .map_or(Ok(0), |value| value.as_u64().ok_or(Failed))
}

fn resource(reply: &Value, account: &Value) -> Value {
    // 用量为 0 时没有这些字段.
    let field = |key: &str| reply.get(key).and_then(Value::as_u64).unwrap_or(0);
    let free = field("freeNetLimit").saturating_sub(field("freeNetUsed"));
    let staked = field("NetLimit").saturating_sub(field("NetUsed"));
    let energy = field("EnergyLimit").saturating_sub(field("EnergyUsed"));
    json!({"bandwidth": free + staked, "energy": energy, "activated": found(account)})
}

/// 合约调用的模拟结果.回滚时 `result.result` 仍然是 true,要看 `transaction.ret` 和 `message`.
fn simulated(reply: &Value) -> Result<Result<Value, String>, Failed> {
    let result = reply.get("result").ok_or(Failed)?;
    let message = result.get("message").and_then(Value::as_str).unwrap_or("调用失败");
    let succeeded = result.get("result").and_then(Value::as_bool) == Some(true);
    let reverted = reply.pointer("/transaction/ret/0/ret").and_then(Value::as_str) == Some("FAILED");
    if !succeeded || reverted {
        return Ok(Err(message.to_string()));
    }
    let energy = reply.get("energy_used").and_then(Value::as_u64).ok_or(Failed)?;
    let raw = reply
        .pointer("/transaction/raw_data_hex")
        .and_then(Value::as_str)
        .ok_or(Failed)?;
    let bandwidth = raw.len() as u64 / 2 + CALL_OVERHEAD;
    Ok(Ok(
        json!({"energy": energy.to_string(), "bandwidth": bandwidth.to_string(), "activationFee": "0"}),
    ))
}

fn base58_address(hex: &str) -> Result<String, Failed> {
    let bytes = Vec::<u8>::from_hex(hex).map_err(|_| Failed)?;
    if bytes.len() != 21 || bytes[0] != 0x41 {
        return Err(Failed);
    }
    Ok(base58::encode_check(&bytes))
}

/// `/v1/accounts/{address}/transactions` 的一行.只有两种记录:普通转账,以及失败的合约调用.
/// 成功的代币转账在代币流里,其余系统合约不是转账.
fn native_item(row: &Value) -> Result<Option<HistoryItem>, Failed> {
    let contract = row.pointer("/raw_data/contract/0").ok_or(Failed)?;
    let kind = contract.get("type").and_then(Value::as_str).ok_or(Failed)?;
    let value = contract.pointer("/parameter/value").ok_or(Failed)?;
    let result = row
        .pointer("/ret/0/contractRet")
        .and_then(Value::as_str)
        .ok_or(Failed)?;
    let failed = result != SUCCESS;
    let (to_key, amount_key) = match (kind, failed) {
        ("TransferContract", _) => ("to_address", "amount"),
        ("TriggerSmartContract", true) => ("contract_address", "call_value"),
        _ => return Ok(None),
    };
    let address = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .ok_or(Failed)
            .and_then(base58_address)
    };
    Ok(Some(HistoryItem {
        hash: row.get("txID").and_then(Value::as_str).ok_or(Failed)?.to_string(),
        time: Some(row.get("block_timestamp").and_then(Value::as_u64).ok_or(Failed)? / 1000),
        from: Some(address("owner_address")?),
        to: Some(address(to_key)?),
        asset: NATIVE.to_string(),
        amount: value.get(amount_key).map_or(Ok("0".to_string()), decimal)?,
        status: if failed { TxStatus::Failed } else { TxStatus::Finalized },
        flows: None,
    }))
}

/// `/v1/accounts/{address}/transactions/trc20` 的一行.这个接口只有成功的 Transfer 事件.
fn token_item(row: &Value, token: &str) -> Result<Option<HistoryItem>, Failed> {
    if row.get("type").and_then(Value::as_str) != Some("Transfer") {
        return Ok(None);
    }
    let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string).ok_or(Failed);
    Ok(Some(HistoryItem {
        hash: text("transaction_id")?,
        time: Some(row.get("block_timestamp").and_then(Value::as_u64).ok_or(Failed)? / 1000),
        from: Some(text("from")?),
        to: Some(text("to")?),
        asset: token.to_string(),
        amount: decimal(row.get("value").ok_or(Failed)?)?,
        status: TxStatus::Finalized,
        flows: None,
    }))
}

/// 广播接口的响应.
fn submitted(reply: &Value) -> Result<(), Option<String>> {
    let code = reply.get("code").and_then(Value::as_str).unwrap_or_default();
    if reply.get("result").and_then(Value::as_bool) == Some(true) || code == DUPLICATE {
        return Ok(());
    }
    if code.is_empty() || NODE_BUSY.contains(&code) {
        return Err(None);
    }
    let message = reply.get("message").and_then(Value::as_str).unwrap_or_default();
    Err(Some(if message.is_empty() {
        code.to_string()
    } else {
        format!("{code}: {message}")
    }))
}

impl Tron {
    pub(super) async fn tip(&self) -> Result<u64, Failed> {
        block_number(&self.http.get("/wallet/getnowblock").await?)
    }

    async fn parameters(&self) -> Result<Value, Failed> {
        self.http.get("/wallet/getchainparameters").await
    }

    async fn account(&self, address: &str) -> Result<Value, Failed> {
        self.http
            .post_to("/wallet/getaccount", &json!({"address": address, "visible": true}))
            .await
    }

    async fn address(&self, address: &str) -> Result<Vec<Balance>, Failed> {
        let reply = self.http.get(&format!("/v1/accounts/{address}")).await?;
        // 未激活的账户 data 是空数组,这是余额为 0,不是错误.
        let account = reply.get("data").and_then(Value::as_array).ok_or(Failed)?.first();
        // 余额为 0 的账户可能没有 balance 字段.
        let native = account
            .and_then(|a| a.get("balance"))
            .map_or(Ok("0".to_string()), decimal)?;
        let trc20 = account.and_then(|a| a.get("trc20")).and_then(Value::as_array);
        let mut out = vec![balance(address, NATIVE, native)];
        for token in &self.tokens {
            let found = trc20.into_iter().flatten().find_map(|entry| entry.get(token));
            out.push(balance(address, token, found.map_or(Ok("0".to_string()), decimal)?));
        }
        Ok(out)
    }

    /// `walletsolidity` 查得到才算区块确认.查不到再问全节点,这时交易可能刚上链.
    async fn tx(&self, hash: &str, tip: u64) -> Result<Vec<Tx>, Failed> {
        let value = json!({"value": hash});
        let receipt = self
            .http
            .post_to("/walletsolidity/gettransactioninfobyid", &value)
            .await?;
        let confirmed = found(&receipt);
        let info = if confirmed {
            receipt
        } else {
            self.http.post_to("/wallet/gettransactioninfobyid", &value).await?
        };
        if !found(&info) {
            let pending = self.http.post_to("/wallet/gettransactionfrompending", &value).await?;
            return Ok(vec![if found(&pending) {
                Tx::pending(hash)
            } else {
                Tx::unknown(hash)
            }]);
        }
        let tx = self.http.post_to("/wallet/gettransactionbyid", &value).await?;
        Ok(vec![parse_tx(hash, &info, &tx, confirmed, tip)?])
    }

    async fn resource(&self, address: &String) -> Result<Vec<(String, Value)>, Failed> {
        let body = json!({"address": address, "visible": true});
        let reply = self.http.post_to("/wallet/getaccountresource", &body).await?;
        let account = self.account(address).await?;
        Ok(vec![(address.clone(), resource(&reply, &account))])
    }

    /// 普通转账不需要模拟.收款方未激活时要付激活费,两项费用都来自链参数.
    async fn estimate_transfer(&self, to: &str) -> Result<Value, Failed> {
        let fee = if found(&self.account(to).await?) {
            0
        } else {
            let parameters = self.parameters().await?;
            chain_parameter(&parameters, "getCreateNewAccountFeeInSystemContract")?
                + chain_parameter(&parameters, "getCreateAccountFee")?
        };
        Ok(json!({"energy": "0", "bandwidth": TRANSFER_BANDWIDTH.to_string(), "activationFee": fee.to_string()}))
    }

    /// 最新已确认区块的时间,单位是毫秒.
    async fn confirmed_time(&self) -> Result<u64, Failed> {
        let block = self.http.post_to("/walletsolidity/getnowblock", &json!({})).await?;
        block
            .pointer("/block_header/raw_data/timestamp")
            .and_then(Value::as_u64)
            .ok_or(Failed)
    }

    /// 一页记录.`confirmed` 时只取已确认的记录,这时的状态都是最终状态.
    async fn page(
        &self,
        addresses: &[String],
        cursor: &Cursor,
        since: Option<u64>,
        confirmed: bool,
    ) -> Result<HistoryPage, Failed> {
        let names =
            || std::iter::once(NATIVE_STREAM.to_string()).chain((0..self.tokens.len()).map(|index| index.to_string()));
        let keys = (0..addresses.len()).flat_map(|index| names().map(move |name| format!("{index}:{name}")));
        let queries = streams(keys, cursor)
            .into_iter()
            .map(|(key, token)| self.stream(key, token, addresses, since, confirmed))
            .collect();
        Ok(merge_pages(run_all(queries).await?))
    }

    async fn stream(
        &self,
        key: String,
        token: Option<String>,
        addresses: &[String],
        since: Option<u64>,
        confirmed: bool,
    ) -> Result<Vec<StreamPage>, Failed> {
        let address = stream_index(&key)
            .and_then(|index| addresses.get(index))
            .ok_or(Failed)?;
        let name = key.split(':').nth(1).ok_or(Failed)?;
        let start = since.map(|since| (since + 1).to_string());
        let mut query = vec![("limit", PAGE_SIZE)];
        if confirmed {
            query.push(("only_confirmed", "true"));
        }
        if let Some(start) = &start {
            query.push(("min_timestamp", start));
        }
        let contract = if name == NATIVE_STREAM {
            None
        } else {
            let contract = name
                .parse::<usize>()
                .ok()
                .and_then(|index| self.tokens.get(index))
                .ok_or(Failed)?;
            query.push(("contract_address", contract));
            Some(contract)
        };
        if let Some(token) = &token {
            query.push(("fingerprint", token));
        }
        let path = match contract {
            None => {
                // 不混入内部交易,它们的结构不同.
                query.push(("search_internal", "false"));
                format!("/v1/accounts/{address}/transactions")
            }
            Some(_) => format!("/v1/accounts/{address}/transactions/trc20"),
        };
        let reply = self.http.get_query(&path, &query).await?;
        let rows = reply.get("data").and_then(Value::as_array).ok_or(Failed)?;
        let items = rows
            .iter()
            .filter_map(|row| match contract {
                None => native_item(row).transpose(),
                Some(contract) => token_item(row, contract).transpose(),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next = reply
            .pointer("/meta/fingerprint")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(vec![StreamPage { key, next, items }])
    }

    async fn submit(&self, raw: &RawTx) -> Result<Broadcast, Broadcast> {
        let body = json!({"transaction": raw.hex()});
        let response = self
            .http
            .request(reqwest::Method::POST, "/wallet/broadcasthex")
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(Broadcast::Unknown);
        }
        let reply = read_json(response).await.map_err(|_| Broadcast::Unknown)?;
        Ok(match submitted(&reply) {
            Ok(()) => Broadcast::Accepted,
            Err(Some(message)) => rejected(self, raw, message).await,
            Err(None) => Broadcast::Unknown,
        })
    }
}

/// 跟踪时也取还没确认的记录,确认之前状态是 `included`.
fn unconfirmed(mut item: HistoryItem, confirmed: u64) -> HistoryItem {
    let newer = item.time.is_some_and(|time| time * 1000 > confirmed);
    if newer && item.status == TxStatus::Finalized {
        item.status = TxStatus::Included;
    }
    item
}

impl Upstream for Tron {
    fn watch(&self, addresses: watch::Receiver<Vec<String>>, signals: mpsc::Sender<Signal>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.follow(addresses, signals).await;
        })
    }

    /// 只取已确认的记录时,刚到账的转账不在推送里,要等下次活动才看得到.
    fn recent<'a>(&'a self, addresses: &'a [String], _previous: u64) -> Query<'a, Vec<HistoryItem>> {
        Box::pin(async move {
            let confirmed = self.confirmed_time().await?;
            let cursor = Cursor::new();
            let page = self.page(addresses, &cursor, None, false).await?;
            Ok(page
                .items
                .into_iter()
                .map(|item| unconfirmed(item, confirmed))
                .collect())
        })
    }

    fn balances<'a>(&'a self, addresses: &'a [String]) -> Query<'a, Vec<Balance>> {
        Box::pin(run_all(addresses.iter().map(|a| self.address(a)).collect()))
    }

    fn height(&self) -> Query<'_, u64> {
        Box::pin(self.tip())
    }

    fn fees(&self) -> Query<'_, Value> {
        Box::pin(async {
            let parameters = self.parameters().await?;
            let energy = chain_parameter(&parameters, "getEnergyFee")?;
            let bandwidth = chain_parameter(&parameters, "getTransactionFee")?;
            Ok(json!({"kind": "tron", "energyPrice": energy, "bandwidthPrice": bandwidth}))
        })
    }

    fn txs<'a>(&'a self, hashes: &'a [String]) -> Query<'a, Vec<Tx>> {
        Box::pin(async move {
            let tip = self.tip().await?;
            run_all(hashes.iter().map(|hash| self.tx(hash, tip)).collect()).await
        })
    }

    fn history<'a>(
        &'a self,
        addresses: &'a [String],
        cursor: &'a Cursor,
        since: Option<u64>,
    ) -> Query<'a, HistoryPage> {
        Box::pin(async move {
            // 这里只有已确认的记录,所以 `checkpoint` 是最新已确认区块的时间.
            let checkpoint = self.confirmed_time().await?;
            let page = self.page(addresses, cursor, since, true).await?;
            Ok(HistoryPage { checkpoint, ..page })
        })
    }

    fn broadcast<'a>(&'a self, raw: &'a RawTx) -> BoxFuture<'a, Broadcast> {
        Box::pin(async move { self.submit(raw).await.unwrap_or_else(|outcome| outcome) })
    }
}

impl Tron {
    /// 内层的 `Err` 是节点拒绝这次调用,带节点原文.
    pub fn estimate<'a>(&'a self, request: &'a Estimate) -> Query<'a, Result<Value, String>> {
        Box::pin(async move {
            let Some(contract) = &request.contract else {
                return Ok(Ok(self.estimate_transfer(&request.to).await?));
            };
            let body = json!({
                "owner_address": request.from,
                "contract_address": contract,
                "data": request.data,
                "visible": true,
            });
            simulated(&self.http.post_to("/wallet/triggerconstantcontract", &body).await?)
        })
    }

    pub fn resources<'a>(&'a self, addresses: &'a [String]) -> Query<'a, BTreeMap<String, Value>> {
        Box::pin(async move {
            let pairs = run_all(addresses.iter().map(|address| self.resource(address)).collect()).await?;
            Ok(pairs.into_iter().collect())
        })
    }

    pub fn ref_block(&self) -> Query<'_, Value> {
        Box::pin(async { ref_block(&self.http.get("/walletsolidity/getnowblock").await?) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "abc";
    const OWNER: &str = "4192689ecfbfcdffa3444961571f2b822d3967d0aa";
    const RECIPIENT: &str = "4188403ac26730e33164eeac403291b48f300b782f";

    fn info() -> Value {
        json!({"id": HASH, "blockNumber": 95, "fee": 345000, "receipt": {"result": "SUCCESS"}})
    }

    fn ret(result: &str) -> Value {
        json!({"ret": [{"contractRet": result}]})
    }

    #[test]
    fn confirmed_is_final_and_included_is_not() {
        let tx = parse_tx(HASH, &info(), &ret(SUCCESS), false, 100).unwrap();
        assert_eq!(
            (tx.status, tx.stage, tx.confirmations),
            (TxStatus::Included, Some("included"), 6)
        );
        let tx = parse_tx(HASH, &info(), &ret(SUCCESS), true, 100).unwrap();
        assert_eq!((tx.status, tx.stage), (TxStatus::Finalized, Some("finalized")));
        assert_eq!((tx.block, tx.fee.as_deref()), (Some(95), Some("345000")));
    }

    #[test]
    fn anything_but_success_is_failed_even_when_confirmed() {
        for result in [
            "REVERT",
            "OUT_OF_ENERGY",
            "OUT_OF_TIME",
            "BAD_JUMP_DESTINATION",
            "UNKNOWN",
        ] {
            for confirmed in [false, true] {
                let tx = parse_tx(HASH, &info(), &ret(result), confirmed, 100).unwrap();
                assert_eq!(tx.status, TxStatus::Failed, "{result}");
            }
        }
    }

    #[test]
    fn a_failed_info_result_wins_over_a_success_flag() {
        let mut info = info();
        info["result"] = json!("FAILED");
        assert_eq!(
            parse_tx(HASH, &info, &ret(SUCCESS), true, 100).unwrap().status,
            TxStatus::Failed
        );
    }

    #[test]
    fn a_missing_result_is_never_a_success() {
        assert!(parse_tx(HASH, &info(), &json!({}), true, 100).is_err());
        assert!(parse_tx(HASH, &info(), &json!({"ret": [{}]}), true, 100).is_err());
    }

    #[test]
    fn free_transfers_have_no_fee_field() {
        let tx = parse_tx(HASH, &json!({"blockNumber": 95}), &ret(SUCCESS), true, 100).unwrap();
        assert_eq!(tx.fee.as_deref(), Some("0"));
    }

    #[test]
    fn ref_block_takes_the_tapos_bytes() {
        let block = json!({
            "blockID": "000000000538151a267822368879ebe44f6ee6fa943f92ec1f1539cfad703dd9",
            "block_header": {"raw_data": {"number": 87561498, "timestamp": 1793357889000u64}},
        });
        assert_eq!(
            ref_block(&block).unwrap(),
            json!({"refBlockBytes": "151a", "refBlockHash": "267822368879ebe4", "timestamp": 1793357889000u64}),
        );
    }

    #[test]
    fn ref_block_rejects_an_id_from_another_height() {
        let block = |id: &str| json!({"blockID": id, "block_header": {"raw_data": {"number": 7, "timestamp": 1}}});
        assert!(ref_block(&block(&format!("{:0>16}{}", "8", "ab".repeat(24)))).is_err());
        assert!(ref_block(&block("short")).is_err());
        assert!(ref_block(&json!({})).is_err());
    }

    #[test]
    fn resources_add_free_and_staked_bandwidth() {
        let reply = json!({"freeNetLimit": 600, "freeNetUsed": 100, "NetLimit": 50, "NetUsed": 10, "EnergyLimit": 90, "EnergyUsed": 30});
        assert_eq!(
            resource(&reply, &json!({"address": "T"})),
            json!({"bandwidth": 540, "energy": 60, "activated": true}),
        );
        assert_eq!(
            resource(&json!({}), &json!({})),
            json!({"bandwidth": 0, "energy": 0, "activated": false})
        );
    }

    #[test]
    fn a_revert_reports_success_in_result_but_is_rejected() {
        let reply = json!({
            "result": {"result": true, "message": "REVERT opcode executed"},
            "energy_used": 8624,
            "transaction": {"ret": [{"ret": "FAILED"}], "raw_data_hex": "00"},
        });
        assert_eq!(simulated(&reply).unwrap().unwrap_err(), "REVERT opcode executed");
    }

    #[test]
    fn a_validation_error_is_rejected() {
        let reply = json!({"result": {"code": "CONTRACT_VALIDATE_ERROR", "message": "Smart contract is not exist."}});
        assert_eq!(simulated(&reply).unwrap().unwrap_err(), "Smart contract is not exist.");
    }

    #[test]
    fn a_simulation_reports_energy_and_bandwidth() {
        let reply = json!({
            "result": {"result": true},
            "energy_used": 64285,
            "transaction": {"ret": [{}], "raw_data_hex": "ab".repeat(205)},
        });
        assert_eq!(
            simulated(&reply).unwrap().unwrap(),
            json!({"energy": "64285", "bandwidth": "345", "activationFee": "0"}),
        );
        assert!(simulated(&json!({})).is_err());
        assert!(simulated(&json!({"result": {"result": true}})).is_err());
    }

    #[test]
    fn hex_addresses_become_base58() {
        assert_eq!(
            base58_address("41a614f803b6fd780986a42c78ec9c7f77e6ded13c").unwrap(),
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"
        );
        for bad in ["", "41", "zz", OWNER.get(2..).unwrap()] {
            assert!(base58_address(bad).is_err(), "{bad}");
        }
    }

    fn transfer(result: &str) -> Value {
        json!({
            "txID": HASH,
            "block_timestamp": 1791436788000u64,
            "ret": [{"contractRet": result}],
            "raw_data": {"contract": [{
                "type": "TransferContract",
                "parameter": {"value": {"amount": 2, "owner_address": OWNER, "to_address": RECIPIENT}},
            }]},
        })
    }

    #[test]
    fn transfers_are_finalized_or_failed() {
        let item = native_item(&transfer(SUCCESS)).unwrap().unwrap();
        assert_eq!(
            (item.status, item.amount.as_str(), item.time),
            (TxStatus::Finalized, "2", Some(1791436788))
        );
        assert_eq!(item.to.as_deref(), Some("TNPdqto8HiuMzoG7Vv9wyyYhWzCojLeHAF"));
        assert_eq!(
            native_item(&transfer("REVERT")).unwrap().unwrap().status,
            TxStatus::Failed
        );
    }

    #[test]
    fn only_failed_contract_calls_are_listed() {
        let call = |result: &str| {
            json!({
                "txID": HASH,
                "block_timestamp": 1000,
                "ret": [{"contractRet": result}],
                "raw_data": {"contract": [{
                    "type": "TriggerSmartContract",
                    "parameter": {"value": {"owner_address": OWNER, "contract_address": RECIPIENT}},
                }]},
            })
        };
        assert!(native_item(&call(SUCCESS)).unwrap().is_none());
        let item = native_item(&call("OUT_OF_ENERGY")).unwrap().unwrap();
        assert_eq!((item.status, item.amount.as_str()), (TxStatus::Failed, "0"));
    }

    #[test]
    fn other_system_contracts_are_not_transfers() {
        let mut row = transfer(SUCCESS);
        row["raw_data"]["contract"][0]["type"] = json!("FreezeBalanceV2Contract");
        assert!(native_item(&row).unwrap().is_none());
    }

    #[test]
    fn approvals_are_not_token_transfers() {
        let row = json!({"transaction_id": HASH, "block_timestamp": 2000, "from": "a", "to": "b", "type": "Approval", "value": "1"});
        assert!(token_item(&row, "T").unwrap().is_none());
        let mut row = row;
        row["type"] = json!("Transfer");
        let item = token_item(&row, "T").unwrap().unwrap();
        assert_eq!(
            (item.asset.as_str(), item.time, item.amount.as_str()),
            ("T", Some(2), "1")
        );
    }

    #[test]
    fn duplicates_count_as_accepted_and_busy_nodes_are_unknown() {
        assert_eq!(submitted(&json!({"result": true, "code": SUCCESS})), Ok(()));
        assert_eq!(submitted(&json!({"result": false, "code": DUPLICATE})), Ok(()));
        for code in NODE_BUSY {
            assert_eq!(submitted(&json!({"result": false, "code": code})), Err(None), "{code}");
        }
        assert_eq!(submitted(&json!({"Error": "格式错误"})), Err(None));
        let reply = json!({"result": false, "code": "SIGERROR", "message": "validate signature error"});
        assert_eq!(
            submitted(&reply),
            Err(Some("SIGERROR: validate signature error".to_string()))
        );
        assert_eq!(
            submitted(&json!({"code": "TAPOS_ERROR"})),
            Err(Some("TAPOS_ERROR".to_string()))
        );
    }

    #[test]
    fn chain_parameters_must_be_present() {
        let reply = json!({"chainParameter": [{"key": "getEnergyFee", "value": 420}, {"key": "zero"}, {"key": "bad", "value": "1"}]});
        assert_eq!(chain_parameter(&reply, "getEnergyFee").unwrap(), 420);
        // 值为 0 时节点不写 value.
        assert_eq!(chain_parameter(&reply, "zero").unwrap(), 0);
        assert!(chain_parameter(&reply, "bad").is_err());
        assert!(chain_parameter(&reply, "getTransactionFee").is_err());
    }
}

//! Electrum 上游的集成测试,上游是本地的行分隔 JSON-RPC 服务器.
//! 重点是批量响应按 id 归位,广播幂等,增量历史和换上游.

mod common;

use axum::http::StatusCode;
use bitcoin::absolute::LockTime;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::DisplayHex;
use bitcoin::transaction::Version;
use bitcoin::{Address, Amount, KnownHrp, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use bitone_gateway::config::Config;
use common::{Calls, Gateway, call, dead_url, error_code, next_json, ok, rpc_failure};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

const BTC: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
const BTC_OTHER: &str = "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr";
const TIME: u64 = 1_700_000_000;

/// 服务器主动推送的入口,连接着的会话都收得到.空字符串让服务器断开连接.
type Pushes = broadcast::Sender<String>;

async fn server(reply: impl Fn(&str, &Value) -> Value + Clone + Send + Sync + 'static) -> (String, Calls) {
    let (url, calls, _) = pushing_server(reply).await;
    (url, calls)
}

/// 一次读到的请求反序回复,前面夹一条没有 id 的推送,验证网关按 id 归位.
async fn pushing_server(
    reply: impl Fn(&str, &Value) -> Value + Clone + Send + Sync + 'static,
) -> (String, Calls, Pushes) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("tcp://{}", listener.local_addr().unwrap());
    let calls = Calls::default();
    let log = calls.clone();
    let (pushes, _) = broadcast::channel(16);
    let sender: Pushes = pushes.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (reply, log) = (reply.clone(), log.clone());
            let (reader, writer) = socket.into_split();
            let writer = Arc::new(tokio::sync::Mutex::new(writer));
            let mut pushed = pushes.subscribe();
            let outbound = writer.clone();
            tokio::spawn(async move {
                while let Ok(message) = pushed.recv().await {
                    let mut outbound = outbound.lock().await;
                    if message.is_empty() {
                        let _ = outbound.shutdown().await;
                        return;
                    }
                    let _ = outbound.write_all(format!("{message}\n").as_bytes()).await;
                }
            });
            tokio::spawn(async move {
                let mut reader = BufReader::new(reader);
                loop {
                    let mut batch = Vec::new();
                    loop {
                        let mut line = String::new();
                        if !matches!(reader.read_line(&mut line).await, Ok(1..)) {
                            return;
                        }
                        batch.push(serde_json::from_str::<Value>(&line).unwrap());
                        if reader.buffer().is_empty() {
                            break;
                        }
                    }
                    let push = json!({"method": "server.peers.subscribe", "params": []});
                    let mut out = format!("{push}\n");
                    for call in batch.iter().rev() {
                        let method = call["method"].as_str().unwrap();
                        log.record(method);
                        let mut response = match method {
                            "server.version" => ok(json!(["mock", "1.4"])),
                            "server.ping" => ok(Value::Null),
                            _ => reply(method, &call["params"]),
                        };
                        response["jsonrpc"] = json!("2.0");
                        response["id"] = call["id"].clone();
                        out.push_str(&format!("{response}\n"));
                    }
                    if writer.lock().await.write_all(out.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (url, calls, sender)
}

fn one_upstream(network: &str, url: &str) -> String {
    format!("[networks.{network}]\nupstreams = ['{url}']\n")
}

fn two_upstreams(first: &str, second: &str) -> String {
    format!("[networks.bitcoin]\nupstreams = ['{first}', '{second}']\n")
}

fn script_of(address: &str) -> ScriptBuf {
    Address::from_str(address).unwrap().assume_checked().script_pubkey()
}

fn script_hash(address: &str) -> String {
    let mut hash = sha256::Hash::hash(script_of(address).as_bytes()).to_byte_array();
    hash.reverse();
    hash.to_lower_hex_string()
}

fn pay(address: &str, sat: u64) -> TxOut {
    TxOut {
        value: Amount::from_sat(sat),
        script_pubkey: script_of(address),
    }
}

fn tx(inputs: &[OutPoint], output: Vec<TxOut>) -> Transaction {
    let input = inputs
        .iter()
        .map(|previous_output| TxIn {
            previous_output: *previous_output,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        })
        .collect();
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input,
        output,
    }
}

fn outpoint(parent: &Transaction, vout: u32) -> OutPoint {
    OutPoint::new(parent.compute_txid(), vout)
}

/// 区块时间在区块头的第 68 字节.
fn header(height: u64) -> String {
    let time = (TIME + height) as u32;
    format!(
        "{}{}{}",
        "00".repeat(68),
        time.to_le_bytes().to_lower_hex_string(),
        "00".repeat(8)
    )
}

#[derive(Clone, Default)]
struct Chain {
    tip: u64,
    txs: HashMap<Txid, (Transaction, i64)>,
    history: HashMap<String, Vec<Txid>>,
}

impl Chain {
    fn new(tip: u64) -> Self {
        Self {
            tip,
            ..Default::default()
        }
    }

    /// `height` 是 0 表示在内存池,`addresses` 是这笔交易出现在谁的历史里.
    fn add(&mut self, tx: &Transaction, height: i64, addresses: &[&str]) {
        let txid = tx.compute_txid();
        self.txs.insert(txid, (tx.clone(), height));
        for address in addresses {
            self.history.entry(script_hash(address)).or_default().push(txid);
        }
    }

    fn reply(&self, method: &str, params: &Value) -> Value {
        match method {
            "blockchain.headers.subscribe" => ok(json!({"height": self.tip, "hex": ""})),
            "blockchain.scripthash.subscribe" => ok(Value::Null),
            "blockchain.scripthash.unsubscribe" => ok(json!(true)),
            "blockchain.scripthash.get_balance" => {
                let mine = self.history.contains_key(params[0].as_str().unwrap());
                ok(json!({"confirmed": if mine { 700 } else { 0 }, "unconfirmed": 0}))
            }
            "blockchain.block.header" => ok(json!(header(params[0].as_u64().unwrap()))),
            "blockchain.scripthash.get_history" => {
                let rows = self.history.get(params[0].as_str().unwrap());
                let rows: Vec<Value> = rows
                    .into_iter()
                    .flatten()
                    .map(|txid| json!({"tx_hash": txid.to_string(), "height": self.txs[txid].1}))
                    .collect();
                ok(json!(rows))
            }
            "blockchain.transaction.get" => {
                let txid = Txid::from_str(params[0].as_str().unwrap()).unwrap();
                let Some((tx, _)) = self.txs.get(&txid) else {
                    return rpc_failure(2, "daemon error: No such mempool or blockchain transaction");
                };
                if params[1] == json!(true) {
                    return rpc_failure(-32603, "verbose transactions are currently unsupported");
                }
                ok(json!(serialize_hex(tx)))
            }
            _ => panic!("没有模拟 {method}"),
        }
    }

    async fn serve(self) -> (String, Calls) {
        server(move |method, params| self.reply(method, params)).await
    }
}

/// 一条收款和一条带找零的付款.
fn payment_pair() -> (Transaction, Transaction, Transaction) {
    let parent = tx(&[OutPoint::null()], vec![pay(BTC_OTHER, 200_000)]);
    let incoming = tx(&[outpoint(&parent, 0)], vec![pay(BTC, 150_000), pay(BTC_OTHER, 40_000)]);
    let outgoing = tx(
        &[outpoint(&incoming, 0)],
        vec![pay(BTC_OTHER, 100_000), pay(BTC, 45_000)],
    );
    (parent, incoming, outgoing)
}

#[tokio::test]
async fn balances_stay_with_their_address() {
    let (url, _) = server(|method, params| {
        assert_eq!(method, "blockchain.scripthash.get_balance");
        let confirmed = if params[0] == script_hash(BTC) { 70 } else { 8 };
        ok(json!({"confirmed": confirmed, "unconfirmed": 1000}))
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let (status, reply) = gateway.balances("bitcoin", &[BTC, BTC_OTHER]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply["balances"],
        json!([
            {"address": BTC, "asset": "native", "amount": "70"},
            {"address": BTC_OTHER, "asset": "native", "amount": "8"},
        ])
    );
}

#[tokio::test]
async fn utxos_mark_the_mempool_with_height_zero() {
    let (url, _) = server(|_, params| {
        assert_eq!(params[0], script_hash(BTC));
        ok(json!([
            {"tx_hash": "a".repeat(64), "tx_pos": 1, "height": 840000, "value": 50000},
            {"tx_hash": "b".repeat(64), "tx_pos": 0, "height": 0, "value": 7},
        ]))
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/utxos", &json!({"addresses": [BTC]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"utxos": [
            {"address": BTC, "txid": "a".repeat(64), "vout": 1, "amount": "50000", "height": 840000},
            {"address": BTC, "txid": "b".repeat(64), "vout": 0, "amount": "7", "height": 0},
        ]})
    );
}

#[tokio::test]
async fn fees_convert_to_sat_per_vbyte() {
    let (url, _) = server(|_, params| match params[0].as_u64().unwrap() {
        1 => ok(json!(0.0002)),
        6 => ok(json!(0.00010)),
        _ => ok(json!(0.00002)),
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let (status, reply) = gateway.get_json("/v1/bitcoin/fees").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"kind": "bitcoin", "slow": 2, "medium": 10, "fast": 20}));
}

#[tokio::test]
async fn missing_fee_estimates_fail_only_on_mainnet() {
    let (minus_one, _) = server(|_, _| ok(json!(-1))).await;
    let (failure, _) = server(|_, _| rpc_failure(-32603, "cannot estimate fee for 1 blocks")).await;
    for url in [&minus_one, &failure] {
        let gateway = Gateway::start(&one_upstream("bitcoin", url)).await;
        assert_eq!(
            gateway.get_json("/v1/bitcoin/fees").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );

        let gateway = Gateway::start(&one_upstream("btc-testnet4", url)).await;
        let (status, reply) = gateway.get_json("/v1/btc-testnet4/fees").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, json!({"kind": "bitcoin", "slow": 1, "medium": 1, "fast": 1}));
    }
}

#[tokio::test]
async fn raw_txs_leave_out_what_the_server_lacks() {
    let (_, incoming, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&incoming, 90, &[]);
    let txid = incoming.compute_txid().to_string();
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let body = json!({"txids": [txid, "e".repeat(64)]});
    let (status, reply) = gateway.post("/v1/bitcoin/raw-txs", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"txs": {txid: serialize_hex(&incoming)}}));
}

#[tokio::test]
async fn raw_txs_reject_garbage_from_the_server() {
    let (bad, _) = server(|_, _| ok(json!("not hex"))).await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &bad)).await;
    let body = json!({"txids": ["e".repeat(64)]});
    let (status, reply) = gateway.post("/v1/bitcoin/raw-txs", &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[tokio::test]
async fn tx_status_covers_every_stage() {
    let (parent, incoming, outgoing) = payment_pair();
    let coinbase = tx(&[OutPoint::null()], vec![pay(BTC, 5_000_000_000)]);
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    chain.add(&incoming, 95, &[BTC]);
    chain.add(&outgoing, 96, &[BTC]);
    chain.add(&coinbase, 90, &[BTC]);
    let pending = tx(&[outpoint(&outgoing, 0)], vec![pay(BTC, 99_000)]);
    chain.add(&pending, 0, &[BTC]);
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let hash = |tx: &Transaction| tx.compute_txid().to_string();
    let unknown = "e".repeat(64);
    let body = json!({"hashes": [hash(&incoming), hash(&outgoing), hash(&coinbase), hash(&pending), unknown]});
    let (status, reply) = gateway.post("/v1/bitcoin/tx", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply["txs"],
        json!([
            {"hash": hash(&incoming), "status": "finalized", "confirmations": 6, "stage": null, "block": 95, "fee": "10000"},
            {"hash": hash(&outgoing), "status": "included", "confirmations": 5, "stage": null, "block": 96, "fee": "5000"},
            {"hash": hash(&coinbase), "status": "finalized", "confirmations": 11, "stage": null, "block": 90, "fee": "0"},
            {"hash": hash(&pending), "status": "pending", "confirmations": 0, "stage": null, "block": null, "fee": null},
            {"hash": unknown, "status": "unknown", "confirmations": 0, "stage": null, "block": null, "fee": null},
        ])
    );
}

/// 收款方的地址可能很热门,服务器会拒绝返回太长的历史,这时用找零的历史.
#[tokio::test]
async fn tx_status_skips_a_script_whose_history_is_too_large() {
    let (parent, incoming, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    chain.add(&incoming, 95, &[BTC]);
    let popular = script_hash(BTC_OTHER);
    let (url, _) = server(move |method, params| match method {
        "blockchain.scripthash.get_history" if params[0] == popular => rpc_failure(1, "history too large"),
        _ => chain.reply(method, params),
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let body = json!({"hashes": [incoming.compute_txid().to_string()]});
    let (_, reply) = gateway.post("/v1/bitcoin/tx", &body).await;
    assert_eq!(reply["txs"][0]["status"], "finalized");
    assert_eq!(reply["txs"][0]["fee"], "10000");
}

/// 只有 OP_RETURN 输出的交易不进输出的索引,要靠它花费的脚本找到.
#[tokio::test]
async fn tx_status_finds_a_burn_through_its_inputs() {
    let parent = tx(&[OutPoint::null()], vec![pay(BTC, 1_000)]);
    let data = TxOut {
        value: Amount::ZERO,
        script_pubkey: ScriptBuf::new_op_return([1u8; 4]),
    };
    let burn = tx(&[outpoint(&parent, 0)], vec![data]);
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[BTC]);
    chain.add(&burn, 95, &[BTC]);
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let body = json!({"hashes": [burn.compute_txid().to_string()]});
    let (_, reply) = gateway.post("/v1/bitcoin/tx", &body).await;
    assert_eq!(reply["txs"][0]["status"], "finalized");
    assert_eq!(reply["txs"][0]["fee"], "1000");
}

/// 找不到高度时不能把已上链的交易报成查不到.
#[tokio::test]
async fn tx_status_moves_on_when_no_output_history_has_the_tx() {
    let (parent, incoming, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    chain.add(&incoming, 95, &[]);
    let (bad, _) = chain.clone().serve().await;
    chain.history.insert(script_hash(BTC), vec![incoming.compute_txid()]);
    let (good, _) = chain.serve().await;
    let gateway = Gateway::start(&two_upstreams(&bad, &good)).await;
    let body = json!({"hashes": [incoming.compute_txid().to_string()]});
    let (status, reply) = gateway.post("/v1/bitcoin/tx", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["txs"][0]["status"], "finalized");
}

#[tokio::test]
async fn history_reads_direction_amount_and_time_from_raw_transactions() {
    let (parent, incoming, outgoing) = payment_pair();
    let coinbase = tx(&[OutPoint::null()], vec![pay(BTC, 5_000_000_000)]);
    let mempool = tx(&[outpoint(&parent, 0)], vec![pay(BTC, 1_000)]);
    let mut chain = Chain::new(100);
    chain.add(&parent, 50, &[]);
    chain.add(&coinbase, 60, &[BTC]);
    chain.add(&incoming, 95, &[BTC]);
    // 找零回到自己,同一笔在两个地址的历史里只算一条.
    chain.add(&outgoing, 96, &[BTC, BTC_OTHER]);
    chain.add(&mempool, -1, &[BTC]);
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let body = json!({"addresses": [BTC, BTC_OTHER]});
    let (status, reply) = gateway.post("/v1/bitcoin/history", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["checkpoint"], "95");
    assert_eq!(reply["cursor"], Value::Null);
    let items = reply["items"].as_array().unwrap();
    let hashes: Vec<String> = [&mempool, &outgoing, &incoming, &coinbase]
        .iter()
        .map(|tx| tx.compute_txid().to_string())
        .collect();
    assert_eq!(
        items
            .iter()
            .map(|item| item["hash"].as_str().unwrap())
            .collect::<Vec<_>>(),
        hashes,
        "内存池排最前,其余新的在前"
    );
    assert_eq!(items[0]["status"], "pending");
    assert_eq!(items[0]["time"], Value::Null);
    assert_eq!(items[1]["status"], "included");
    assert_eq!(items[1]["time"], TIME + 96);
    assert_eq!(items[2]["status"], "finalized");
    assert_eq!(items[2]["time"], TIME + 95);
    assert_eq!(items[3]["from"], Value::Null, "coinbase 没有发送方");
    assert_eq!(items[3]["to"], BTC);
    assert_eq!(items[3]["amount"], "5000000000");

    let body = json!({"addresses": [BTC]});
    let (_, reply) = gateway.post("/v1/bitcoin/history", &body).await;
    let spend = &reply["items"][1];
    assert_eq!(spend["from"], BTC);
    assert_eq!(spend["to"], BTC_OTHER);
    assert_eq!(spend["amount"], "100000", "付款金额不含找零");
    let receipt = &reply["items"][2];
    assert_eq!(receipt["from"], BTC_OTHER);
    assert_eq!(receipt["to"], BTC);
    assert_eq!(receipt["amount"], "150000");
}

#[tokio::test]
async fn history_pages_without_gaps_or_repeats() {
    let mut chain = Chain::new(1000);
    let mut expected = Vec::new();
    for height in 1..=120 {
        let row = tx(&[OutPoint::null()], vec![pay(BTC, height)]);
        chain.add(&row, height as i64, &[BTC]);
        expected.push(row.compute_txid().to_string());
    }
    expected.reverse();
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let body = json!({"addresses": [BTC], "cursor": cursor});
        let (status, reply) = gateway.post("/v1/bitcoin/history", &body).await;
        assert_eq!(status, StatusCode::OK);
        seen.extend(
            reply["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["hash"].as_str().unwrap().to_string()),
        );
        pages += 1;
        cursor = reply["cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(pages, 5);
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn history_delta_starts_after_the_checkpoint_and_keeps_the_mempool() {
    let (parent, incoming, outgoing) = payment_pair();
    let mempool = tx(&[outpoint(&parent, 0)], vec![pay(BTC, 1_000)]);
    let mut chain = Chain::new(100);
    chain.add(&parent, 50, &[]);
    chain.add(&incoming, 90, &[BTC]);
    chain.add(&outgoing, 91, &[BTC]);
    chain.add(&mempool, 0, &[BTC]);
    let (url, _) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let body = json!({"addresses": [BTC], "since": "90"});
    let (_, reply) = gateway.post("/v1/bitcoin/history", &body).await;
    let hashes: Vec<&str> = reply["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["hash"].as_str().unwrap())
        .collect();
    assert_eq!(
        hashes,
        [mempool.compute_txid().to_string(), outgoing.compute_txid().to_string()],
    );
}

#[tokio::test]
async fn history_skips_addresses_when_the_tip_has_not_moved() {
    let (url, calls) = Chain::new(100).serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let body = json!({"addresses": [BTC], "since": "95"});
    let (_, reply) = gateway.post("/v1/bitcoin/history", &body).await;
    assert_eq!(reply["unchanged"], true);
    assert_eq!(reply["checkpoint"], "95");
    assert_eq!(calls.count("blockchain.scripthash.get_history"), 0);

    let body = json!({"addresses": [BTC], "since": "94"});
    let (_, reply) = gateway.post("/v1/bitcoin/history", &body).await;
    assert_eq!(reply["unchanged"], false);
    assert_eq!(calls.count("blockchain.scripthash.get_history"), 1);
}

/// 前序交易超过一批的上限时分批发送,结果不能错位.
#[tokio::test]
async fn history_with_more_parents_than_one_batch_stays_in_order() {
    let sender = |seed: u32| {
        let script = ScriptBuf::from_bytes(seed.to_le_bytes().to_vec());
        Address::p2wsh(&script, KnownHrp::Mainnet).to_string()
    };
    let mut chain = Chain::new(1000);
    let mut expected = Vec::new();
    for height in 1..=25 {
        let parents: Vec<Transaction> = (0..5)
            .map(|i| tx(&[OutPoint::null()], vec![pay(&sender(height * 10 + i), 1_000)]))
            .collect();
        for parent in &parents {
            chain.add(parent, 1, &[]);
        }
        let inputs: Vec<OutPoint> = parents.iter().map(|parent| outpoint(parent, 0)).collect();
        chain.add(&tx(&inputs, vec![pay(BTC, 900)]), i64::from(height) + 1, &[BTC]);
        expected.push(sender(height * 10));
    }
    expected.reverse();
    let (url, calls) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;

    let (status, reply) = gateway.post("/v1/bitcoin/history", &json!({"addresses": [BTC]})).await;
    assert_eq!(status, StatusCode::OK);
    let senders: Vec<&str> = reply["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["from"].as_str().unwrap())
        .collect();
    assert_eq!(senders, expected);
    assert_eq!(calls.count("blockchain.transaction.get"), 150);
}

#[tokio::test]
async fn a_dead_or_broken_server_moves_on_to_the_next() {
    let dead = dead_url().await.replacen("http", "tcp", 1);
    let (garbled, _) = server(|_, _| ok(json!({"unexpected": true}))).await;
    let (good, _) = server(|_, _| ok(json!({"confirmed": 5, "unconfirmed": 0}))).await;
    for first in [&dead, &garbled] {
        let gateway = Gateway::start(&two_upstreams(first, &good)).await;
        let (status, reply) = gateway.balances("bitcoin", &[BTC]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["balances"][0]["amount"], "5");
    }
}

#[tokio::test]
async fn esplora_can_back_up_an_electrum_server() {
    let dead = dead_url().await.replacen("http", "tcp", 1);
    let esplora = axum::Router::new().route(
        "/address/{address}",
        axum::routing::get(|| async { axum::Json(json!({"chain_stats": {"funded_txo_sum": 9, "spent_txo_sum": 2}})) }),
    );
    let esplora = common::mock(esplora).await;
    let gateway = Gateway::start(&two_upstreams(&dead, &esplora)).await;
    let (_, reply) = gateway.balances("bitcoin", &[BTC]).await;
    assert_eq!(reply["balances"][0]["amount"], "7");
}

#[tokio::test]
async fn used_reports_addresses_with_any_history_in_one_request() {
    let (_, incoming, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&incoming, 90, &[BTC]);
    let (url, calls) = chain.serve().await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let (status, reply) = gateway
        .post("/v1/bitcoin/used", &json!({"addresses": [BTC, BTC_OTHER]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"used": [BTC]}));
    assert_eq!(calls.count("server.version"), 1, "一批地址一条连接");
}

fn broadcast_tx() -> (Transaction, String) {
    let (_, incoming, _) = payment_pair();
    let hash = incoming.compute_txid().to_string();
    (incoming, hash)
}

#[tokio::test]
async fn broadcast_sends_the_raw_hex_once() {
    let (tx, hash) = broadcast_tx();
    let raw = serialize_hex(&tx);
    let expected = raw.clone();
    let reply_hash = hash.clone();
    let (url, calls) = server(move |_, params| {
        assert_eq!(params[0], expected);
        ok(json!(reply_hash))
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": raw})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"hash": hash, "status": "accepted"}));
    assert_eq!(calls.count("blockchain.transaction.broadcast"), 1);
}

#[tokio::test]
async fn broadcast_is_idempotent() {
    let (tx, hash) = broadcast_tx();
    for text in [
        "the transaction was rejected by network rules.\n\nTransaction already in block chain",
        "the transaction was rejected by network rules.\n\ntxn-already-in-mempool",
        "the transaction was rejected by network rules.\n\ntxn-already-known",
    ] {
        let (url, _) = server(move |_, _| rpc_failure(1, text)).await;
        let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
        let body = json!({"raw": serialize_hex(&tx)});
        let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &body).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(reply, json!({"hash": hash, "status": "accepted"}));
    }
}

#[tokio::test]
async fn broadcast_rejection_carries_the_text_and_is_not_retried() {
    let (tx, _) = broadcast_tx();
    for (code, text) in [
        (
            1,
            "the transaction was rejected by network rules.\n\nmin relay fee not met",
        ),
        (2, "sendrawtransaction RPC error -26: min relay fee not met"),
        (
            -32603,
            r#"sendrawtransaction RPC error: {"code":-26,"message":"min relay fee not met"}"#,
        ),
    ] {
        let chain = Chain::new(100);
        let (a, first) = server(move |method, params| match method {
            "blockchain.transaction.broadcast" => rpc_failure(code, text),
            _ => chain.reply(method, params),
        })
        .await;
        let (b, second) = server(|_, _| ok(json!("unused"))).await;
        let gateway = Gateway::start(&two_upstreams(&a, &b)).await;
        let body = json!({"raw": serialize_hex(&tx)});
        let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(reply, json!({"error": {"code": "broadcast_rejected", "message": text}}));
        assert_eq!(first.count("blockchain.transaction.broadcast"), 1);
        assert_eq!(second.total(), 0, "被拒绝的广播不能换上游重试");
    }
}

/// 交易上链后找零又被花掉,节点只会报输入缺失.
#[tokio::test]
async fn broadcast_of_a_spent_tx_on_chain_is_accepted() {
    let (parent, incoming, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    chain.add(&incoming, 90, &[BTC]);
    let (url, _) = server(move |method, params| match method {
        "blockchain.transaction.broadcast" => rpc_failure(1, "bad-txns-inputs-missingorspent"),
        _ => chain.reply(method, params),
    })
    .await;
    let gateway = Gateway::start(&one_upstream("bitcoin", &url)).await;
    let body = json!({"raw": serialize_hex(&incoming)});
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &body).await;
    assert_eq!((status, reply["status"].as_str()), (StatusCode::OK, Some("accepted")));
}

#[tokio::test]
async fn broadcast_server_errors_are_not_rejections_and_are_not_retried() {
    let (tx, _) = broadcast_tx();
    for (code, text) in [
        (-32603, "internal error"),
        (2, "Daemon is unavailable: sendrawtransaction failed: No route to host"),
        (
            -32603,
            r#"sendrawtransaction RPC error: {"code":-28,"message":"Loading block index"}"#,
        ),
    ] {
        let (a, _) = server(move |_, _| rpc_failure(code, text)).await;
        let (b, second) = server(|_, _| ok(json!("unused"))).await;
        let gateway = Gateway::start(&two_upstreams(&a, &b)).await;
        let body = json!({"raw": serialize_hex(&tx)});
        let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(&reply), "upstream_unavailable");
        assert_eq!(second.total(), 0, "请求可能已经到达节点,不能换上游");
    }
}

#[tokio::test]
async fn broadcast_moves_on_when_the_connection_never_opened() {
    let (tx, hash) = broadcast_tx();
    let dead = dead_url().await.replacen("http", "tcp", 1);
    let reply_hash = hash.clone();
    let (good, calls) = server(move |_, _| ok(json!(reply_hash))).await;
    let gateway = Gateway::start(&two_upstreams(&dead, &good)).await;
    let body = json!({"raw": serialize_hex(&tx)});
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"hash": hash, "status": "accepted"}));
    assert_eq!(calls.count("blockchain.transaction.broadcast"), 1);
}

#[test]
fn config_keeps_electrum_to_bitcoin_and_verified_mainnet() {
    let parses = |network: &str, url: &str| Config::parse(&one_upstream(network, url)).is_ok();
    assert!(parses("bitcoin", "ssl://electrum.example:50002"));
    assert!(parses("bitcoin", "tcp://127.0.0.1:50001"));
    assert!(parses("btc-testnet4", "ssl-insecure://electrum.example:50002"));
    assert!(
        !parses("bitcoin", "ssl-insecure://electrum.example:50002"),
        "主网必须验证证书"
    );
    assert!(!parses("bitcoin", "ssl://electrum.example"), "端口不能省略");
    assert!(!parses("ethereum", "ssl://electrum.example:50002"));
}

fn watch(network: &str, addresses: &[&str]) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe",
           "params": {"channel": "addresses", "network": network, "addresses": addresses}})
}

fn status_push(address: &str) -> String {
    json!({"method": "blockchain.scripthash.subscribe", "params": [script_hash(address), "changed"]}).to_string()
}

fn head_push(height: u64) -> String {
    json!({"method": "blockchain.headers.subscribe", "params": [{"height": height, "hex": ""}]}).to_string()
}

async fn wait_for_calls(calls: &Calls, method: &str, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while calls.count(method) < count {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等 {method} 超时"));
}

/// 内存池里的交易不改变已确认余额,只有服务器的推送能让用户立刻看到.
#[tokio::test]
async fn long_connection_reports_mempool_transactions_at_once() {
    let (parent, _, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    let chain = Arc::new(Mutex::new(chain));
    let node = chain.clone();
    let (url, calls, pushes) = pushing_server(move |method, params| node.lock().unwrap().reply(method, params)).await;
    let gateway = Gateway::start(&format!(
        "{}{}",
        "ws_poll_interval_ms = 10\nfollow_poll_interval_ms = 10\n",
        one_upstream("bitcoin", &url)
    ))
    .await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, watch("bitcoin", &[BTC])).await["result"].is_string());
    wait_for_calls(&calls, "blockchain.scripthash.subscribe", 1).await;
    // 建立连接时补查一次,内容为空,这里先消费掉.
    assert_eq!(next_json(&mut ws).await["params"]["txs"], json!([]));

    let incoming = tx(&[outpoint(&parent, 0)], vec![pay(BTC, 1_000)]);
    chain.lock().unwrap().add(&incoming, 0, &[BTC]);
    pushes.send(status_push(BTC)).unwrap();
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["height"], 100, "高度没变,交易照样推送");
    assert_eq!(update["params"]["txs"][0]["hash"], incoming.compute_txid().to_string());
    assert_eq!(update["params"]["txs"][0]["status"], "pending");
    assert_eq!(
        calls.count("blockchain.scripthash.subscribe"),
        1,
        "地址只订阅一次,不轮询"
    );

    chain.lock().unwrap().tip = 101;
    pushes.send(head_push(101)).unwrap();
    let update = next_json(&mut ws).await;
    assert_eq!(
        (&update["params"]["previous"], &update["params"]["height"]),
        (&json!(100), &json!(101))
    );
}

/// 连接断了要换上游,重新订阅,并补一次断开期间可能错过的变化.
#[tokio::test]
async fn long_connection_moves_on_when_the_server_goes_away() {
    let (parent, _, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    let incoming = tx(&[outpoint(&parent, 0)], vec![pay(BTC, 1_000)]);
    let chain = Arc::new(Mutex::new(chain));
    let (first, second) = (chain.clone(), chain.clone());
    let (first, first_calls, pushes) =
        pushing_server(move |method, params| first.lock().unwrap().reply(method, params)).await;
    let (second, second_calls, _) =
        pushing_server(move |method, params| second.lock().unwrap().reply(method, params)).await;
    let config = format!(
        "ws_poll_interval_ms = 10\nfollow_poll_interval_ms = 10\n{}",
        two_upstreams(&first, &second)
    );
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, watch("bitcoin", &[BTC])).await;
    wait_for_calls(&first_calls, "blockchain.scripthash.subscribe", 1).await;
    assert_eq!(next_json(&mut ws).await["params"]["txs"], json!([]));

    // 这笔交易到达时连接已经断了,服务器不会推送.
    chain.lock().unwrap().add(&incoming, 0, &[BTC]);
    pushes.send(String::new()).unwrap();
    wait_for_calls(&second_calls, "blockchain.scripthash.subscribe", 1).await;
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["txs"][0]["status"], "pending");
}

/// 基线比上游订阅早,后加入的地址在这段时间到账时服务器不会推送,要补查一次.
#[tokio::test]
async fn an_address_added_later_is_checked_after_it_is_subscribed() {
    let (parent, _, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    let incoming = tx(&[outpoint(&parent, 0)], vec![pay(BTC_OTHER, 1_000)]);
    let txid = incoming.compute_txid().to_string();
    let chain = Arc::new(Mutex::new(chain));
    let node = chain.clone();
    let later = script_hash(BTC_OTHER);
    let (url, calls, _) = pushing_server(move |method, params| {
        let mut chain = node.lock().unwrap();
        if method == "blockchain.scripthash.subscribe" && params[0] == later {
            chain.add(&incoming, 0, &[BTC_OTHER]);
        }
        chain.reply(method, params)
    })
    .await;
    let config = format!(
        "ws_poll_interval_ms = 10\nfollow_poll_interval_ms = 10\n{}",
        one_upstream("bitcoin", &url)
    );
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, watch("bitcoin", &[BTC])).await["result"].is_string());
    wait_for_calls(&calls, "blockchain.scripthash.subscribe", 1).await;
    assert_eq!(next_json(&mut ws).await["params"]["txs"], json!([]));

    assert!(call(&mut ws, watch("bitcoin", &[BTC_OTHER])).await["result"].is_string());
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["txs"][0]["hash"], txid, "{update}");
}

/// 没有人看的地址要退订,退订之后服务器发来的推送也不能断开连接.
#[tokio::test]
async fn long_connection_unsubscribes_addresses_nobody_watches() {
    let (parent, _, _) = payment_pair();
    let mut chain = Chain::new(100);
    chain.add(&parent, 80, &[]);
    let chain = Arc::new(Mutex::new(chain));
    let node = chain.clone();
    let (url, calls, pushes) = pushing_server(move |method, params| node.lock().unwrap().reply(method, params)).await;
    let config = format!(
        "ws_poll_interval_ms = 10\nfollow_poll_interval_ms = 10\n{}",
        one_upstream("bitcoin", &url)
    );
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await;
    let first = call(&mut ws, watch("bitcoin", &[BTC])).await["result"].clone();
    assert!(call(&mut ws, watch("bitcoin", &[BTC_OTHER])).await["result"].is_string());
    wait_for_calls(&calls, "blockchain.scripthash.subscribe", 2).await;

    let message = json!({"jsonrpc": "2.0", "id": 9, "method": "unsubscribe", "params": [first]});
    let mut update = call(&mut ws, message).await;
    while update.get("result").is_none() {
        update = next_json(&mut ws).await;
    }
    wait_for_calls(&calls, "blockchain.scripthash.unsubscribe", 1).await;
    pushes.send(status_push(BTC)).unwrap();
    // 节点的高度不动,这个高度只能从长连接的推送拿到,退回轮询时等不到.
    pushes.send(head_push(101)).unwrap();
    let mut height = json!(null);
    for _ in 0..4 {
        height = next_json(&mut ws).await["params"]["height"].clone();
        if height == 101 {
            break;
        }
    }
    assert_eq!(height, 101, "退订之后长连接还在");
}

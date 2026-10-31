//! 查询接口和广播的集成测试,上游都是本地模拟服务.
//! 重点是 failed 不被报成成功,广播幂等,广播超时和不适用的链.

mod common;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use common::{Calls, Gateway, dead_url, error_code, mock, ok, rpc_failure, rpc_node};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;

const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
const EVM_OTHER: &str = "0x3535353535353535353535353535353535353535";
const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
const BTC: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
const BTC_OTHER: &str = "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr";
const TRON: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
const SOLANA: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const SOLANA_MINT: &str = "So11111111111111111111111111111111111111112";

/// 创世块的 coinbase.
const BTC_RAW: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";
const BTC_TXID: &str = "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b";
/// EIP-155 文档里的交易示例.
const EVM_RAW: &str = "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";
const EVM_HASH: &str = "0x33469b22e9f636356c4160a87eb19df52b7412e8eac32a4a55ffe88ea8350788";
/// 第一个字段长度 3 的 protobuf,raw_data 是 `aabbcc`.
const TRON_RAW: &str = "0a03aabbcc";

fn hex_hash(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn evm_hash(seed: char) -> String {
    format!("0x{}", hex_hash(seed))
}

fn solana_signature(seed: u8) -> String {
    bitcoin::base58::encode(&[seed; 64])
}

fn solana_raw() -> String {
    format!("01{}0102", "07".repeat(64))
}

fn network_config(network: &str, url: &str) -> String {
    format!("[networks.{network}]\nupstreams = ['{url}']\n")
}

fn two_upstreams(network: &str, first: &str, second: &str) -> String {
    format!("[networks.{network}]\nupstreams = ['{first}', '{second}']\n")
}

// ---- 不适用的链 ----

#[tokio::test]
async fn endpoints_refuse_the_wrong_chain_before_any_upstream_call() {
    let dead = dead_url().await;
    let config: String = ["bitcoin", "ethereum", "tron", "solana"]
        .iter()
        .map(|n| network_config(n, &dead))
        .collect();
    let gateway = Gateway::start(&config).await;
    let addresses = json!({"addresses": []});
    let posts = [
        ("utxos", vec!["ethereum", "tron", "solana"], addresses.clone()),
        ("raw-txs", vec!["ethereum", "tron", "solana"], json!({"txids": []})),
        ("nonce", vec!["bitcoin", "tron", "solana"], addresses.clone()),
        ("resources", vec!["bitcoin", "ethereum", "solana"], addresses),
        (
            "nonce-account",
            vec!["bitcoin", "ethereum", "tron"],
            json!({"address": SOLANA}),
        ),
        ("estimate", vec!["bitcoin"], json!({})),
    ];
    for (endpoint, networks, body) in posts {
        for network in networks {
            let (status, reply) = gateway.post(&format!("/v1/{network}/{endpoint}"), &body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{network} {endpoint}");
            assert_eq!(error_code(&reply), "invalid_request", "{network} {endpoint}");
        }
    }
    let gets = [
        ("ref-block", ["bitcoin", "ethereum", "solana"]),
        ("blockhash", ["bitcoin", "ethereum", "tron"]),
    ];
    for (endpoint, networks) in gets {
        for network in networks {
            let (status, reply) = gateway.get_json(&format!("/v1/{network}/{endpoint}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{network} {endpoint}");
            assert_eq!(error_code(&reply), "invalid_request", "{network} {endpoint}");
        }
    }
}

#[tokio::test]
async fn unknown_networks_are_reported_first_on_every_endpoint() {
    let gateway = Gateway::start("").await;
    for endpoint in [
        "utxos",
        "raw-txs",
        "nonce",
        "resources",
        "nonce-account",
        "estimate",
        "broadcast",
        "tx",
        "history",
    ] {
        let (status, reply) = gateway.post(&format!("/v1/dogecoin/{endpoint}"), &json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{endpoint}");
        assert_eq!(error_code(&reply), "unknown_network", "{endpoint}");
    }
    for endpoint in ["ref-block", "blockhash", "fees"] {
        let (status, reply) = gateway.get_json(&format!("/v1/dogecoin/{endpoint}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{endpoint}");
        assert_eq!(error_code(&reply), "unknown_network", "{endpoint}");
    }
}

#[tokio::test]
async fn malformed_bodies_never_reach_the_upstream() {
    let (url, calls) = rpc_node(|_, _| ok(json!(null))).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let endpoint = |name: &str| format!("/v1/ethereum/{name}");
    let cases = [
        ("nonce", json!({"addresses": "x"})),
        ("tx", json!({})),
        ("tx", json!({"hashes": ["0x12"]})),
        ("tx", json!({"hashes": [hex_hash('a')]})),
        ("broadcast", json!({})),
        ("broadcast", json!({"raw": "zz"})),
        ("broadcast", json!({"raw": ""})),
        ("estimate", json!({"from": EVM})),
        ("estimate", json!({"from": EVM, "to": EVM, "value": "1.5"})),
        ("estimate", json!({"from": EVM, "to": EVM, "value": "-1"})),
        ("estimate", json!({"from": EVM, "to": EVM, "data": "zz"})),
        ("estimate", json!({"from": EVM, "to": EVM, "data": "abcd"})),
        ("history", json!({"addresses": [EVM], "cursor": "不是游标"})),
        ("history", json!({"addresses": [EVM], "since": "不是水位"})),
        ("history", json!({"addresses": [EVM], "cursor": "{\"9:txlist\":\"2\"}"})),
    ];
    for (name, body) in cases {
        let (status, reply) = gateway.post(&endpoint(name), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name} {body}");
        assert_eq!(error_code(&reply), "invalid_request", "{name} {body}");
    }
    let (_, reply) = gateway
        .post(&endpoint("estimate"), &json!({"from": "0x12", "to": EVM}))
        .await;
    assert_eq!(error_code(&reply), "invalid_address");
    assert_eq!(calls.total(), 0);
}

#[tokio::test]
async fn hash_lists_are_capped_at_100() {
    let (url, calls) = rpc_node(|_, _| ok(json!(null))).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let hashes: Vec<String> = (0..101).map(|i| format!("0x{i:064x}")).collect();
    let (status, reply) = gateway.post("/v1/ethereum/tx", &json!({"hashes": hashes})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&reply), "invalid_request");
    assert_eq!(calls.total(), 0);

    let txids: Vec<String> = (0..101).map(|i| format!("{i:064x}")).collect();
    let config = network_config("bitcoin", &dead_url().await);
    let gateway = Gateway::start(&config).await;
    let (status, reply) = gateway.post("/v1/bitcoin/raw-txs", &json!({"txids": txids})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&reply), "invalid_request");
}

// ---- Bitcoin ----

#[derive(Clone, Default)]
struct Esplora {
    /// `POST /tx` 的固定响应.
    broadcast: Arc<Mutex<Option<(u16, String)>>>,
    posted: Arc<Mutex<Vec<String>>>,
    tx: Arc<Mutex<Option<Value>>>,
    hits: Arc<AtomicUsize>,
}

async fn esplora_tx(State(node): State<Esplora>, Path(_): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match node.tx.lock().unwrap().clone() {
        Some(tx) => Json(tx).into_response(),
        None => (StatusCode::NOT_FOUND, "Transaction not found").into_response(),
    }
}

async fn esplora_broadcast(State(node): State<Esplora>, body: String) -> (StatusCode, String) {
    node.hits.fetch_add(1, Ordering::SeqCst);
    node.posted.lock().unwrap().push(body);
    let (status, text) = node.broadcast.lock().unwrap().clone().unwrap();
    (StatusCode::from_u16(status).unwrap(), text)
}

impl Esplora {
    async fn start(status: u16, text: &str) -> (Self, String) {
        let node = Self::default();
        *node.broadcast.lock().unwrap() = Some((status, text.to_string()));
        let router = Router::new()
            .route("/blocks/tip/height", get(|| async { "100\n" }))
            .route("/tx", post(esplora_broadcast))
            .route("/tx/{txid}", get(esplora_tx))
            .with_state(node.clone());
        (node, mock(router).await)
    }

    fn set_tx(&self, tx: Value) {
        *self.tx.lock().unwrap() = Some(tx);
    }
}

fn btc_confirmed(height: u64) -> Value {
    json!({"txid": BTC_TXID, "fee": 141, "status": {"confirmed": true, "block_height": height, "block_time": 1700000000}})
}

#[tokio::test]
async fn bitcoin_tx_reports_confirmations_and_the_six_block_rule() {
    let (node, url) = Esplora::start(200, "").await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let body = json!({"hashes": [BTC_TXID]});

    let (status, reply) = gateway.post("/v1/bitcoin/tx", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"txs": [{"hash": BTC_TXID, "status": "unknown", "confirmations": 0, "stage": null, "block": null, "fee": null}]}),
    );

    node.set_tx(json!({"status": {"confirmed": false}, "fee": 141}));
    assert_eq!(
        gateway.post("/v1/bitcoin/tx", &body).await.1["txs"][0]["status"],
        "pending"
    );

    node.set_tx(btc_confirmed(96));
    let reply = gateway.post("/v1/bitcoin/tx", &body).await.1;
    assert_eq!(
        reply["txs"][0],
        json!({"hash": BTC_TXID, "status": "included", "confirmations": 5, "stage": null, "block": 96, "fee": "141"}),
    );

    node.set_tx(btc_confirmed(95));
    let reply = gateway.post("/v1/bitcoin/tx", &body).await.1;
    assert_eq!(
        (&reply["txs"][0]["status"], &reply["txs"][0]["confirmations"]),
        (&json!("finalized"), &json!(6))
    );
}

#[tokio::test]
async fn bitcoin_used_counts_chain_and_mempool_transactions() {
    let esplora = Router::new().route(
        "/address/{address}",
        get(|Path(address): Path<String>| async move {
            let pending = u64::from(address == BTC);
            Json(json!({"chain_stats": {"tx_count": 0}, "mempool_stats": {"tx_count": pending}}))
        }),
    );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let body = json!({"addresses": [BTC, BTC_OTHER]});
    let (status, reply) = gateway.post("/v1/bitcoin/used", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"used": [BTC]}));
    let (status, _) = gateway.post("/v1/ethereum/used", &json!({"addresses": [EVM]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "没有配置这个网络");
}

#[tokio::test]
async fn bitcoin_utxos_mark_the_mempool_with_height_zero() {
    let esplora = Router::new().route(
        "/address/{address}/utxo",
        get(|Path(address): Path<String>| async move {
            assert_eq!(address, BTC);
            Json(json!([
                {"txid": hex_hash('a'), "vout": 1, "value": 50000, "status": {"confirmed": true, "block_height": 840000}},
                {"txid": hex_hash('b'), "vout": 0, "value": 7, "status": {"confirmed": false}},
            ]))
        }),
    );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/utxos", &json!({"addresses": [BTC]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"utxos": [
            {"address": BTC, "txid": hex_hash('a'), "vout": 1, "amount": "50000", "height": 840000},
            {"address": BTC, "txid": hex_hash('b'), "vout": 0, "amount": "7", "height": 0},
        ]}),
    );
}

#[tokio::test]
async fn bitcoin_garbled_utxos_fail_over() {
    let bad = mock(Router::new().route(
        "/address/{address}/utxo",
        get(|| async { Json(json!([{"txid": "x"}])) }),
    ))
    .await;
    let good = mock(Router::new().route("/address/{address}/utxo", get(|| async { Json(json!([])) }))).await;
    let gateway = Gateway::start(&two_upstreams("bitcoin", &bad, &good)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/utxos", &json!({"addresses": [BTC]})).await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"utxos": []})));
}

#[tokio::test]
async fn bitcoin_raw_txs_leave_out_what_the_upstream_lacks() {
    let known = hex_hash('a');
    let expected = known.clone();
    let esplora = Router::new().route(
        "/tx/{txid}/hex",
        get(move |Path(txid): Path<String>| {
            let found = txid == expected;
            async move {
                if found {
                    (StatusCode::OK, "00ff\n")
                } else {
                    (StatusCode::NOT_FOUND, "Transaction not found")
                }
            }
        }),
    );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let body = json!({"txids": [known, hex_hash('b')]});
    let (status, reply) = gateway.post("/v1/bitcoin/raw-txs", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"txs": {(known): "00ff"}}));
}

#[tokio::test]
async fn bitcoin_raw_txs_reject_garbage_from_the_upstream() {
    let bad = mock(Router::new().route("/tx/{txid}/hex", get(|| async { "<html>" }))).await;
    let gateway = Gateway::start(&network_config("bitcoin", &bad)).await;
    let (status, reply) = gateway
        .post("/v1/bitcoin/raw-txs", &json!({"txids": [hex_hash('a')]}))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[tokio::test]
async fn bitcoin_txids_are_checked_before_they_reach_a_url() {
    let (node, url) = Esplora::start(200, "").await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    for bad in ["../../address/x", "ab", &format!("0x{}", hex_hash('a'))] {
        let (status, _) = gateway.post("/v1/bitcoin/raw-txs", &json!({"txids": [bad]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(node.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bitcoin_fees_come_from_the_estimates() {
    let esplora = Router::new().route(
        "/fee-estimates",
        get(|| async { Json(json!({"1": 7.2, "6": 3.0, "144": 0.5})) }),
    );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let (status, reply) = gateway.get_json("/v1/bitcoin/fees").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"kind": "bitcoin", "slow": 1, "medium": 3, "fast": 8}));
}

fn btc_tx(txid: &str, input: &str, outputs: &[(&str, u64)], status: Value) -> Value {
    let vout: Vec<Value> = outputs
        .iter()
        .map(|(a, v)| json!({"scriptpubkey_address": a, "value": v}))
        .collect();
    json!({"txid": txid, "status": status, "vin": [{"prevout": {"scriptpubkey_address": input, "value": 1}}], "vout": vout})
}

#[tokio::test]
async fn bitcoin_history_pages_through_the_chain() {
    let chain_page: Vec<Value> = (0..25)
        .map(|i| {
            let status = json!({"confirmed": true, "block_height": 100 - i, "block_time": 1700000000 - i});
            btc_tx(&format!("{i:064x}"), BTC_OTHER, &[(BTC, 10)], status)
        })
        .collect();
    let first = {
        let mut page = vec![btc_tx(
            &hex_hash('f'),
            BTC,
            &[(BTC_OTHER, 9)],
            json!({"confirmed": false}),
        )];
        page.extend(chain_page);
        page
    };
    let last_seen = format!("{:064x}", 24);
    let expected = last_seen.clone();
    let esplora = Router::new()
        .route("/blocks/tip/height", get(|| async { "100" }))
        .route(
            "/address/{address}/txs",
            get(move || {
                let first = first.clone();
                async move { Json(Value::Array(first)) }
            }),
        )
        .route(
            "/address/{address}/txs/chain/{last}",
            get(move |Path((_, last)): Path<(String, String)>| {
                assert_eq!(last, expected);
                async move {
                    let status = json!({"confirmed": true, "block_height": 50, "block_time": 1600000000});
                    Json(json!([btc_tx(&hex_hash('e'), BTC, &[(BTC_OTHER, 3)], status)]))
                }
            }),
        );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;

    let (status, reply) = gateway
        .post("/v1/bitcoin/history", &json!({"addresses": [BTC], "cursor": null}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = reply["items"].as_array().unwrap();
    assert_eq!(items.len(), 26);
    assert_eq!(items[0]["status"], "pending", "内存池排在最前");
    assert_eq!(items[0]["time"], Value::Null);
    assert_eq!(items[1]["status"], "included");
    assert_eq!(items[25]["status"], "finalized");
    assert_eq!(items[1]["amount"], "10");
    assert_eq!(items[1]["to"], BTC);

    let cursor = reply["cursor"].as_str().expect("还有下一页");
    let (_, next) = gateway
        .post("/v1/bitcoin/history", &json!({"addresses": [BTC], "cursor": cursor}))
        .await;
    assert_eq!(next["items"][0]["hash"], hex_hash('e'));
    assert_eq!(next["items"][0]["from"], BTC);
    assert_eq!(next["cursor"], Value::Null, "不足一页就结束");

    let (_, delta) = gateway
        .post("/v1/bitcoin/history", &json!({"addresses": [BTC], "since": "90"}))
        .await;
    assert_eq!(delta["items"].as_array().unwrap().len(), 11, "内存池加高度 91 到 100");
    assert_eq!(delta["cursor"], Value::Null, "碰到水位就不用再翻页");
}

/// 比特币每个地址都要单独查,公共节点按 IP 限流,所以高度没变时一个地址都不能查.
#[tokio::test]
async fn bitcoin_history_skips_addresses_when_the_tip_has_not_moved() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let esplora = Router::new()
        .route("/blocks/tip/height", get(|| async { "100" }))
        .route(
            "/address/{address}/txs",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { Json(json!([])) }
            }),
        );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;

    let (_, full) = gateway.post("/v1/bitcoin/history", &json!({"addresses": [BTC]})).await;
    assert_eq!(full["checkpoint"], "95", "六个确认之前的区块不可回滚");
    assert_eq!(full["unchanged"], false);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let (_, idle) = gateway
        .post("/v1/bitcoin/history", &json!({"addresses": [BTC], "since": "95"}))
        .await;
    assert_eq!(idle["unchanged"], true);
    assert_eq!(idle["checkpoint"], "95");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "高度没变不查地址");

    let (_, moved) = gateway
        .post("/v1/bitcoin/history", &json!({"addresses": [BTC], "since": "94"}))
        .await;
    assert_eq!(moved["unchanged"], false);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bitcoin_broadcast_posts_the_raw_hex() {
    let (node, url) = Esplora::start(200, BTC_TXID).await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": BTC_RAW})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply, json!({"hash": BTC_TXID, "status": "accepted"}));
    assert_eq!(node.posted.lock().unwrap().as_slice(), [BTC_RAW.to_string()]);
}

#[tokio::test]
async fn bitcoin_broadcast_is_idempotent() {
    for text in [
        r#"sendrawtransaction RPC error: {"code":-27,"message":"Transaction already in block chain"}"#,
        r#"sendrawtransaction RPC error: {"code":-27,"message":"Transaction outputs already in utxo set"}"#,
        r#"sendrawtransaction RPC error: {"code":-26,"message":"txn-already-in-mempool"}"#,
        r#"sendrawtransaction RPC error: {"code":-26,"message":"txn-already-known"}"#,
    ] {
        let (_, url) = Esplora::start(400, text).await;
        let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
        let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": BTC_RAW})).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(reply, json!({"hash": BTC_TXID, "status": "accepted"}));
    }
}

#[tokio::test]
async fn bitcoin_rejection_carries_the_node_text_and_is_not_retried() {
    let text = r#"sendrawtransaction RPC error: {"code":-26,"message":"min relay fee not met"}"#;
    let (first, a) = Esplora::start(400, text).await;
    let (second, b) = Esplora::start(200, BTC_TXID).await;
    let gateway = Gateway::start(&two_upstreams("bitcoin", &a, &b)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": BTC_RAW})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply, json!({"error": {"code": "broadcast_rejected", "message": text}}));
    assert_eq!(first.hits.load(Ordering::SeqCst), 1);
    assert_eq!(second.hits.load(Ordering::SeqCst), 0, "被拒绝的广播不能换上游重试");
}

#[tokio::test]
async fn bitcoin_spent_inputs_are_accepted_when_the_tx_is_on_chain() {
    // 交易上链后找零又被花掉,节点只会报输入缺失.
    let (node, url) = Esplora::start(400, "bad-txns-inputs-missingorspent").await;
    node.set_tx(btc_confirmed(90));
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": BTC_RAW})).await;
    assert_eq!((status, reply["status"].as_str()), (StatusCode::OK, Some("accepted")));
}

#[tokio::test]
async fn bitcoin_server_errors_are_not_rejections() {
    let (first, a) = Esplora::start(502, "bad gateway").await;
    let (second, b) = Esplora::start(200, BTC_TXID).await;
    let gateway = Gateway::start(&two_upstreams("bitcoin", &a, &b)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": BTC_RAW})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
    assert_eq!(first.hits.load(Ordering::SeqCst), 1);
    assert_eq!(second.hits.load(Ordering::SeqCst), 0, "请求可能已经到达节点,不能换上游");
}

#[tokio::test]
async fn malformed_raw_transactions_are_refused_locally() {
    let (node, url) = Esplora::start(200, "").await;
    let gateway = Gateway::start(&network_config("bitcoin", &url)).await;
    let (status, reply) = gateway.post("/v1/bitcoin/broadcast", &json!({"raw": "0011"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&reply), "invalid_request");
    assert_eq!(node.hits.load(Ordering::SeqCst), 0);
}

// ---- EVM ----

fn evm_block(number: u64) -> Value {
    json!({"number": format!("{number:#x}"), "hash": "0x1"})
}

/// 区块标签固定:latest 100,safe 90,finalized 80.
fn tags(method: &str, params: &Value) -> Option<Value> {
    match (method, params[0].as_str()) {
        ("eth_blockNumber", _) => Some(ok(json!("0x64"))),
        ("eth_getBlockByNumber", Some("safe")) => Some(ok(evm_block(90))),
        ("eth_getBlockByNumber", Some("finalized")) => Some(ok(evm_block(80))),
        _ => None,
    }
}

fn evm_receipt(block: u64, status: &str) -> Value {
    json!({"blockNumber": format!("{block:#x}"), "status": status, "gasUsed": "0x5208", "effectiveGasPrice": "0x3b9aca00"})
}

async fn evm_tx_node(receipt: Value, tx: Value) -> (String, Calls) {
    rpc_node(move |method, params| {
        tags(method, params).unwrap_or_else(|| match method {
            "eth_getTransactionReceipt" => ok(receipt.clone()),
            "eth_getTransactionByHash" => ok(tx.clone()),
            other => panic!("{other}"),
        })
    })
    .await
}

async fn evm_status(receipt: Value, tx: Value) -> Value {
    let (url, _) = evm_tx_node(receipt, tx).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.post("/v1/ethereum/tx", &json!({"hashes": [EVM_HASH]})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["txs"][0].clone()
}

#[tokio::test]
async fn evm_tx_reports_each_stage() {
    let tx = json!({"hash": EVM_HASH});
    for (block, status, stage, confirmations) in [
        (100, "included", "latest", 1),
        (90, "included", "safe", 11),
        (80, "finalized", "finalized", 21),
    ] {
        let reply = evm_status(evm_receipt(block, "0x1"), tx.clone()).await;
        assert_eq!(
            reply,
            json!({"hash": EVM_HASH, "status": status, "confirmations": confirmations, "stage": stage, "block": block, "fee": "21000000000000"}),
        );
    }
}

#[tokio::test]
async fn evm_reverted_transactions_are_failed_at_every_stage() {
    for block in [100, 90, 80, 1] {
        let reply = evm_status(evm_receipt(block, "0x0"), json!({"hash": EVM_HASH})).await;
        assert_eq!(reply["status"], "failed", "第 {block} 块");
        assert_eq!(reply["fee"], "21000000000000", "手续费已扣除");
    }
}

#[tokio::test]
async fn evm_tx_pending_and_unknown() {
    let reply = evm_status(Value::Null, json!({"hash": EVM_HASH, "blockNumber": null})).await;
    assert_eq!(reply["status"], "pending");
    let reply = evm_status(Value::Null, Value::Null).await;
    assert_eq!(
        reply,
        json!({"hash": EVM_HASH, "status": "unknown", "confirmations": 0, "stage": null, "block": null, "fee": null})
    );
}

#[tokio::test]
async fn evm_receipt_without_status_is_not_reported_as_success() {
    let mut old = evm_receipt(80, "0x1");
    old.as_object_mut().unwrap().remove("status");
    let (url, _) = evm_tx_node(old, json!({})).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.post("/v1/ethereum/tx", &json!({"hashes": [EVM_HASH]})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[tokio::test]
async fn evm_works_without_the_safe_tag() {
    // Polygon 的公共节点这样回复.
    let (url, _) = rpc_node(|method, params| match (method, params[0].as_str()) {
        ("eth_getBlockByNumber", Some("safe")) => rpc_failure(-32000, "safe block not found"),
        ("eth_getTransactionReceipt", _) => ok(evm_receipt(85, "0x1")),
        ("eth_getTransactionByHash", _) => ok(json!({})),
        _ => tags(method, params).unwrap(),
    })
    .await;
    let gateway = Gateway::start(&network_config("polygon", &url)).await;
    let (status, reply) = gateway.post("/v1/polygon/tx", &json!({"hashes": [EVM_HASH]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&reply["txs"][0]["status"], &reply["txs"][0]["stage"]),
        (&json!("included"), &json!("latest"))
    );
}

#[tokio::test]
async fn evm_without_a_finalized_tag_cannot_claim_finality() {
    let bad = |method: &str, params: &Value| match (method, params[0].as_str()) {
        ("eth_getBlockByNumber", Some("finalized")) => rpc_failure(-32000, "finalized block not found"),
        _ => ok(evm_receipt(1, "0x1")),
    };
    let (a, _) = rpc_node(move |method, params| {
        tags(method, params)
            .filter(|_| params[0] != "finalized")
            .unwrap_or_else(|| bad(method, params))
    })
    .await;
    let (b, _) = evm_tx_node(evm_receipt(1, "0x1"), json!({})).await;
    let gateway = Gateway::start(&network_config("ethereum", &a)).await;
    let (status, _) = gateway.post("/v1/ethereum/tx", &json!({"hashes": [EVM_HASH]})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // 另一个上游支持时切过去.
    let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
    let (status, reply) = gateway.post("/v1/ethereum/tx", &json!({"hashes": [EVM_HASH]})).await;
    assert_eq!(
        (status, &reply["txs"][0]["status"]),
        (StatusCode::OK, &json!("finalized"))
    );
}

#[tokio::test]
async fn evm_large_tx_queries_are_split_and_stay_in_order() {
    let hashes: Vec<String> = (0..100).map(|i| format!("0x{i:064x}")).collect();
    let (url, calls) = rpc_node(move |method, params| {
        tags(method, params).unwrap_or_else(|| {
            let index = usize::from_str_radix(params[0].as_str().unwrap().trim_start_matches("0x"), 16).unwrap() as u64;
            match method {
                "eth_getTransactionReceipt" => ok(evm_receipt(index + 1, "0x1")),
                _ => ok(json!({})),
            }
        })
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.post("/v1/ethereum/tx", &json!({"hashes": hashes})).await;
    assert_eq!(status, StatusCode::OK);
    for (i, tx) in reply["txs"].as_array().unwrap().iter().enumerate() {
        assert_eq!(tx["hash"], hashes[i].as_str());
        assert_eq!(tx["block"], i + 1);
    }
    assert_eq!(calls.count("eth_getTransactionReceipt"), 100);
}

#[tokio::test]
async fn evm_nonce_reads_latest_and_pending() {
    let (url, _) = rpc_node(|method, params| {
        assert_eq!(method, "eth_getTransactionCount");
        ok(json!(if params[1] == "latest" { "0xc" } else { "0xd" }))
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway
        .post("/v1/ethereum/nonce", &json!({"addresses": [EVM, EVM_OTHER]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"nonces": {
            (EVM): {"latest": 12, "pending": 13},
            (EVM_OTHER): {"latest": 12, "pending": 13},
        }}),
    );
}

#[tokio::test]
async fn evm_fees_use_the_fee_history() {
    let (url, _) = rpc_node(|method, params| match method {
        "eth_feeHistory" => {
            assert_eq!(params[1], "latest");
            assert_eq!(params[2], json!([25, 50, 75]));
            ok(json!({"baseFeePerGas": ["0x1", "0x4a817c800"], "reward": [["0x5f5e100", "0x77359400", "0xb2d05e00"]]}))
        }
        "eth_maxPriorityFeePerGas" => ok(json!("0x1")),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.get_json("/v1/ethereum/fees").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({
            "kind": "evm",
            "baseFee": "20000000000",
            "slow": {"maxFee": "25100000000", "priority": "100000000"},
            "medium": {"maxFee": "32000000000", "priority": "2000000000"},
            "fast": {"maxFee": "43000000000", "priority": "3000000000"},
        }),
    );
}

#[tokio::test]
async fn evm_estimate_converts_value_and_reads_gas() {
    let (url, _) = rpc_node(|method, params| {
        assert_eq!(method, "eth_estimateGas");
        assert_eq!(
            params[0],
            json!({"from": EVM, "to": USDT, "value": "0x3e8", "data": "0xa9059cbb"})
        );
        ok(json!("0xfde8"))
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let body = json!({"from": EVM, "to": USDT, "value": "1000", "data": "0xa9059cbb"});
    let (status, reply) = gateway.post("/v1/ethereum/estimate", &body).await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"gas": "65000"})));
}

#[tokio::test]
async fn evm_estimate_revert_is_the_callers_problem_not_an_outage() {
    let (a, first) = rpc_node(|_, _| rpc_failure(3, "execution reverted: transfer amount exceeds balance")).await;
    let (b, second) = rpc_node(|_, _| ok(json!("0x5208"))).await;
    let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
    let (status, reply) = gateway
        .post("/v1/ethereum/estimate", &json!({"from": EVM, "to": USDT}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply,
        json!({"error": {"code": "invalid_request", "message": "execution reverted: transfer amount exceeds balance"}})
    );
    assert_eq!((first.total(), second.total()), (1, 0));
}

#[tokio::test]
async fn evm_estimate_fails_over_on_node_trouble() {
    let (a, _) = rpc_node(|_, _| rpc_failure(-32005, "limit exceeded")).await;
    let (b, _) = rpc_node(|_, _| ok(json!("0x5208"))).await;
    let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
    let (status, reply) = gateway
        .post("/v1/ethereum/estimate", &json!({"from": EVM, "to": USDT}))
        .await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"gas": "21000"})));
}

/// 发送固定结果的 EVM 节点,可以指定这笔交易在链上能不能查到.
async fn evm_broadcast_node(send: Value, on_chain: bool) -> (String, Calls) {
    rpc_node(move |method, params| {
        tags(method, params).unwrap_or_else(|| match method {
            "eth_sendRawTransaction" => {
                assert_eq!(params[0], EVM_RAW);
                send.clone()
            }
            "eth_getTransactionReceipt" if on_chain => ok(evm_receipt(90, "0x1")),
            "eth_getTransactionReceipt" | "eth_getTransactionByHash" => ok(Value::Null),
            other => panic!("{other}"),
        })
    })
    .await
}

async fn evm_broadcast(send: Value, on_chain: bool) -> (StatusCode, Value, Calls) {
    let (url, calls) = evm_broadcast_node(send, on_chain).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
    (status, reply, calls)
}

#[tokio::test]
async fn evm_broadcast_returns_the_locally_computed_hash() {
    let (status, reply, calls) = evm_broadcast(ok(json!(EVM_HASH)), false).await;
    assert_eq!(
        (status, reply),
        (StatusCode::OK, json!({"hash": EVM_HASH, "status": "accepted"}))
    );
    assert_eq!(calls.count("eth_sendRawTransaction"), 1);
}

#[tokio::test]
async fn evm_broadcast_is_idempotent() {
    for message in [
        "already known",
        "AlreadyKnown",
        "ALREADY_KNOWN",
        "Known transaction",
        "transaction already imported",
    ] {
        let (status, reply, _) = evm_broadcast(rpc_failure(-32000, message), false).await;
        assert_eq!(status, StatusCode::OK, "{message}");
        assert_eq!(reply, json!({"hash": EVM_HASH, "status": "accepted"}));
    }
}

#[tokio::test]
async fn evm_nonce_too_low_is_success_only_if_this_tx_is_on_chain() {
    let (status, reply, _) = evm_broadcast(rpc_failure(-32000, "nonce too low"), true).await;
    assert_eq!((status, reply["hash"].as_str()), (StatusCode::OK, Some(EVM_HASH)));

    // 同一个 nonce 被另一笔交易用掉了,这是真正的拒绝.
    let (status, reply, _) = evm_broadcast(rpc_failure(-32000, "nonce too low"), false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply,
        json!({"error": {"code": "broadcast_rejected", "message": "nonce too low"}})
    );
}

#[tokio::test]
async fn evm_rejection_is_not_retried_on_another_upstream() {
    let (a, first) = evm_broadcast_node(rpc_failure(-32000, "insufficient funds for gas * price + value"), false).await;
    let (b, second) = evm_broadcast_node(ok(json!(EVM_HASH)), false).await;
    let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&reply), "broadcast_rejected");
    assert_eq!(reply["error"]["message"], "insufficient funds for gas * price + value");
    assert_eq!(first.count("eth_sendRawTransaction"), 1);
    assert_eq!(second.total(), 0);
}

#[tokio::test]
async fn evm_node_trouble_during_broadcast_is_unknown_and_stays_put() {
    for failure in [
        rpc_failure(-32005, "limit exceeded"),
        rpc_failure(-32603, "internal error"),
    ] {
        let (a, first) = evm_broadcast_node(failure, false).await;
        let (b, second) = evm_broadcast_node(ok(json!(EVM_HASH)), false).await;
        let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
        let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(&reply), "upstream_unavailable");
        assert_eq!(first.count("eth_sendRawTransaction"), 1);
        assert_eq!(second.total(), 0);
    }
}

#[tokio::test]
async fn evm_broadcast_moves_on_only_after_a_connection_failure() {
    let dead = dead_url().await;
    let (good, calls) = evm_broadcast_node(ok(json!(EVM_HASH)), false).await;
    let gateway = Gateway::start(&two_upstreams("ethereum", &dead, &good)).await;
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
    assert_eq!((status, reply["status"].as_str()), (StatusCode::OK, Some("accepted")));
    assert_eq!(calls.count("eth_sendRawTransaction"), 1);
}

#[tokio::test]
async fn broadcast_with_every_upstream_unreachable_is_unknown() {
    let (a, b) = (dead_url().await, dead_url().await);
    let gateway = Gateway::start(&two_upstreams("ethereum", &a, &b)).await;
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[tokio::test]
async fn broadcast_timeout_is_neither_success_nor_failure_and_is_not_resent() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let hanging = mock(Router::new().route(
        "/",
        post(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                sleep(Duration::from_secs(60)).await;
                StatusCode::OK
            }
        }),
    ))
    .await;
    let (good, calls) = evm_broadcast_node(ok(json!(EVM_HASH)), false).await;
    let config = format!(
        "upstream_timeout_ms = 200\n{}",
        two_upstreams("ethereum", &hanging, &good)
    );
    let gateway = Gateway::start(&config).await;
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": EVM_RAW})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(calls.total(), 0, "超时的交易可能已经到达节点,不能发给下一个上游");
}

#[tokio::test]
async fn broadcast_accepts_bodies_larger_than_the_query_limit() {
    let (url, calls) = rpc_node(|_, _| ok(json!(EVM_HASH))).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let big = format!("0x02{}", "ab".repeat(200_000));
    let (status, _) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": big})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(calls.count("eth_sendRawTransaction"), 1);

    let huge = format!("0x02{}", "ab".repeat(600_000));
    let (status, reply) = gateway.post("/v1/ethereum/broadcast", &json!({"raw": huge})).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&reply), "invalid_request");
}

fn indexer_row(hash: &str, value: &str, is_error: &str, block: u64) -> Value {
    json!({
        "hash": hash, "from": EVM, "to": EVM_OTHER, "value": value, "isError": is_error,
        "blockNumber": block.to_string(), "timeStamp": (1700000000 + block).to_string(), "contractAddress": "",
    })
}

/// Etherscan 兼容接口.`rows` 按 action 和 page 查表.
async fn indexer(rows: HashMap<(String, String), Value>) -> String {
    let handler = move |Query(query): Query<HashMap<String, String>>| {
        assert_eq!(query["module"], "account");
        assert_eq!(query["sort"], "desc");
        let key = (query["action"].clone(), query["page"].clone());
        let result = rows.get(&key).cloned().unwrap_or(json!([]));
        async move { Json(json!({"status": "1", "message": "OK", "result": result})) }
    };
    mock(Router::new().route("/", get(handler))).await
}

async fn evm_with_indexer(indexer_url: &str) -> Gateway {
    let (node, _) = rpc_node(|method, params| tags(method, params).unwrap()).await;
    Gateway::start(&format!(
        "[networks.ethereum]\nupstreams = ['{node}']\ntokens = ['{USDT}']\nindexer = '{indexer_url}?chainid=1'\n"
    ))
    .await
}

#[tokio::test]
async fn evm_history_merges_native_and_listed_tokens() {
    let mut token = indexer_row(&evm_hash('c'), "2500000", "0", 79);
    token["contractAddress"] = json!(USDT.to_lowercase());
    let mut spam = indexer_row(&evm_hash('d'), "1", "0", 78);
    spam["contractAddress"] = json!("0x1111111111111111111111111111111111111111");
    let rows = HashMap::from([
        (
            ("txlist".to_string(), "1".to_string()),
            json!([
                indexer_row(&evm_hash('a'), "5", "0", 95),
                indexer_row(&evm_hash('b'), "0", "1", 85),
                indexer_row(&evm_hash('e'), "0", "0", 84),
            ]),
        ),
        (("tokentx".to_string(), "1".to_string()), json!([token, spam])),
    ]);
    let url = indexer(rows).await;
    let gateway = evm_with_indexer(&url).await;
    let (status, reply) = gateway.post("/v1/ethereum/history", &json!({"addresses": [EVM]})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["cursor"], Value::Null);
    let summary: Vec<(&str, &str, &str)> = reply["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["asset"].as_str().unwrap(),
                i["amount"].as_str().unwrap(),
                i["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("native", "5", "included"),
            ("native", "0", "failed"),
            (USDT, "2500000", "finalized")
        ],
    );
}

#[tokio::test]
async fn evm_history_pages_each_stream_separately() {
    let full: Vec<Value> = (0..50)
        .map(|i| indexer_row(&format!("0x{i:064x}"), "1", "0", 70))
        .collect();
    let rows = HashMap::from([
        (("txlist".to_string(), "1".to_string()), Value::Array(full)),
        (
            ("txlist".to_string(), "2".to_string()),
            json!([indexer_row(&evm_hash('a'), "2", "0", 60)]),
        ),
    ]);
    let url = indexer(rows).await;
    let gateway = evm_with_indexer(&url).await;
    let (_, reply) = gateway.post("/v1/ethereum/history", &json!({"addresses": [EVM]})).await;
    assert_eq!(reply["items"].as_array().unwrap().len(), 50);
    let cursor = reply["cursor"].as_str().unwrap();
    assert_eq!(cursor, r#"{"0:txlist":"2"}"#, "只有写满一页的流才有下文");
    let (_, next) = gateway
        .post("/v1/ethereum/history", &json!({"addresses": [EVM], "cursor": cursor}))
        .await;
    assert_eq!(next["items"][0]["hash"], evm_hash('a'));
    assert_eq!(next["cursor"], Value::Null);
}

#[tokio::test]
async fn evm_history_without_an_indexer_is_an_error_not_an_empty_list() {
    let (url, _) = rpc_node(|method, params| tags(method, params).unwrap()).await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    let (status, reply) = gateway.post("/v1/ethereum/history", &json!({"addresses": [EVM]})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[tokio::test]
async fn evm_history_rate_limit_text_is_an_upstream_failure() {
    let url = mock(Router::new().route(
        "/",
        get(|| async { Json(json!({"status": "0", "message": "NOTOK", "result": "Max rate limit reached"})) }),
    ))
    .await;
    let gateway = evm_with_indexer(&url).await;
    let (status, _) = gateway.post("/v1/ethereum/history", &json!({"addresses": [EVM]})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn evm_history_empty_result_is_an_empty_page() {
    let url = mock(Router::new().route(
        "/",
        get(|| async { Json(json!({"status": "0", "message": "No transactions found", "result": []})) }),
    ))
    .await;
    let gateway = evm_with_indexer(&url).await;
    let (status, reply) = gateway.post("/v1/ethereum/history", &json!({"addresses": [EVM]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["items"], json!([]));
    assert_eq!(reply["cursor"], Value::Null);
    assert!(reply["checkpoint"].is_string());
    assert_eq!(reply["unchanged"], false);
}

#[tokio::test]
async fn evm_history_starts_after_the_checkpoint() {
    let url = mock(Router::new().route(
        "/",
        get(|Query(query): Query<HashMap<String, String>>| async move {
            assert_eq!(query["startblock"], "81");
            Json(json!({"status": "0", "message": "No transactions found", "result": []}))
        }),
    ))
    .await;
    let gateway = evm_with_indexer(&url).await;
    let (status, reply) = gateway
        .post("/v1/ethereum/history", &json!({"addresses": [EVM], "since": "80"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
}

// ---- TRON ----

fn tron_address(seed: u8) -> String {
    let mut payload = [seed; 21];
    payload[0] = 0x41;
    bitcoin::base58::encode_check(&payload)
}

fn tron_block(number: u64) -> Value {
    json!({
        "blockID": format!("{number:016x}{}", "ab".repeat(24)),
        "block_header": {"raw_data": {"number": number, "timestamp": 1790000000000u64}},
    })
}

#[derive(Clone, Default)]
struct TronNode {
    solid: Arc<Mutex<Value>>,
    info: Arc<Mutex<Value>>,
    tx: Arc<Mutex<Value>>,
    pending: Arc<Mutex<Value>>,
    broadcast: Arc<Mutex<Value>>,
    keys: Arc<Mutex<Vec<String>>>,
    sent: Arc<AtomicUsize>,
    /// 已激活的账户.
    activated: Arc<Mutex<Vec<String>>>,
}

type TronState = State<TronNode>;

async fn tron_post(
    State(node): TronState,
    Path(path): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    node.keys.lock().unwrap().push(
        headers
            .get("tron-pro-api-key")
            .map_or("", |k| k.to_str().unwrap())
            .to_string(),
    );
    let reply = match path.as_str() {
        "walletsolidity/gettransactioninfobyid" => node.solid.lock().unwrap().clone(),
        "wallet/gettransactioninfobyid" => node.info.lock().unwrap().clone(),
        "wallet/gettransactionbyid" => node.tx.lock().unwrap().clone(),
        "wallet/gettransactionfrompending" => node.pending.lock().unwrap().clone(),
        "wallet/getaccount" => {
            let known = node
                .activated
                .lock()
                .unwrap()
                .iter()
                .any(|a| body["address"] == a.as_str());
            if known {
                json!({"address": body["address"]})
            } else {
                json!({})
            }
        }
        "wallet/getaccountresource" => {
            json!({"freeNetLimit": 600, "freeNetUsed": 100, "EnergyLimit": 90, "EnergyUsed": 30})
        }
        "wallet/broadcasthex" => {
            node.sent.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body["transaction"], TRON_RAW);
            node.broadcast.lock().unwrap().clone()
        }
        "wallet/triggerconstantcontract" => {
            assert_eq!(body["data"], "a9059cbb");
            json!({"result": {"result": true}, "energy_used": 64285, "transaction": {"ret": [{}], "raw_data_hex": "ab".repeat(205)}})
        }
        other => panic!("{other}"),
    };
    Json(reply)
}

async fn tron_get(Path(path): Path<String>) -> Json<Value> {
    Json(match path.as_str() {
        "wallet/getnowblock" => tron_block(100),
        "walletsolidity/getnowblock" => tron_block(0x151a),
        "wallet/getchainparameters" => json!({"chainParameter": [
            {"key": "getEnergyFee", "value": 420},
            {"key": "getTransactionFee", "value": 1000},
            {"key": "getCreateAccountFee", "value": 100000},
            {"key": "getCreateNewAccountFeeInSystemContract", "value": 1000000},
        ]}),
        other => panic!("{other}"),
    })
}

impl TronNode {
    async fn start() -> (Self, String) {
        let node = Self::default();
        *node.solid.lock().unwrap() = json!({});
        *node.info.lock().unwrap() = json!({});
        *node.tx.lock().unwrap() = json!({});
        *node.pending.lock().unwrap() = json!({});
        let router = Router::new()
            .route("/{*path}", post(tron_post).get(tron_get))
            .with_state(node.clone());
        (node, mock(router).await)
    }
}

async fn tron_gateway(url: &str) -> Gateway {
    Gateway::start(&format!(
        "[networks.tron]\nupstreams = ['{url}']\ntron_api_key = 'secret-key'\n"
    ))
    .await
}

async fn tron_status(gateway: &Gateway) -> Value {
    let (status, reply) = gateway.post("/v1/tron/tx", &json!({"hashes": [hex_hash('a')]})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["txs"][0].clone()
}

#[tokio::test]
async fn tron_tx_walks_through_the_stages() {
    let (node, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    assert_eq!(tron_status(&gateway).await["status"], "unknown");

    *node.pending.lock().unwrap() = json!({"txID": hex_hash('a')});
    assert_eq!(tron_status(&gateway).await["status"], "pending");

    let info = json!({"id": hex_hash('a'), "blockNumber": 95, "fee": 1100000});
    *node.info.lock().unwrap() = info.clone();
    *node.tx.lock().unwrap() = json!({"ret": [{"contractRet": "SUCCESS"}]});
    assert_eq!(
        tron_status(&gateway).await,
        json!({"hash": hex_hash('a'), "status": "included", "confirmations": 6, "stage": "included", "block": 95, "fee": "1100000"}),
    );

    *node.solid.lock().unwrap() = info;
    let tx = tron_status(&gateway).await;
    assert_eq!(
        (&tx["status"], &tx["stage"]),
        (&json!("finalized"), &json!("finalized"))
    );
}

#[tokio::test]
async fn tron_contract_failures_are_failed_even_after_confirmation() {
    let (node, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    let info = json!({"id": hex_hash('a'), "blockNumber": 95, "fee": 27000000, "result": "FAILED"});
    *node.info.lock().unwrap() = info.clone();
    for result in ["REVERT", "OUT_OF_ENERGY"] {
        *node.tx.lock().unwrap() = json!({"ret": [{"contractRet": result}]});
        let tx = tron_status(&gateway).await;
        assert_eq!(
            (&tx["status"], &tx["stage"]),
            (&json!("failed"), &json!("included")),
            "{result}"
        );
    }
    *node.solid.lock().unwrap() = info;
    let tx = tron_status(&gateway).await;
    assert_eq!(
        (&tx["status"], &tx["stage"], &tx["fee"]),
        (&json!("failed"), &json!("finalized"), &json!("27000000"))
    );
}

#[tokio::test]
async fn tron_resources_combine_two_endpoints() {
    let (node, url) = TronNode::start().await;
    let (active, empty) = (tron_address(1), tron_address(2));
    node.activated.lock().unwrap().push(active.clone());
    let gateway = tron_gateway(&url).await;
    let (status, reply) = gateway
        .post("/v1/tron/resources", &json!({"addresses": [active, empty]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"resources": {
            (active): {"bandwidth": 500, "energy": 60, "activated": true},
            (empty): {"bandwidth": 500, "energy": 60, "activated": false},
        }}),
    );
}

#[tokio::test]
async fn tron_ref_block_is_the_latest_confirmed_block() {
    let (_, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    let (status, reply) = gateway.get_json("/v1/tron/ref-block").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reply,
        json!({"refBlockBytes": "151a", "refBlockHash": "abababababababab", "timestamp": 1790000000000u64})
    );
}

#[tokio::test]
async fn tron_fees_read_the_chain_parameters() {
    let (_, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    let (_, reply) = gateway.get_json("/v1/tron/fees").await;
    assert_eq!(
        reply,
        json!({"kind": "tron", "energyPrice": 420, "bandwidthPrice": 1000})
    );
}

#[tokio::test]
async fn tron_estimate_charges_activation_only_for_new_accounts() {
    let (node, url) = TronNode::start().await;
    let (from, known, fresh) = (tron_address(1), tron_address(2), tron_address(3));
    node.activated.lock().unwrap().push(known.clone());
    let gateway = tron_gateway(&url).await;
    let (status, reply) = gateway
        .post(
            "/v1/tron/estimate",
            &json!({"from": from, "to": known, "contract": "", "data": ""}),
        )
        .await;
    assert_eq!(
        (status, reply),
        (
            StatusCode::OK,
            json!({"energy": "0", "bandwidth": "268", "activationFee": "0"})
        )
    );
    let (_, reply) = gateway
        .post("/v1/tron/estimate", &json!({"from": from, "to": fresh}))
        .await;
    assert_eq!(
        reply,
        json!({"energy": "0", "bandwidth": "268", "activationFee": "1100000"})
    );
}

#[tokio::test]
async fn tron_estimate_simulates_contract_calls() {
    let (_, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    let body = json!({"from": tron_address(1), "to": tron_address(2), "contract": TRON, "data": "a9059cbb"});
    let (status, reply) = gateway.post("/v1/tron/estimate", &body).await;
    assert_eq!(
        (status, reply),
        (
            StatusCode::OK,
            json!({"energy": "64285", "bandwidth": "345", "activationFee": "0"})
        )
    );

    let body = json!({"from": tron_address(1), "to": tron_address(2), "contract": TRON});
    let (status, reply) = gateway.post("/v1/tron/estimate", &body).await;
    assert_eq!(
        (status, error_code(&reply)),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

#[tokio::test]
async fn tron_estimate_revert_is_reported_with_the_node_text() {
    let revert = json!({"result": {"result": true, "message": "REVERT opcode executed"}, "energy_used": 8624, "transaction": {"ret": [{"ret": "FAILED"}]}});
    let tron = Router::new().route(
        "/wallet/triggerconstantcontract",
        post(move || {
            let r = revert.clone();
            async move { Json(r) }
        }),
    );
    let url = mock(tron).await;
    let gateway = Gateway::start(&network_config("tron", &url)).await;
    let body = json!({"from": tron_address(1), "to": tron_address(2), "contract": TRON, "data": "a9059cbb"});
    let (status, reply) = gateway.post("/v1/tron/estimate", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply,
        json!({"error": {"code": "invalid_request", "message": "REVERT opcode executed"}})
    );
}

#[tokio::test]
async fn tron_broadcast_outcomes() {
    let (node, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    let cases = [
        (json!({"result": true, "code": "SUCCESS"}), StatusCode::OK, None),
        (
            json!({"result": false, "code": "DUP_TRANSACTION_ERROR", "message": "dup"}),
            StatusCode::OK,
            None,
        ),
        (
            json!({"result": false, "code": "SIGERROR", "message": "validate signature error"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("broadcast_rejected"),
        ),
        (
            json!({"result": false, "code": "SERVER_BUSY", "message": "busy"}),
            StatusCode::SERVICE_UNAVAILABLE,
            Some("upstream_unavailable"),
        ),
    ];
    for (reply, expected, code) in cases {
        *node.broadcast.lock().unwrap() = reply.clone();
        let (status, body) = gateway.post("/v1/tron/broadcast", &json!({"raw": TRON_RAW})).await;
        assert_eq!(status, expected, "{reply}");
        match code {
            Some(code) => assert_eq!(error_code(&body), code, "{reply}"),
            None => assert_eq!(body["status"], "accepted"),
        }
    }
}

#[tokio::test]
async fn tron_expired_rebroadcast_succeeds_if_it_landed() {
    let (node, url) = TronNode::start().await;
    *node.broadcast.lock().unwrap() =
        json!({"result": false, "code": "TRANSACTION_EXPIRATION_ERROR", "message": "expired"});
    *node.info.lock().unwrap() = json!({"blockNumber": 95});
    *node.tx.lock().unwrap() = json!({"ret": [{"contractRet": "SUCCESS"}]});
    let gateway = tron_gateway(&url).await;
    let (status, body) = gateway.post("/v1/tron/broadcast", &json!({"raw": TRON_RAW})).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::OK, Some("accepted")));
}

#[tokio::test]
async fn tron_api_key_goes_on_every_upstream_request() {
    let (node, url) = TronNode::start().await;
    let gateway = tron_gateway(&url).await;
    tron_status(&gateway).await;
    let keys = node.keys.lock().unwrap().clone();
    assert!(!keys.is_empty());
    assert!(keys.iter().all(|key| key == "secret-key"), "{keys:?}");
}

async fn solid_block() -> Json<Value> {
    Json(json!({"block_header": {"raw_data": {"number": 7, "timestamp": 1780000000000u64}}}))
}

#[tokio::test]
async fn tron_history_starts_after_the_checkpoint() {
    let owner = tron_address(0x41);
    let trongrid = Router::new()
        .route(
            "/v1/accounts/{address}/transactions",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query["min_timestamp"], "1770000000001");
                Json(json!({"success": true, "meta": {}, "data": []}))
            }),
        )
        .route("/walletsolidity/getnowblock", post(solid_block));
    let url = mock(trongrid).await;
    let gateway = Gateway::start(&network_config("tron", &url)).await;
    let (status, reply) = gateway
        .post(
            "/v1/tron/history",
            &json!({"addresses": [owner], "since": "1770000000000"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["checkpoint"], "1780000000000");
}

#[tokio::test]
async fn tron_history_merges_transfers_and_tokens() {
    let token = tron_address(9);
    let owner = tron_address(1);
    let expected_token = token.clone();
    let rows_token = token.clone();
    let trongrid = Router::new()
        .route(
            "/v1/accounts/{address}/transactions",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(query["only_confirmed"], "true");
                assert_eq!(query["search_internal"], "false");
                Json(json!({"success": true, "meta": {}, "data": [{
                    "txID": hex_hash('a'),
                    "block_timestamp": 1790000000000u64,
                    "ret": [{"contractRet": "SUCCESS"}],
                    "raw_data": {"contract": [{
                        "type": "TransferContract",
                        "parameter": {"value": {
                            "amount": 7,
                            "owner_address": "4192689ecfbfcdffa3444961571f2b822d3967d0aa",
                            "to_address": "4188403ac26730e33164eeac403291b48f300b782f",
                        }},
                    }]},
                }]}))
            }),
        )
        .route(
            "/v1/accounts/{address}/transactions/trc20",
            get(move |Query(query): Query<HashMap<String, String>>| {
                assert_eq!(query["contract_address"], expected_token);
                let has_next = !query.contains_key("fingerprint");
                let token = rows_token.clone();
                async move {
                    let meta = if has_next {
                        json!({"fingerprint": "abc=_-"})
                    } else {
                        json!({})
                    };
                    Json(json!({"success": true, "meta": meta, "data": [{
                        "transaction_id": hex_hash('b'),
                        "block_timestamp": 1780000000000u64,
                        "from": "a", "to": "b", "type": "Transfer", "value": "2500000",
                        "token_info": {"address": token},
                    }]}))
                }
            }),
        );
    let url = mock(trongrid.route("/walletsolidity/getnowblock", post(solid_block))).await;
    let config = format!("[networks.tron]\nupstreams = ['{url}']\ntokens = ['{token}']");
    let gateway = Gateway::start(&config).await;
    let (status, reply) = gateway.post("/v1/tron/history", &json!({"addresses": [owner]})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(
        reply["items"],
        json!([
            {"hash": hex_hash('a'), "time": 1790000000, "from": "TPKLzDVGcSdp36toUXqAtkrmxKsXq7Eahm", "to": "TNPdqto8HiuMzoG7Vv9wyyYhWzCojLeHAF", "asset": "native", "amount": "7", "status": "finalized"},
            {"hash": hex_hash('b'), "time": 1780000000, "from": "a", "to": "b", "asset": token, "amount": "2500000", "status": "finalized"},
        ]),
    );
    let cursor = reply["cursor"].as_str().expect("代币流还有下一页");
    let (_, next) = gateway
        .post("/v1/tron/history", &json!({"addresses": [owner], "cursor": cursor}))
        .await;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["cursor"], Value::Null);
}

// ---- Solana ----

async fn solana_status(entry: Value) -> Value {
    let (url, _) = rpc_node(move |method, _| match method {
        "getSignatureStatuses" => ok(json!({"context": {"slot": 100}, "value": [entry]})),
        "getTransaction" => ok(json!({"meta": {"fee": 5000, "err": null}})),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, reply) = gateway
        .post("/v1/solana/tx", &json!({"hashes": [solana_signature(1)]}))
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["txs"][0].clone()
}

#[tokio::test]
async fn solana_commitment_maps_to_stage_and_status() {
    let entry = |level: &str| json!({"slot": 90, "confirmations": 3, "err": null, "confirmationStatus": level});
    let tx = solana_status(entry("processed")).await;
    assert_eq!(
        (&tx["status"], &tx["stage"], &tx["fee"]),
        (&json!("included"), &json!("processed"), &Value::Null)
    );
    let tx = solana_status(entry("confirmed")).await;
    assert_eq!(
        (&tx["status"], &tx["stage"], &tx["fee"]),
        (&json!("included"), &json!("confirmed"), &json!("5000"))
    );
    let finalized = json!({"slot": 90, "confirmations": null, "err": null, "confirmationStatus": "finalized"});
    let tx = solana_status(finalized).await;
    assert_eq!(
        tx,
        json!({"hash": solana_signature(1), "status": "finalized", "confirmations": 10, "stage": "finalized", "block": 90, "fee": "5000"}),
    );
}

#[tokio::test]
async fn solana_err_is_failed_even_when_finalized() {
    let err = json!({"InstructionError": [1, {"Custom": 6001}]});
    for level in ["processed", "confirmed", "finalized"] {
        let entry = json!({"slot": 90, "confirmations": null, "err": err, "confirmationStatus": level});
        let tx = solana_status(entry).await;
        assert_eq!((&tx["status"], &tx["stage"]), (&json!("failed"), &json!(level)));
    }
}

#[tokio::test]
async fn solana_unknown_signature_and_a_missing_fee() {
    assert_eq!(solana_status(Value::Null).await["status"], "unknown");

    // 手续费取不到不影响状态.
    let (url, _) = rpc_node(|method, _| match method {
        "getSignatureStatuses" => ok(json!({"context": {"slot": 100}, "value": [
            {"slot": 90, "confirmations": 1, "err": null, "confirmationStatus": "confirmed"},
        ]})),
        _ => rpc_failure(-32015, "Transaction version (2) is not supported"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, reply) = gateway
        .post("/v1/solana/tx", &json!({"hashes": [solana_signature(1)]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&reply["txs"][0]["status"], &reply["txs"][0]["fee"]),
        (&json!("included"), &Value::Null)
    );
}

#[tokio::test]
async fn solana_status_list_of_the_wrong_length_is_an_upstream_failure() {
    let (url, _) = rpc_node(|_, _| ok(json!({"context": {"slot": 1}, "value": []}))).await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, _) = gateway
        .post("/v1/solana/tx", &json!({"hashes": [solana_signature(1)]}))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn solana_nonce_account_reads_the_parsed_account() {
    let missing = solana_address(2);
    let target = missing.clone();
    let (url, _) = rpc_node(move |method, params| {
        assert_eq!(method, "getAccountInfo");
        assert_eq!(params[1]["commitment"], "confirmed", "要读到最新的 nonce");
        let value = if params[0] == target.as_str() {
            Value::Null
        } else {
            json!({"lamports": 1447680, "data": {"program": "nonce", "parsed": {
                "type": "initialized",
                "info": {"authority": "a", "blockhash": "NonceValue"},
            }}})
        };
        ok(json!({"context": {"slot": 1}, "value": value}))
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, reply) = gateway
        .post("/v1/solana/nonce-account", &json!({"address": solana_address(1)}))
        .await;
    assert_eq!(
        (status, reply),
        (
            StatusCode::OK,
            json!({"exists": true, "nonce": "NonceValue", "lamports": "1447680"})
        )
    );
    let (status, reply) = gateway
        .post("/v1/solana/nonce-account", &json!({"address": missing}))
        .await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"exists": false})));
    let (status, reply) = gateway.post("/v1/solana/nonce-account", &json!({"address": EVM})).await;
    assert_eq!(
        (status, error_code(&reply)),
        (StatusCode::BAD_REQUEST, "invalid_address")
    );
}

fn solana_address(seed: u8) -> String {
    bitcoin::base58::encode(&[seed; 32])
}

#[tokio::test]
async fn solana_blockhash_and_fees() {
    let (url, _) = rpc_node(|method, _| match method {
        "getLatestBlockhash" => {
            ok(json!({"context": {"slot": 1}, "value": {"blockhash": "Hash", "lastValidBlockHeight": 9}}))
        }
        "getRecentPrioritizationFees" => ok(json!([{"slot": 1, "prioritizationFee": 0}])),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana-devnet", &url)).await;
    let (status, reply) = gateway.get_json("/v1/solana-devnet/blockhash").await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"blockhash": "Hash"})));
    let (status, reply) = gateway.get_json("/v1/solana-devnet/fees").await;
    assert_eq!(
        (status, reply),
        (
            StatusCode::OK,
            json!({"kind": "solana", "slow": 0, "medium": 1000, "fast": 100000})
        )
    );
}

#[tokio::test]
async fn solana_estimate_looks_for_the_token_account() {
    let (with, without) = (solana_address(1), solana_address(2));
    let holder = with.clone();
    let (url, _) = rpc_node(move |method, params| match method {
        "getTokenAccountsByOwner" => {
            assert_eq!(params[1]["mint"], SOLANA_MINT);
            let accounts = if params[0] == holder.as_str() {
                json!([{"pubkey": "x"}])
            } else {
                json!([])
            };
            ok(json!({"context": {"slot": 1}, "value": accounts}))
        }
        "getMinimumBalanceForRentExemption" => {
            assert_eq!(params[0], 165);
            ok(json!(1488440))
        }
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let body = |recipient: &str, mint: &str| json!({"recipient": recipient, "mint": mint});

    let (status, reply) = gateway.post("/v1/solana/estimate", &body(&with, SOLANA_MINT)).await;
    assert_eq!(
        (status, reply),
        (StatusCode::OK, json!({"recipientTokenAccount": true, "rent": "0"}))
    );
    let (_, reply) = gateway.post("/v1/solana/estimate", &body(&without, SOLANA_MINT)).await;
    assert_eq!(reply, json!({"recipientTokenAccount": false, "rent": "1488440"}));
    // 原生币不需要代币账户,也不用问节点.
    let (_, reply) = gateway.post("/v1/solana/estimate", &body(&without, "")).await;
    assert_eq!(reply, json!({"recipientTokenAccount": true, "rent": "0"}));
}

async fn solana_broadcast(reply: Value, status_entry: Value) -> (StatusCode, Value, Calls) {
    let (url, calls) = rpc_node(move |method, params| match method {
        "sendTransaction" => {
            assert_eq!(params[1]["encoding"], "base64");
            reply.clone()
        }
        "getSignatureStatuses" => ok(json!({"context": {"slot": 100}, "value": [status_entry]})),
        "getTransaction" => ok(json!({"meta": {"fee": 5000}})),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, body) = gateway
        .post("/v1/solana/broadcast", &json!({"raw": solana_raw()}))
        .await;
    (status, body, calls)
}

#[tokio::test]
async fn solana_broadcast_outcomes() {
    let signature = bitcoin::base58::encode(&[7u8; 64]);
    let (status, reply, _) = solana_broadcast(ok(json!(signature)), Value::Null).await;
    assert_eq!(
        (status, reply),
        (StatusCode::OK, json!({"hash": signature, "status": "accepted"}))
    );

    let processed = rpc_failure(
        -32002,
        "Transaction simulation failed: This transaction has already been processed",
    );
    let (status, reply, _) = solana_broadcast(processed, Value::Null).await;
    assert_eq!(
        (status, reply["hash"].as_str()),
        (StatusCode::OK, Some(signature.as_str()))
    );

    let rejected = rpc_failure(-32002, "Transaction simulation failed: Blockhash not found");
    let (status, reply, _) = solana_broadcast(rejected.clone(), Value::Null).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        reply["error"]["message"],
        "Transaction simulation failed: Blockhash not found"
    );

    // nonce 已经推进,这笔交易其实上链了.
    let landed = json!({"slot": 90, "confirmations": null, "err": null, "confirmationStatus": "finalized"});
    let (status, _, _) = solana_broadcast(rejected, landed).await;
    assert_eq!(status, StatusCode::OK);

    let (status, reply, _) = solana_broadcast(rpc_failure(-32005, "Node is unhealthy"), Value::Null).await;
    assert_eq!(
        (status, error_code(&reply)),
        (StatusCode::SERVICE_UNAVAILABLE, "upstream_unavailable")
    );
}

#[tokio::test]
async fn solana_history_skips_signatures_before_the_checkpoint() {
    let owner = solana_address(1);
    let (fresh, old) = (solana_signature(1), solana_signature(2));
    let (url, calls) = rpc_node(move |method, _| match method {
        "getSlot" => ok(json!(500)),
        "getSignaturesForAddress" => ok(json!([
            {"signature": fresh, "slot": 120, "confirmationStatus": "finalized", "err": null},
            {"signature": old, "slot": 100, "confirmationStatus": "finalized", "err": null},
        ])),
        "getTransaction" => ok(json!({
            "blockTime": 1700000000,
            "transaction": {"message": {"accountKeys": [{"pubkey": solana_address(1)}]}},
            "meta": {"err": null, "fee": 5000, "preBalances": [10000], "postBalances": [5000]},
        })),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    let (status, reply) = gateway
        .post("/v1/solana/history", &json!({"addresses": [owner], "since": "110"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["checkpoint"], "500");
    assert_eq!(calls.count("getTransaction"), 1, "水位之前的签名不取详情");
}

#[tokio::test]
async fn solana_history_reads_owner_and_token_accounts() {
    let owner = solana_address(1);
    let other = solana_address(2);
    let mint = solana_address(9);
    let token_account = solana_address(5);
    let (sig_a, sig_b) = (solana_signature(1), solana_signature(2));
    let (o, p, m, t, a, b) = (
        owner.clone(),
        other.clone(),
        mint.clone(),
        token_account.clone(),
        sig_a.clone(),
        sig_b.clone(),
    );
    let (url, _) = rpc_node(move |method, params| match method {
        "getSlot" => ok(json!(500)),
        "getTokenAccountsByOwner" => ok(json!({"context": {"slot": 1}, "value": [{"pubkey": t}]})),
        "getSignaturesForAddress" => {
            let entry =
                |signature: &str| json!({"signature": signature, "confirmationStatus": "finalized", "err": null});
            ok(if params[0] == t.as_str() {
                json!([entry(&b)])
            } else {
                json!([entry(&a), entry(&b)])
            })
        }
        "getTransaction" => {
            // 手续费由第一个账户支付.
            let (keys, meta) = if params[0] == a.as_str() {
                let meta =
                    json!({"err": null, "fee": 5000, "preBalances": [1000000, 0], "postBalances": [895000, 100000]});
                (json!([{"pubkey": o}, {"pubkey": p}]), meta)
            } else {
                let row =
                    |owner: &str, amount: &str| json!({"owner": owner, "mint": m, "uiTokenAmount": {"amount": amount}});
                let meta = json!({
                    "err": null, "fee": 5000, "preBalances": [1000000, 0], "postBalances": [995000, 0],
                    "preTokenBalances": [row(&p, "50"), row(&o, "0")],
                    "postTokenBalances": [row(&p, "20"), row(&o, "30")],
                });
                (json!([{"pubkey": p}, {"pubkey": o}]), meta)
            };
            let block_time = if params[0] == a.as_str() {
                1700000002
            } else {
                1700000001
            };
            ok(json!({"blockTime": block_time, "transaction": {"message": {"accountKeys": keys}}, "meta": meta}))
        }
        other => panic!("{other}"),
    })
    .await;
    let config = format!("[networks.solana]\nupstreams = ['{url}']\ntokens = ['{mint}']");
    let gateway = Gateway::start(&config).await;
    let (status, reply) = gateway.post("/v1/solana/history", &json!({"addresses": [owner]})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(
        reply,
        json!({"items": [
            {"hash": sig_a, "time": 1700000002, "from": owner, "to": other, "asset": "native", "amount": "100000", "status": "finalized"},
            {"hash": sig_b, "time": 1700000001, "from": other, "to": owner, "asset": mint, "amount": "30", "status": "finalized"},
        ], "cursor": null,
            "checkpoint": "500",
            "unchanged": false}),
    );
}

// ---- 价格 ----

#[tokio::test]
async fn prices_are_empty_without_an_upstream() {
    let gateway = Gateway::start("").await;
    let (status, reply) = gateway.get_json("/v1/prices").await;
    assert_eq!((status, reply), (StatusCode::OK, json!({"prices": {}})));
}

#[tokio::test]
async fn prices_map_coingecko_ids_to_network_keys_and_are_cached() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream = Router::new().route(
        "/simple/price",
        get(
            move |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| {
                counter.fetch_add(1, Ordering::SeqCst);
                assert_eq!(query["ids"], "ethereum,tether");
                assert_eq!(query["vs_currencies"], "usd");
                assert_eq!(headers["x-cg-demo-api-key"], "k");
                // CoinGecko 对没有 User-Agent 的请求返回 403.
                assert!(headers["user-agent"].to_str().unwrap().starts_with("bitone-gateway/"));
                async { Json(json!({"ethereum": {"usd": 3400.12}, "tether": {"usd": 1}})) }
            },
        ),
    );
    let url = mock(upstream).await;
    let config = format!(
        "[prices]\nurl = '{url}/simple/price'\napi_key_header = 'x-cg-demo-api-key'\napi_key = 'k'\n\
         [prices.ids]\n'ethereum:native' = 'ethereum'\n'ethereum:{USDT}' = 'tether'\n'bsc:{USDT}' = 'tether'\n"
    );
    let gateway = Gateway::start(&config).await;
    let expected = json!({"prices": {
        "ethereum:native": "3400.12",
        (format!("ethereum:{USDT}")): "1",
        (format!("bsc:{USDT}")): "1",
    }});
    for _ in 0..3 {
        let (status, reply) = gateway.get_json("/v1/prices").await;
        assert_eq!((status, reply), (StatusCode::OK, expected.clone()));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "价格要缓存");
}

#[tokio::test]
async fn prices_upstream_failure_is_reported() {
    let url = mock(Router::new().route("/", get(|| async { StatusCode::TOO_MANY_REQUESTS }))).await;
    let config = format!("[prices]\nurl = '{url}/'\n[prices.ids]\n'bitcoin:native' = 'bitcoin'\n");
    let gateway = Gateway::start(&config).await;
    let (status, reply) = gateway.get_json("/v1/prices").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "upstream_unavailable");
}

#[test]
fn testnet_prices_are_refused_at_startup() {
    let config = "[prices.ids]\n'sepolia:native' = 'ethereum'\n";
    assert!(bitone_gateway::config::Config::parse(config).is_err());
}

// ---- 缓存 ----

#[tokio::test]
async fn public_chain_data_is_cached() {
    let (url, calls) = rpc_node(|method, _| match method {
        "eth_feeHistory" => ok(json!({"baseFeePerGas": ["0x1"], "reward": []})),
        "eth_maxPriorityFeePerGas" => ok(json!("0x1")),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    for _ in 0..3 {
        assert_eq!(gateway.get_json("/v1/ethereum/fees").await.0, StatusCode::OK);
    }
    assert_eq!(calls.count("eth_feeHistory"), 1);

    let (url, calls) = rpc_node(|method, _| match method {
        "getLatestBlockhash" => ok(json!({"context": {"slot": 1}, "value": {"blockhash": "Hash"}})),
        other => panic!("{other}"),
    })
    .await;
    let gateway = Gateway::start(&network_config("solana", &url)).await;
    for _ in 0..3 {
        assert_eq!(gateway.get_json("/v1/solana/blockhash").await.0, StatusCode::OK);
    }
    assert_eq!(calls.count("getLatestBlockhash"), 1);
}

#[tokio::test]
async fn tron_ref_block_is_cached() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let tron = Router::new().route(
        "/walletsolidity/getnowblock",
        get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Json(tron_block(7)) }
        }),
    );
    let url = mock(tron).await;
    let gateway = Gateway::start(&network_config("tron", &url)).await;
    for _ in 0..3 {
        assert_eq!(gateway.get_json("/v1/tron/ref-block").await.0, StatusCode::OK);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn address_and_transaction_queries_are_never_cached() {
    let (url, calls) = rpc_node(|method, params| {
        tags(method, params).unwrap_or_else(|| match method {
            "eth_getTransactionCount" => ok(json!("0x1")),
            _ => ok(Value::Null),
        })
    })
    .await;
    let gateway = Gateway::start(&network_config("ethereum", &url)).await;
    for _ in 0..3 {
        gateway.post("/v1/ethereum/nonce", &json!({"addresses": [EVM]})).await;
        gateway.post("/v1/ethereum/tx", &json!({"hashes": [EVM_HASH]})).await;
    }
    assert_eq!(calls.count("eth_getTransactionCount"), 6);
    assert_eq!(calls.count("eth_getTransactionReceipt"), 3);
}

// ---- 响应里不泄露内部信息 ----

#[tokio::test]
async fn failures_do_not_echo_the_query_or_the_upstream() {
    let dead = dead_url().await;
    let gateway = Gateway::start(&network_config("ethereum", &dead)).await;
    for (path, body) in [
        ("/v1/ethereum/tx", json!({"hashes": [EVM_HASH]})),
        ("/v1/ethereum/nonce", json!({"addresses": [EVM]})),
        ("/v1/ethereum/broadcast", json!({"raw": EVM_RAW})),
    ] {
        let (_, reply) = gateway.post(path, &body).await;
        let text = reply.to_string();
        assert_eq!(reply["error"]["message"], "上游不可用", "{path}");
        assert!(
            !text.contains(&dead) && !text.contains(EVM) && !text.contains(EVM_HASH),
            "{text}"
        );
    }
}

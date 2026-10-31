//! 集成测试。网关真实监听本地端口，上游全部是本地起的 axum 模拟服务，不访问外网。

mod common;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use common::{Gateway, Ws, call, dead_url, error_code, mock, next_json, send};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
const BTC: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
const BTC_TESTNET: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
const UINT256_MAX: &str = "115792089237316195423570985008687907853269984665640564039457584007913129639935";

const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self'; manifest-src 'self'; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn tron_address(seed: u8) -> String {
    let mut payload = [seed; 21];
    payload[0] = 0x41;
    bitcoin::base58::encode_check(&payload)
}

fn solana_address(seed: u8) -> String {
    bitcoin::base58::encode(&[seed; 32])
}

fn amounts(body: &Value) -> Vec<(String, String)> {
    body["balances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["asset"].as_str().unwrap().to_string(),
                b["amount"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

// ---- 模拟上游 ----

/// EVM 节点。响应故意反序，验证网关按 id 归位。余额可以在测试中途修改。
#[derive(Clone)]
struct EvmNode {
    native: Arc<Mutex<String>>,
    block: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
    balance_calls: Arc<AtomicUsize>,
}

impl EvmNode {
    async fn start(native: &str) -> (Self, String) {
        let node = Self {
            native: Arc::new(Mutex::new(native.into())),
            block: Arc::new(AtomicUsize::new(1)),
            hits: Default::default(),
            balance_calls: Default::default(),
        };
        let router = Router::new().route("/", post(evm_rpc)).with_state(node.clone());
        (node, mock(router).await)
    }

    /// 余额只会在新块里变化。
    fn set_native(&self, native: &str) {
        *self.native.lock().unwrap() = native.into();
        self.next_block();
    }

    fn next_block(&self) {
        self.block.fetch_add(1, Ordering::SeqCst);
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn balance_calls(&self) -> usize {
        self.balance_calls.load(Ordering::SeqCst)
    }

    /// 等到网关至少又轮询了一次。
    async fn wait_for_poll(&self) {
        let start = self.hits();
        timeout(Duration::from_secs(5), async {
            while self.hits() == start {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("网关没有轮询上游");
    }
}

async fn evm_rpc(State(node): State<EvmNode>, Json(body): Json<Value>) -> Json<Value> {
    node.hits.fetch_add(1, Ordering::SeqCst);
    let native = node.native.lock().unwrap().clone();
    let block = format!("{:#x}", node.block.load(Ordering::SeqCst));
    let calls = body.as_array().cloned().unwrap_or_else(|| vec![body.clone()]);
    let replies: Vec<Value> = calls
        .iter()
        .rev()
        .map(|call| {
            let result = match call["method"].as_str().unwrap() {
                "eth_getBalance" => {
                    node.balance_calls.fetch_add(1, Ordering::SeqCst);
                    native.clone()
                }
                "eth_blockNumber" => block.clone(),
                "eth_getBlockByNumber" => {
                    let block = json!({"number": block, "timestamp": "0x1", "transactions": []});
                    return json!({"jsonrpc": "2.0", "id": call["id"], "result": block});
                }
                "eth_getLogs" => return json!({"jsonrpc": "2.0", "id": call["id"], "result": []}),
                _ => {
                    assert_eq!(call["params"][0]["to"], USDT);
                    let data = call["params"][0]["data"].as_str().unwrap();
                    assert!(data.starts_with("0x70a08231000000000000000000000000"), "{data}");
                    assert_eq!(data.len(), 74);
                    format!("0x{}", "f".repeat(64))
                }
            };
            json!({"jsonrpc": "2.0", "id": call["id"], "result": result})
        })
        .collect();
    Json(if body.is_array() {
        Value::Array(replies)
    } else {
        replies[0].clone()
    })
}

fn evm_config(urls: &[&str], extra: &str) -> String {
    let urls = urls.iter().map(|u| format!("'{u}'")).collect::<Vec<_>>().join(",");
    format!("{extra}\n[networks.ethereum]\nupstreams = [{urls}]\ntokens = ['{USDT}']\n")
}

// ---- 版本 ----

#[tokio::test]
async fn version_lists_only_configured_networks() {
    let config = "[networks.solana]\nupstreams = ['http://a']\n[networks.bitcoin]\nupstreams = ['http://b']\n[networks.sepolia]\nupstreams = []";
    let gateway = Gateway::start(config).await;
    let body: Value = gateway.get("/v1/version").await.json().await.unwrap();
    assert_eq!(
        body,
        json!({"version": env!("CARGO_PKG_VERSION"), "networks": ["bitcoin", "solana"], "noHistory": []})
    );
}

#[tokio::test]
async fn version_lists_networks_without_history() {
    let config = "[networks.ethereum]\nupstreams = ['http://a']\nindexer = 'http://b'\n\
                  [networks.bsc]\nupstreams = ['http://a']\n[networks.solana]\nupstreams = ['http://a']";
    let gateway = Gateway::start(config).await;
    let body: Value = gateway.get("/v1/version").await.json().await.unwrap();
    assert_eq!(body["noHistory"], json!(["bsc"]));
}

// ---- 四条链的余额解析 ----

#[tokio::test]
async fn bitcoin_is_funded_minus_spent_on_chain() {
    let esplora = Router::new().route(
        "/api/address/{address}",
        get(|Path(address): Path<String>| async move {
            Json(json!({
                "address": address,
                "chain_stats": {"funded_txo_sum": 150_000, "spent_txo_sum": 50_000},
                "mempool_stats": {"funded_txo_sum": 7, "spent_txo_sum": 0},
            }))
        }),
    );
    let url = mock(esplora).await;
    // 上游地址带路径和结尾斜杠。
    let gateway = Gateway::start(&format!("[networks.bitcoin]\nupstreams = ['{url}/api/']")).await;
    let (status, body) = gateway.balances("bitcoin", &[BTC]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"balances": [{"address": BTC, "asset": "native", "amount": "100000"}]})
    );
}

#[tokio::test]
async fn bitcoin_spent_above_funded_is_an_upstream_failure() {
    let esplora = Router::new().route(
        "/address/{address}",
        get(|| async { Json(json!({"chain_stats": {"funded_txo_sum": 1, "spent_txo_sum": 2}})) }),
    );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&format!("[networks.bitcoin]\nupstreams = ['{url}']")).await;
    let (status, body) = gateway.balances("bitcoin", &[BTC]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "upstream_unavailable");
}

#[tokio::test]
async fn evm_reads_native_and_uint256_token() {
    let (node, url) = EvmNode::start("0xde0b6b3a7640000").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let (status, body) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"balances": [
            {"address": EVM, "asset": "native", "amount": "1000000000000000000"},
            {"address": EVM, "asset": USDT, "amount": UINT256_MAX},
        ]}),
    );
    assert_eq!(node.hits(), 1, "一次查询应该合并成一个批量请求");
}

#[tokio::test]
async fn evm_splits_large_queries_into_batches() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let addresses: Vec<String> = (0..100).map(|i| format!("0x{i:040x}")).collect();
    let refs: Vec<&str> = addresses.iter().map(String::as_str).collect();
    let (status, body) = gateway.balances("ethereum", &refs).await;
    assert_eq!(status, StatusCode::OK);
    let balances = body["balances"].as_array().unwrap();
    assert_eq!(balances.len(), 200);
    // 分批之后地址和资产的对应关系不能乱。
    for (i, pair) in balances.chunks(2).enumerate() {
        assert_eq!(pair[0]["address"], addresses[i].as_str());
        assert_eq!(pair[0]["amount"], "1");
        assert_eq!(pair[1]["address"], addresses[i].as_str());
        assert_eq!(pair[1]["amount"], UINT256_MAX);
    }
    assert_eq!(node.hits(), 2);
}

#[tokio::test]
async fn tron_handles_activated_and_unactivated_accounts() {
    let token = tron_address(9);
    let other_token = tron_address(8);
    let (active, empty) = (tron_address(1), tron_address(2));
    let (a, t, o) = (active.clone(), token.clone(), other_token.clone());
    let tron = Router::new().route(
        "/v1/accounts/{address}",
        get(move |Path(address): Path<String>| {
            let data = if address == a {
                json!([{"balance": 7_000_000, "trc20": [{o.clone(): "1"}, {t.clone(): "2500000"}]}])
            } else {
                json!([])
            };
            async move { Json(json!({"data": data, "success": true, "meta": {}})) }
        }),
    );
    let url = mock(tron).await;
    let config = format!("[networks.tron]\nupstreams = ['{url}']\ntokens = ['{token}']");
    let gateway = Gateway::start(&config).await;
    let (status, body) = gateway.balances("tron", &[&active, &empty]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"balances": [
            {"address": active, "asset": "native", "amount": "7000000"},
            {"address": active, "asset": token, "amount": "2500000"},
            {"address": empty, "asset": "native", "amount": "0"},
            {"address": empty, "asset": token, "amount": "0"},
        ]}),
    );
}

#[tokio::test]
async fn tron_without_data_is_an_upstream_failure() {
    let tron = Router::new().route(
        "/v1/accounts/{address}",
        get(|| async { Json(json!({"success": false})) }),
    );
    let url = mock(tron).await;
    let gateway = Gateway::start(&format!("[networks.tron]\nupstreams = ['{url}']")).await;
    let (status, _) = gateway.balances("tron", &[&tron_address(1)]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn solana_sums_token_accounts() {
    let (mint, other_mint) = (solana_address(9), solana_address(8));
    let owner = solana_address(1);
    let (m, o) = (mint.clone(), other_mint.clone());
    let solana = Router::new().route(
        "/",
        post(move |Json(call): Json<Value>| {
            let account = |mint: &str, amount: &str| {
                json!({"pubkey": "x", "account": {"data": {"parsed": {"info": {
                    "mint": mint,
                    "tokenAmount": {"amount": amount, "decimals": 6},
                }}}}})
            };
            let value = match call["method"].as_str().unwrap() {
                "getBalance" => json!(5_000_000_000u64),
                "getTokenAccountsByOwner" => {
                    assert_eq!(call["params"][1]["mint"], m);
                    assert_eq!(call["params"][2]["encoding"], "jsonParsed");
                    json!([account(&m, "1500000"), account(&m, "2500000"), account(&o, "999")])
                }
                method => panic!("{method}"),
            };
            let reply = json!({"jsonrpc": "2.0", "id": call["id"], "result": {"context": {"slot": 1}, "value": value}});
            async move { Json(reply) }
        }),
    );
    let url = mock(solana).await;
    let config = format!("[networks.solana-devnet]\nupstreams = ['{url}']\ntokens = ['{mint}']");
    let gateway = Gateway::start(&config).await;
    let (status, body) = gateway.balances("solana-devnet", &[&owner]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(amounts(&body), pairs(&[("native", "5000000000"), (&mint, "4000000")]));
}

#[tokio::test]
async fn solana_rpc_error_is_an_upstream_failure() {
    let solana = Router::new().route(
        "/",
        post(|| async { Json(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32005, "message": "限流"}})) }),
    );
    let url = mock(solana).await;
    let gateway = Gateway::start(&format!("[networks.solana]\nupstreams = ['{url}']")).await;
    let (status, _) = gateway.balances("solana", &[&solana_address(1)]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ---- 上游切换 ----

#[tokio::test]
async fn fails_over_past_every_kind_of_bad_upstream() {
    let (good_node, good) = EvmNode::start("0x5").await;
    let call = |result: &str| json!({"jsonrpc": "2.0", "id": 0, "result": result});
    let bad_replies: Vec<Router> = vec![
        Router::new().route("/", post(|| async { StatusCode::INTERNAL_SERVER_ERROR })),
        Router::new().route("/", post(|| async { StatusCode::TOO_MANY_REQUESTS })),
        Router::new().route("/", post(|| async { "不是 JSON" })),
        // 被限流时回一个错误对象，不是数组。
        Router::new().route("/", post(|| async { Json(json!({"error": {"code": -32005}})) })),
        // 缺一项。
        Router::new().route(
            "/",
            post({
                let r = json!([call("0x1")]);
                move || async move { Json(r) }
            }),
        ),
        // 单项出错。
        Router::new().route(
            "/",
            post(|| async { Json(json!([{"id": 0, "error": {"code": -32000}}, {"id": 1, "result": "0x1"}])) }),
        ),
        // 合约不存在，不能当成 0。
        Router::new().route(
            "/",
            post(|| async { Json(json!([{"id": 0, "result": "0x1"}, {"id": 1, "result": "0x"}])) }),
        ),
        // id 越界。
        Router::new().route(
            "/",
            post(|| async { Json(json!([{"id": 0, "result": "0x1"}, {"id": 9, "result": "0x1"}])) }),
        ),
    ];
    let count = bad_replies.len();
    for (i, router) in bad_replies.into_iter().enumerate() {
        let bad = mock(router).await;
        let gateway = Gateway::start(&evm_config(&[&bad, &good], "")).await;
        let (status, body) = gateway.balances("ethereum", &[EVM]).await;
        assert_eq!(status, StatusCode::OK, "第 {i} 种坏上游");
        assert_eq!(amounts(&body)[0].1, "5");
    }
    assert_eq!(good_node.hits(), count);
}

#[tokio::test]
async fn redirects_are_not_followed() {
    // 跟随重定向会把查询的地址带到配置之外的服务器。
    let (trap, trap_url) = EvmNode::start("0x9").await;
    let (_, good) = EvmNode::start("0x5").await;
    let redirect = Router::new().route(
        "/",
        post(move || {
            let location = trap_url.clone();
            async move { (StatusCode::TEMPORARY_REDIRECT, [("location", location)]) }
        }),
    );
    let bad = mock(redirect).await;
    let gateway = Gateway::start(&evm_config(&[&bad, &good], "")).await;
    let (status, body) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(amounts(&body)[0].1, "5");
    assert_eq!(trap.hits(), 0);
}

#[tokio::test]
async fn fails_over_from_connection_refused_and_timeout() {
    let (_, good) = EvmNode::start("0x5").await;
    let dead = dead_url().await;
    let hanging = mock(Router::new().route(
        "/",
        post(|| async {
            sleep(Duration::from_secs(60)).await;
            StatusCode::OK
        }),
    ))
    .await;
    let gateway = Gateway::start(&evm_config(&[&dead, &hanging, &good], "upstream_timeout_ms = 100")).await;
    let (status, body) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(amounts(&body)[0].1, "5");
}

#[tokio::test]
async fn stays_on_the_working_upstream_after_failover() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let bad = mock(Router::new().route(
        "/",
        post(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::BAD_GATEWAY }
        }),
    ))
    .await;
    let (_, good) = EvmNode::start("0x5").await;
    let gateway = Gateway::start(&evm_config(&[&bad, &good], "")).await;
    for _ in 0..3 {
        assert_eq!(gateway.balances("ethereum", &[EVM]).await.0, StatusCode::OK);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn all_upstreams_down_is_503() {
    let (a, b) = (dead_url().await, dead_url().await);
    let gateway = Gateway::start(&evm_config(&[&a, &b], "")).await;
    let (status, body) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "upstream_unavailable");
    // 错误信息不能带上游地址。
    assert_eq!(body["error"]["message"], "上游不可用");
}

// ---- 请求校验 ----

#[tokio::test]
async fn unknown_and_unconfigured_networks() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    for network in ["dogecoin", "sepolia", "version"] {
        let (status, body) = gateway.balances(network, &[EVM]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error_code(&body), "unknown_network");
    }
}

#[tokio::test]
async fn invalid_addresses_are_rejected_before_any_upstream_call() {
    let (node, url) = EvmNode::start("0x1").await;
    let config = format!("{}\n[networks.bitcoin]\nupstreams = ['{url}']", evm_config(&[&url], ""));
    let gateway = Gateway::start(&config).await;
    for (network, address) in [
        ("ethereum", "0x1234"),
        ("ethereum", BTC),
        ("ethereum", "../../admin"),
        ("bitcoin", BTC_TESTNET),
        ("bitcoin", EVM),
        ("bitcoin", "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyv"),
    ] {
        let (status, body) = gateway.balances(network, &[address]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{address}");
        assert_eq!(
            body,
            json!({"error": {"code": "invalid_address", "message": "地址格式错误"}})
        );
    }
    // 一批里有一个错的，整批拒绝。
    assert_eq!(
        gateway.balances("ethereum", &[EVM, "0x1234"]).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(node.hits(), 0);
}

#[tokio::test]
async fn malformed_bodies_use_the_error_format() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let endpoint = format!("{}/v1/ethereum/balances", gateway.base);
    for body in ["", "{", "{}", "[]", r#"{"addresses": "x"}"#, r#"{"addresses": [1]}"#] {
        let response = reqwest::Client::new().post(&endpoint).body(body).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(error_code(&body), "invalid_request");
    }
}

#[tokio::test]
async fn enforces_the_address_limit_and_body_size() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let addresses: Vec<String> = (0..101).map(|i| format!("0x{i:040x}")).collect();
    let mut refs: Vec<&str> = addresses.iter().map(String::as_str).collect();
    let (status, body) = gateway.balances("ethereum", &refs).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "too_many_addresses");
    assert_eq!(node.hits(), 0);

    refs.pop();
    assert_eq!(gateway.balances("ethereum", &refs).await.0, StatusCode::OK);

    let huge = json!({"addresses": ["0".repeat(100_000)]}).to_string();
    let endpoint = format!("{}/v1/ethereum/balances", gateway.base);
    let response = reqwest::Client::new().post(endpoint).body(huge).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = response.json().await.unwrap();
    assert_eq!(error_code(&body), "invalid_request");
}

// ---- 限流 ----

#[tokio::test]
async fn rate_limit_returns_the_error_format() {
    let (_, url) = EvmNode::start("0x1").await;
    let config = evm_config(&[&url], "rate_limit_burst = 3\nrate_limit_per_second = 0.001");
    let gateway = Gateway::start(&config).await;
    for _ in 0..3 {
        assert_eq!(gateway.get("/v1/version").await.status(), StatusCode::OK);
    }
    let (status, body) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_code(&body), "rate_limited");
    // 配额用完后握手也被拒绝。
    assert!(connect_async(gateway.ws.as_str()).await.is_err());
}

// ---- WebSocket ----

fn subscribe_message(id: u64, addresses: &[String]) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "subscribe",
           "params": {"channel": "addresses", "network": "ethereum", "addresses": addresses}})
}

async fn subscribe(ws: &mut Ws, addresses: &[&str]) -> String {
    let addresses: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
    let reply = call(ws, subscribe_message(1, &addresses)).await;
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], 1);
    reply["result"].as_str().expect("订阅 ID").to_string()
}

const FAST_POLL: &str = "ws_poll_interval_ms = 10";

#[tokio::test]
async fn ws_pushes_only_when_balance_changes() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let id = subscribe(&mut ws, &[EVM]).await;

    // 余额没变时多轮询几次，不应该有推送。
    for _ in 0..3 {
        node.wait_for_poll().await;
    }
    assert!(timeout(Duration::from_millis(50), next_json(&mut ws)).await.is_err());

    node.set_native("0x2");
    let update = next_json(&mut ws).await;
    let (_, http) = gateway.balances("ethereum", &[EVM]).await;
    assert_eq!(update["jsonrpc"], "2.0");
    assert_eq!(update["method"], "update");
    assert!(update.get("id").is_none(), "推送是通知，不能带 id");
    assert_eq!(update["params"]["subscription"], id.as_str());
    assert_eq!(update["params"]["height"], 2);
    assert_eq!(update["params"]["txs"], json!([]));
    assert_eq!(update["params"]["balances"], http["balances"], "格式要与 HTTP 接口相同");

    // 同一个变化只推一次，再变再推。
    assert!(timeout(Duration::from_millis(50), next_json(&mut ws)).await.is_err());
    node.set_native("0x3");
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["balances"][0]["amount"], "3");
    assert_eq!(
        (&update["params"]["previous"], &update["params"]["height"]),
        (&json!(2), &json!(3))
    );
}

/// 没有变化的新块也要通知高度，客户端靠它推进游标，靠 `previous` 发现漏了推送。
#[tokio::test]
async fn ws_reports_every_new_height() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;

    node.next_block();
    let first = next_json(&mut ws).await;
    assert_eq!(first["params"]["balances"], json!([]));
    node.next_block();
    let second = next_json(&mut ws).await;
    assert_eq!(second["params"]["previous"], first["params"]["height"]);
    assert_eq!(second["params"]["height"], 3);
}

/// 同一个地址有多少订阅，一轮都只查一次。
#[tokio::test]
async fn ws_subscribers_share_one_round() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let (mut first, mut second) = (gateway.ws().await, gateway.ws().await);
    subscribe(&mut first, &[EVM]).await;
    subscribe(&mut second, &[EVM]).await;
    node.wait_for_poll().await;

    let before = node.balance_calls();
    node.set_native("0x2");
    next_json(&mut first).await;
    next_json(&mut second).await;
    assert_eq!(node.balance_calls() - before, 1);
}

/// 有订阅才跟块，闲时不访问上游。
#[tokio::test]
async fn ws_follows_only_while_someone_is_subscribed() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    sleep(Duration::from_millis(50)).await;
    assert_eq!(node.hits(), 0, "没有订阅时不访问上游");

    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;
    ws.close(None).await.unwrap();
    sleep(Duration::from_millis(100)).await;
    let after = node.hits();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(node.hits(), after, "最后一个订阅离开后跟块停止");
}

/// 公共 Esplora 按 IP 限流，80 个地址每轮都查撑不住。余额只算已确认，所以高度没变就不查地址。
#[tokio::test]
async fn ws_bitcoin_requeries_addresses_only_when_the_tip_moves() {
    let (hits, height, funded) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(100)),
        Arc::new(AtomicUsize::new(1)),
    );
    let esplora = Router::new()
        .route(
            "/address/{address}",
            get({
                let (hits, funded) = (hits.clone(), funded.clone());
                move |Path(address): Path<String>| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let funded = funded.load(Ordering::SeqCst);
                    Json(json!({
                        "address": address,
                        "chain_stats": {"funded_txo_sum": funded, "spent_txo_sum": 0},
                    }))
                }
            }),
        )
        .route(
            "/blocks/tip/height",
            get({
                let height = height.clone();
                move || async move { height.load(Ordering::SeqCst).to_string() }
            }),
        );
    let url = mock(esplora).await;
    let gateway = Gateway::start(&format!("{FAST_POLL}\n[networks.bitcoin]\nupstreams = ['{url}']")).await;
    let mut ws = gateway.ws().await;
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe",
                         "params": {"channel": "addresses", "network": "bitcoin", "addresses": [BTC]}});
    assert!(call(&mut ws, message).await["result"].is_string());

    let baseline = hits.load(Ordering::SeqCst);
    sleep(Duration::from_millis(100)).await;
    assert_eq!(hits.load(Ordering::SeqCst), baseline, "高度没变不查地址");

    funded.store(2, Ordering::SeqCst);
    height.store(101, Ordering::SeqCst);
    let update = timeout(Duration::from_secs(5), next_json(&mut ws))
        .await
        .expect("新块之后要推送");
    assert_eq!(update["params"]["balances"][0]["amount"], "2");
}

#[tokio::test]
async fn ws_ping_answers_on_the_application_layer() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let reply = call(&mut ws, json!({"jsonrpc": "2.0", "id": 9, "method": "ping"})).await;
    assert_eq!(reply["result"], "pong");
}

#[tokio::test]
async fn ws_baseline_is_taken_before_the_subscribe_reply() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    // 收到回复后马上发生的变化一定要推送。
    node.set_native("0x2");
    assert_eq!(next_json(&mut ws).await["params"]["balances"][0]["amount"], "2");
}

#[tokio::test]
async fn ws_pushes_after_upstream_recovers_from_failed_baseline() {
    let dead = dead_url().await;
    let gateway = Gateway::start(&evm_config(&[&dead], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let id = subscribe(&mut ws, &[EVM]).await;

    // 上游恢复：同一个端口开始监听。
    let (node, _) = EvmNode::start("0x7").await;
    node.next_block();
    let router = Router::new().route("/", post(evm_rpc)).with_state(node);
    let listener = TcpListener::bind(dead.trim_start_matches("http://")).await.unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });

    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["subscription"], id.as_str());
    assert_eq!(update["params"]["balances"][0]["amount"], "7");
}

#[tokio::test]
async fn ws_reports_errors() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let mut ws = gateway.ws().await;
    let subscribe = |network: &str, addresses: Value| {
        json!({"jsonrpc": "2.0", "id": 7, "method": "subscribe",
               "params": {"channel": "addresses", "network": network, "addresses": addresses}})
    };

    let reply = call(&mut ws, subscribe("dogecoin", json!([EVM]))).await;
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["error"]["code"], -32001);
    assert_eq!(reply["error"]["data"]["code"], "unknown_network");
    let reply = call(&mut ws, subscribe("ethereum", json!(["0x12"]))).await;
    assert_eq!(reply["error"]["data"]["code"], "invalid_address");
    let reply = call(&mut ws, subscribe("ethereum", json!([]))).await;
    assert_eq!(reply["error"]["code"], -32602);

    let channel =
        json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe", "params": {"channel": "nope", "network": "ethereum"}});
    assert_eq!(call(&mut ws, channel).await["error"]["code"], -32602);
    let method = json!({"jsonrpc": "2.0", "id": 1, "method": "nope"});
    assert_eq!(call(&mut ws, method).await["error"]["code"], -32601);
    let version = json!({"jsonrpc": "1.0", "id": 1, "method": "subscribe"});
    assert_eq!(call(&mut ws, version).await["error"]["code"], -32600);

    ws.send(Message::text("不是 JSON")).await.unwrap();
    let reply = next_json(&mut ws).await;
    assert_eq!(reply["error"]["code"], -32700);
    assert_eq!(reply["id"], Value::Null);

    // 通知不回复：下一条收到的是后面这个请求的回复。
    send(&mut ws, json!({"jsonrpc": "2.0", "method": "nope"})).await;
    assert_eq!(
        call(&mut ws, json!({"jsonrpc": "2.0", "id": 2, "method": "nope"})).await["id"],
        2
    );
}

#[tokio::test]
async fn ws_rate_limit_applies_per_message() {
    let (_, url) = EvmNode::start("0x1").await;
    let config = evm_config(&[&url], "rate_limit_burst = 3\nrate_limit_per_second = 0.001");
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await; // 第 1 个令牌
    let method = json!({"jsonrpc": "2.0", "id": 1, "method": "nope"});
    assert_eq!(call(&mut ws, method.clone()).await["error"]["code"], -32601);
    assert_eq!(call(&mut ws, method.clone()).await["error"]["code"], -32601);
    let reply = call(&mut ws, method).await;
    assert_eq!(reply["error"]["data"]["code"], "rate_limited");
}

#[tokio::test]
async fn ws_address_cap_is_per_connection() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let addresses: Vec<String> = (0..1001).map(|i| format!("0x{i:040x}")).collect();
    let mut ws = gateway.ws().await;

    let reply = call(&mut ws, subscribe_message(1, &addresses)).await;
    assert_eq!(reply["error"]["data"]["code"], "too_many_addresses");

    // 上限按连接累计，不是按单次订阅。
    let first = call(&mut ws, subscribe_message(2, &addresses[..600])).await["result"].clone();
    let reply = call(&mut ws, subscribe_message(3, &addresses[600..1001])).await;
    assert_eq!(reply["error"]["data"]["code"], "too_many_addresses");
    let second = call(&mut ws, subscribe_message(4, &addresses[600..1000])).await;
    assert!(second["result"].is_string());

    // 退订后释放额度。
    let unsubscribe = json!({"jsonrpc": "2.0", "id": 5, "method": "unsubscribe", "params": [first]});
    assert_eq!(call(&mut ws, unsubscribe).await["result"], true);
    assert!(call(&mut ws, subscribe_message(6, &addresses[..600])).await["result"].is_string());

    // 另一个连接的额度是独立的。
    let mut other = gateway.ws().await;
    assert!(call(&mut other, subscribe_message(1, &addresses[..1000])).await["result"].is_string());
}

#[tokio::test]
async fn ws_oversized_message_closes_the_connection() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "")).await;
    let mut ws = gateway.ws().await;
    ws.send(Message::text("x".repeat(200_000))).await.unwrap();
    let closed = timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        }
    });
    closed.await.expect("超大消息没有断开连接");
}

#[tokio::test]
async fn ws_unsubscribe_stops_polling() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let id = subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;

    for (id_value, params, result) in [
        (1, json!({"subscription": "不存在"}), false),
        (2, json!(id), true),
        (3, json!([id]), false),
    ] {
        let message = json!({"jsonrpc": "2.0", "id": id_value, "method": "unsubscribe", "params": params});
        assert_eq!(call(&mut ws, message).await["result"], result);
    }

    assert_polling_stops(&node).await;
    node.set_native("0x2");
    assert!(timeout(Duration::from_millis(50), next_json(&mut ws)).await.is_err());
}

async fn assert_polling_stops(node: &EvmNode) {
    // 留出在途请求结束的时间。
    sleep(Duration::from_millis(100)).await;
    let hits = node.hits();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(node.hits(), hits, "订阅还在轮询上游");
}

#[tokio::test]
async fn ws_disconnect_clears_subscriptions() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;
    drop(ws);
    assert_polling_stops(&node).await;
}

#[tokio::test]
async fn ws_close_frame_clears_subscriptions() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;
    ws.close(None).await.unwrap();
    assert_polling_stops(&node).await;
}

#[tokio::test]
async fn ws_silent_client_is_dropped_by_heartbeat() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "ws_poll_interval_ms = 10\nws_heartbeat_ms = 20")).await;
    // 连接保持着，但不再读取，也就不会回 pong。
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    node.wait_for_poll().await;
    assert_polling_stops(&node).await;
    drop(ws);
}

#[tokio::test]
async fn ws_sends_heartbeats() {
    let (_, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], "ws_heartbeat_ms = 20")).await;
    let mut ws = gateway.ws().await;
    for _ in 0..3 {
        let frame = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(frame, Message::Ping(_)), "{frame:?}");
    }
}

// ---- 优雅退出 ----

#[tokio::test]
async fn shutdown_closes_websockets_and_returns() {
    let (node, url) = EvmNode::start("0x1").await;
    let gateway = Gateway::start(&evm_config(&[&url], FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    subscribe(&mut ws, &[EVM]).await;
    assert_eq!(gateway.balances("ethereum", &[EVM]).await.0, StatusCode::OK);

    gateway.stop.send(()).unwrap();
    let frame = timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Close(frame) = ws.next().await.unwrap().unwrap() {
                return frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(frame.unwrap().code, CloseCode::Away);
    timeout(Duration::from_secs(5), gateway.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_polling_stops(&node).await;
}

// ---- 静态托管和安全头 ----

fn web_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gateway-web")
        .join(name);
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("assets")).unwrap();
    std::fs::write(site.join("index.html"), "<!doctype html>BitOne").unwrap();
    std::fs::write(site.join("assets/app-abc.js"), "console.log(1)").unwrap();
    std::fs::write(site.join("assets/core.wasm"), [0, 97, 115, 109]).unwrap();
    std::fs::write(dir.join("secret.txt"), "不该被读到").unwrap();
    site
}

async fn hosted(name: &str, extra: &str) -> Gateway {
    let dir = web_dir(name);
    Gateway::start(&format!("web_dir = '{}'\n{extra}", dir.display())).await
}

fn assert_security_headers(response: &reqwest::Response) {
    let headers = response.headers();
    assert_eq!(headers["content-security-policy"], CSP);
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["permissions-policy"], "camera=(self)");
}

#[tokio::test]
async fn every_response_has_security_headers() {
    let gateway = hosted("headers", "").await;
    for path in [
        "/",
        "/assets/app-abc.js",
        "/wallet",
        "/v1/version",
        "/assets/none.js",
        "/v1/none",
    ] {
        assert_security_headers(&gateway.get(path).await);
    }
    // 错误响应和方法不允许也要带。
    let post = reqwest::Client::new()
        .post(format!("{}/v1/version", gateway.base))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_security_headers(&post);
    let post = reqwest::Client::new()
        .post(format!("{}/v1/nope/balances", gateway.base))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&post);
}

#[tokio::test]
async fn rate_limited_responses_have_security_headers_and_static_files_stay_available() {
    let gateway = hosted("limit", "rate_limit_burst = 1\nrate_limit_per_second = 0.001").await;
    assert_eq!(gateway.get("/v1/version").await.status(), StatusCode::OK);
    let limited = gateway.get("/v1/version").await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_security_headers(&limited);
    assert_eq!(gateway.get("/wallet").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn websocket_handshake_has_security_headers() {
    let gateway = hosted("ws-headers", "").await;
    let (_, response) = connect_async(gateway.ws.as_str()).await.unwrap();
    assert_eq!(response.headers()["content-security-policy"], CSP);
}

#[tokio::test]
async fn spa_routes_fall_back_to_index() {
    let gateway = hosted("spa", "").await;
    for path in ["/", "/index.html", "/wallet", "/wallet/settings/", "/tx/123"] {
        let response = gateway.get(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let content_type = response.headers()["content-type"].to_str().unwrap().to_string();
        assert!(content_type.starts_with("text/html"), "{path} {content_type}");
        assert_eq!(response.text().await.unwrap(), "<!doctype html>BitOne", "{path}");
    }
}

#[tokio::test]
async fn static_files_are_served_with_their_types() {
    let gateway = hosted("static", "").await;
    let js = gateway.get("/assets/app-abc.js").await;
    assert_eq!(js.headers()["content-type"], "text/javascript");
    assert_eq!(js.text().await.unwrap(), "console.log(1)");
    let wasm = gateway.get("/assets/core.wasm").await;
    assert_eq!(wasm.headers()["content-type"], "application/wasm");
}

#[tokio::test]
async fn missing_assets_and_api_paths_do_not_fall_back_to_index() {
    let gateway = hosted("no-fallback", "").await;
    for path in [
        "/assets/missing.js",
        "/favicon.ico",
        "/v1",
        "/v1/",
        "/v1/nope",
        "/v1/ethereum/nope",
        "/..%2fsecret.txt",
    ] {
        let response = gateway.get(path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!response.text().await.unwrap().contains("BitOne"), "{path}");
    }
    assert_eq!(gateway.get("/v1/version").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn without_web_dir_only_the_api_exists() {
    let gateway = Gateway::start("").await;
    let response = gateway.get("/").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&response);
    assert_eq!(gateway.get("/v1/version").await.status(), StatusCode::OK);
}

// ---- 响应体里不回显输入 ----

#[tokio::test]
async fn error_bodies_do_not_echo_the_address() {
    let gateway = Gateway::start("[networks.ethereum]\nupstreams = ['http://127.0.0.1:1']").await;
    let (_, body) = gateway.balances("ethereum", &["0xdeadbeef"]).await;
    assert!(!body.to_string().contains("deadbeef"), "{body}");
}

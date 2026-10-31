//! 跟块和快照的集成测试，上游都是本地模拟服务。
//! 重点是没有索引服务时也能解析交易、一部分失败不影响其余，以及水位分组。

mod common;

use axum::Router;
use axum::extract::Path;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use common::{Calls, Gateway, call, dead_url, error_code, mock, next_json, ok, rpc_failure, rpc_node};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};

const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
const OTHER: &str = "0x3535353535353535353535353535353535353535";
const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const BTC: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

fn padded(address: &str) -> String {
    format!("0x{:0>64}", address[2..].to_lowercase())
}

/// 一条模拟的 EVM 链。
#[derive(Default)]
struct Scene {
    block: u64,
    native: String,
    token: u64,
    txs: HashMap<u64, Vec<Value>>,
    logs: Vec<Value>,
    broken_logs: bool,
}

type Chain = Arc<Mutex<Scene>>;

fn reply(chain: &Chain, method: &str, params: &Value) -> Value {
    let scene = chain.lock().unwrap();
    let hex = |n: u64| format!("{n:#x}");
    match method {
        "eth_blockNumber" => ok(json!(hex(scene.block))),
        "eth_getBalance" => ok(json!(scene.native)),
        "eth_call" => ok(json!(format!("{:#066x}", scene.token))),
        "eth_getLogs" if scene.broken_logs => rpc_failure(-32000, "query returned more than 10000 results"),
        "eth_getLogs" => {
            let wanted = |log: &Value| {
                let topic = |index: usize| &params[0]["topics"][index];
                (1..=2).any(|index| {
                    topic(index)
                        .as_array()
                        .is_some_and(|list| list.contains(&log["topics"][index]))
                })
            };
            ok(json!(scene.logs.iter().filter(|log| wanted(log)).collect::<Vec<_>>()))
        }
        "eth_getTransactionReceipt" => ok(json!({"status": "0x1"})),
        "eth_getBlockByNumber" => {
            let number = match params[0].as_str().unwrap() {
                "safe" | "finalized" => 5,
                text => u64::from_str_radix(text.trim_start_matches("0x"), 16).unwrap(),
            };
            let txs = scene.txs.get(&number).cloned().unwrap_or_default();
            ok(json!({"number": hex(number), "timestamp": hex(1000 + number), "transactions": txs}))
        }
        _ => panic!("没有模拟 {method}"),
    }
}

async fn evm_chain() -> (Chain, String) {
    let chain: Chain = Arc::new(Mutex::new(Scene {
        block: 10,
        native: "0x1".to_string(),
        token: 7,
        ..Default::default()
    }));
    let node = chain.clone();
    let (url, _) = rpc_node(move |method, params| reply(&node, method, params)).await;
    (chain, url)
}

fn evm_config(url: &str) -> String {
    format!("ws_poll_interval_ms = 10\n[networks.ethereum]\nupstreams = ['{url}']\ntokens = ['{USDT}']\n")
}

/// 快照一行一个 JSON。
async fn snapshot(gateway: &Gateway, body: &Value) -> Vec<Value> {
    let url = format!("{}/v1/snapshot", gateway.base);
    let response = reqwest::Client::new().post(url).json(body).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.unwrap();
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn subscribe(addresses: &[&str]) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe",
           "params": {"channel": "addresses", "network": "ethereum", "addresses": addresses}})
}

#[tokio::test]
async fn evm_update_parses_the_block_without_an_indexer() {
    let (chain, url) = evm_chain().await;
    let gateway = Gateway::start(&evm_config(&url)).await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, subscribe(&[EVM])).await["result"].is_string());

    {
        let mut scene = chain.lock().unwrap();
        scene.block = 11;
        scene.native = "0x11".to_string();
        let own = EVM.to_lowercase();
        scene.txs.insert(
            11,
            vec![
                json!({"hash": "0xaa", "from": OTHER, "to": own, "value": "0x10"}),
                json!({"hash": "0xbb", "from": OTHER, "to": OTHER, "value": "0x99"}),
                // 成功的零金额调用不是转账，这和索引服务的规则一致。
                json!({"hash": "0xcc", "from": own, "to": USDT, "value": "0x0"}),
            ],
        );
        scene.logs.push(json!({
            "address": USDT.to_lowercase(),
            "blockNumber": "0xb",
            "transactionHash": "0xcc",
            "topics": [TRANSFER, padded(EVM), padded(OTHER)],
            "data": format!("{:#066x}", 32),
        }));
    }
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["height"], 11);
    assert_eq!(update["params"]["balances"][0]["amount"], "17");
    let txs = &update["params"]["txs"];
    assert_eq!(txs.as_array().unwrap().len(), 2, "{txs}");
    assert_eq!(
        txs[0],
        json!({"hash": "0xaa", "time": 1011, "from": OTHER, "to": EVM.to_lowercase(), "asset": "native", "amount": "16", "status": "included"}),
    );
    assert_eq!(
        txs[1],
        json!({"hash": "0xcc", "time": 1011, "from": EVM.to_lowercase(), "to": OTHER, "asset": USDT, "amount": "32", "status": "included"}),
    );
}

#[tokio::test]
async fn evm_update_keeps_the_balance_when_the_transactions_fail() {
    let (chain, url) = evm_chain().await;
    let gateway = Gateway::start(&evm_config(&url)).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, subscribe(&[EVM])).await;

    {
        let mut scene = chain.lock().unwrap();
        scene.block = 11;
        scene.native = "0x2".to_string();
        scene.broken_logs = true;
    }
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["balances"][0]["amount"], "2");
    assert_eq!(update["params"]["txs"], Value::Null, "客户端看到空值要自己补取");
}

#[tokio::test]
async fn snapshot_returns_only_the_requested_assets_and_isolates_failures() {
    let (_, url) = evm_chain().await;
    let dead = dead_url().await;
    let config = format!(
        "[networks.ethereum]\nupstreams = ['{url}']\ntokens = ['{USDT}', '{USDC}']\n[networks.sepolia]\nupstreams = ['{dead}']\n"
    );
    let gateway = Gateway::start(&config).await;
    let body = json!({"items": [
        {"network": "ethereum", "address": EVM},
        {"network": "ethereum", "address": EVM, "contract": USDT.to_lowercase()},
        {"network": "sepolia", "address": EVM},
    ]});
    let results = snapshot(&gateway, &body).await;
    let ethereum = results.iter().find(|row| row["network"] == "ethereum").unwrap();
    assert_eq!(ethereum["height"], 10);
    assert_eq!(ethereum["addresses"], json!([EVM]));
    assert_eq!(
        ethereum["balances"],
        json!([
            {"address": EVM, "asset": "native", "amount": "1"},
            {"address": EVM, "asset": USDT, "amount": "7"},
        ]),
    );
    assert_eq!(ethereum["history"], Value::Null, "没有索引服务");
    let sepolia = results.iter().find(|row| row["network"] == "sepolia").unwrap();
    assert_eq!(sepolia["error"], "upstream_unavailable");
    assert_eq!(sepolia["addresses"], json!([EVM]));
}

#[tokio::test]
async fn snapshot_groups_addresses_by_their_checkpoint() {
    let esplora = Router::new()
        .route("/blocks/tip/height", get(|| async { "100" }))
        .route(
            "/address/{address}",
            get(|Path(_): Path<String>| async {
                axum::Json(json!({"chain_stats": {"funded_txo_sum": 5, "spent_txo_sum": 0}}))
            }),
        )
        .route("/address/{address}/txs", get(|| async { axum::Json(json!([])) }));
    let url = mock(esplora).await;
    let gateway = Gateway::start(&format!("[networks.bitcoin]\nupstreams = ['{url}']\n")).await;
    let other = "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr";
    let body = json!({"items": [
        {"network": "bitcoin", "address": BTC, "since": "95"},
        {"network": "bitcoin", "address": other},
    ]});
    let results = snapshot(&gateway, &body).await;
    assert_eq!(results.len(), 2, "新地址没有水位，要单独查");
    let known = results.iter().find(|row| row["addresses"] == json!([BTC])).unwrap();
    assert_eq!(known["history"]["unchanged"], true);
    assert_eq!(known["height"], 100);
    let fresh = results.iter().find(|row| row["addresses"] == json!([other])).unwrap();
    assert_eq!(fresh["history"]["unchanged"], false);
    assert_eq!(fresh["history"]["checkpoint"], "95");
}

/// 慢的网络不能拖住快的网络。
#[tokio::test]
async fn snapshot_sends_each_group_as_soon_as_it_is_ready() {
    let (_, fast) = evm_chain().await;
    let hang = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow = format!("http://{}", hang.local_addr().unwrap());
    let config = format!(
        "upstream_timeout_ms = 1000\n[networks.ethereum]\nupstreams = ['{fast}']\n[networks.sepolia]\nupstreams = ['{slow}']\n"
    );
    let gateway = Gateway::start(&config).await;
    let body =
        json!({"items": [{"network": "sepolia", "address": EVM}, {"network": "ethereum", "address": EVM, "group": 1}]});
    let url = format!("{}/v1/snapshot", gateway.base);
    let mut response = reqwest::Client::new().post(url).json(&body).send().await.unwrap();
    let first = timeout(Duration::from_millis(500), response.chunk())
        .await
        .expect("快的网络要先送达")
        .unwrap()
        .unwrap();
    let first: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(first["network"], "ethereum");
    drop(hang);
}

/// HyperSync 的响应：一笔原生币收款、一笔零金额调用、一笔失败的调用、一条代币转账。
fn hypersync_reply() -> Value {
    json!({
        "next_block": 201,
        "archive_height": 500,
        "data": [{
            "blocks": [{"number": 100, "timestamp": "0x3e8"}, {"number": 200, "timestamp": "0x7d0"}],
            "transactions": [
                {"block_number": 100, "from": OTHER, "to": EVM.to_lowercase(), "value": "0x0a", "status": 1, "hash": "0xaa"},
                {"block_number": 100, "from": EVM.to_lowercase(), "to": USDT.to_lowercase(), "value": "0x", "status": 1, "hash": "0xbb"},
                {"block_number": 200, "from": EVM.to_lowercase(), "to": OTHER, "value": "0x00", "status": 0, "hash": "0xcc"},
            ],
            "logs": [{
                "address": USDT.to_lowercase(),
                "block_number": 100,
                "transaction_hash": "0xbb",
                "topic0": TRANSFER,
                "topic1": padded(EVM),
                "topic2": padded(OTHER),
                "data": format!("{:#066x}", 32),
            }],
        }],
    })
}

#[tokio::test]
async fn hypersync_history_uses_the_token_and_pages_by_block() {
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log = seen.clone();
    let indexer = Router::new().route(
        "/query",
        post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let log = log.clone();
            async move {
                let token = headers["authorization"].to_str().unwrap().to_string();
                log.lock().unwrap().push((token, body));
                axum::Json(hypersync_reply())
            }
        }),
    );
    let indexer = mock(indexer).await;
    let (_, url) = evm_chain().await;
    let config = format!(
        "[networks.ethereum]\nupstreams = ['{url}']\ntokens = ['{USDT}']\nindexer = '{indexer}'\nindexer_token = 'secret'\n"
    );
    let gateway = Gateway::start(&config).await;

    let (status, reply) = gateway
        .post("/v1/ethereum/history", &json!({"addresses": [EVM], "since": "9"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["checkpoint"], "5", "水位是节点的 finalized");
    assert_eq!(reply["cursor"], json!({"0": "201"}).to_string(), "还没扫到最新的块");
    let items = reply["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{items:?}");
    assert_eq!(items[0]["hash"], "0xcc", "新的在前");
    assert_eq!(items[0]["status"], "failed");
    assert_eq!(items[0]["time"], 2000);
    let token = items.iter().find(|item| item["asset"] == USDT).unwrap();
    assert_eq!(
        token,
        &json!({"hash": "0xbb", "time": 1000, "from": EVM.to_lowercase(), "to": OTHER, "asset": USDT, "amount": "32", "status": "included"}),
    );

    let cursor = reply["cursor"].as_str().unwrap();
    gateway
        .post("/v1/ethereum/history", &json!({"addresses": [EVM], "cursor": cursor}))
        .await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, "Bearer secret");
    assert_eq!(seen[0].1["from_block"], 10, "从水位之后开始");
    assert_eq!(seen[1].1["from_block"], 201, "按游标接着扫");
    assert_eq!(seen[0].1["transactions"][0]["from"], json!([EVM.to_lowercase()]));
}

#[tokio::test]
async fn snapshot_validates_before_it_reaches_the_upstream() {
    let (_, url) = evm_chain().await;
    let gateway = Gateway::start(&evm_config(&url)).await;
    let item = |network: &str, address: &str| json!({"network": network, "address": address});
    let cases = [
        (json!({"items": [item("dogecoin", EVM)]}), "unknown_network"),
        (json!({"items": [item("ethereum", "0x12")]}), "invalid_address"),
        (
            json!({"items": [{"network": "ethereum", "address": EVM, "contract": "0x12"}]}),
            "invalid_address",
        ),
        (
            json!({"items": [{"network": "ethereum", "address": EVM, "since": "不是水位"}]}),
            "invalid_request",
        ),
        (
            json!({"items": vec![item("ethereum", EVM); 1001]}),
            "too_many_addresses",
        ),
        (json!({}), "invalid_request"),
    ];
    for (body, code) in cases {
        let (status, reply) = gateway.post("/v1/snapshot", &body).await;
        assert_ne!(status, StatusCode::OK, "{code}");
        assert_eq!(error_code(&reply), code);
    }
}

// ---- 长连接：没有动静的地址不发余额请求 ----

/// 同一个地址上提供 HTTP 和 WebSocket 的模拟节点。
#[derive(Clone)]
struct SocketNode {
    url: String,
    /// 客户端发来的订阅，按收到的顺序。
    subscribed: Arc<Mutex<Vec<Value>>>,
    /// 发给所有连接的推送。
    pushes: broadcast::Sender<String>,
    calls: Calls,
}

impl SocketNode {
    async fn start(
        ack: fn(u64) -> Value,
        reply: impl Fn(&str, &Value) -> Value + Clone + Send + Sync + 'static,
    ) -> Self {
        let (pushes, _) = broadcast::channel(16);
        let node = Self {
            url: String::new(),
            subscribed: Arc::default(),
            pushes,
            calls: Calls::default(),
        };
        let (state, log) = (node.clone(), node.calls.clone());
        let rpc = move |axum::Json(body): axum::Json<Value>| {
            let (reply, log) = (reply.clone(), log.clone());
            async move {
                let answer = |call: &Value| {
                    let method = call["method"].as_str().unwrap();
                    log.record(method);
                    let mut response = reply(method, &call["params"]);
                    response["jsonrpc"] = json!("2.0");
                    response["id"] = call["id"].clone();
                    response
                };
                axum::Json(match body.as_array() {
                    Some(batch) => Value::Array(batch.iter().map(answer).collect()),
                    None => answer(&body),
                })
            }
        };
        let upgrade = move |upgrade: WebSocketUpgrade| {
            let state = state.clone();
            async move { upgrade.on_upgrade(move |socket| state.serve(socket, ack)) }
        };
        let url = mock(Router::new().route("/", post(rpc).get(upgrade))).await;
        Self { url, ..node }
    }

    async fn serve(self, mut socket: WebSocket, ack: fn(u64) -> Value) {
        let mut pushes = self.pushes.subscribe();
        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    let Some(Ok(WsMessage::Text(text))) = incoming else { return };
                    let call: Value = serde_json::from_str(&text).unwrap();
                    self.subscribed.lock().unwrap().push(call.clone());
                    let id = call["id"].as_u64().unwrap();
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": ack(id)}).to_string();
                    if socket.send(WsMessage::text(reply)).await.is_err() {
                        return;
                    }
                }
                Ok(push) = pushes.recv() => {
                    if socket.send(WsMessage::text(push)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    fn methods(&self, method: &str) -> Vec<Value> {
        let calls = self.subscribed.lock().unwrap();
        calls.iter().filter(|call| call["method"] == method).cloned().collect()
    }

    async fn wait_for(&self, method: &str, count: usize) {
        timeout(Duration::from_secs(5), async {
            while self.methods(method).len() < count {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("等 {method} 超时"));
    }

    fn push(&self, message: Value) {
        self.pushes.send(message.to_string()).unwrap();
    }
}

fn new_head(block: u64) -> Value {
    json!({"method": "eth_subscription", "params": {"subscription": "0xhead", "result": {"number": format!("{block:#x}")}}})
}

async fn evm_socket_node(chain: &Chain) -> SocketNode {
    let node = chain.clone();
    SocketNode::start(
        |id| json!(format!("{id:#x}")),
        move |method, params| reply(&node, method, params),
    )
    .await
}

/// 只有高度时不访问上游，代币由节点推送，订阅多少地址都不用每轮 `balanceOf`。
#[tokio::test]
async fn evm_tokens_are_pushed_by_the_node_not_polled() {
    let (chain, _) = evm_chain().await;
    let node = evm_socket_node(&chain).await;
    let gateway = Gateway::start(&evm_config(&node.url)).await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, subscribe(&[EVM])).await["result"].is_string());
    node.wait_for("eth_subscribe", 3).await;
    let filters = node.methods("eth_subscribe");
    assert_eq!(filters[0]["params"], json!(["newHeads"]));
    assert_eq!(filters[1]["params"][1]["topics"][1], json!([padded(EVM)]), "转出");
    assert_eq!(filters[2]["params"][1]["topics"][2], json!([padded(EVM)]), "转入");

    let token_calls = node.calls.count("eth_call");
    chain.lock().unwrap().block = 11;
    node.push(new_head(11));
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["height"], 11);
    assert_eq!(update["params"]["balances"], json!([]));
    assert_eq!(update["params"]["txs"], json!([]));
    assert_eq!(node.calls.count("eth_call"), token_calls, "没有动静不查代币余额");

    let log = json!({
        "address": USDT.to_lowercase(),
        "blockNumber": "0xb",
        "transactionHash": "0xdd",
        "topics": [TRANSFER, padded(OTHER), padded(EVM)],
        "data": format!("{:#066x}", 7),
    });
    chain.lock().unwrap().logs.push(log.clone());
    chain.lock().unwrap().token = 9;
    node.push(json!({"method": "eth_subscription", "params": {"subscription": "0xlogs", "result": log}}));
    let update = next_json(&mut ws).await;
    let balances = update["params"]["balances"].as_array().unwrap();
    assert!(
        balances.iter().any(|row| row["asset"] == USDT && row["amount"] == "9"),
        "{balances:?}"
    );
    assert_eq!(update["params"]["txs"][0]["hash"], "0xdd", "推送后不用等下一个块");
}

/// 原生币没有日志，内部转账也不在交易列表里，只有余额能发现。
#[tokio::test]
async fn evm_native_changes_are_found_by_the_balance_batch() {
    let (chain, _) = evm_chain().await;
    let node = evm_socket_node(&chain).await;
    let gateway = Gateway::start(&evm_config(&node.url)).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, subscribe(&[EVM])).await;
    node.wait_for("eth_subscribe", 3).await;

    chain.lock().unwrap().block = 11;
    node.push(new_head(11));
    next_json(&mut ws).await;
    sleep(Duration::from_millis(3100)).await;
    {
        let mut scene = chain.lock().unwrap();
        scene.block = 12;
        scene.native = "0x5".to_string();
    }
    node.push(new_head(12));
    let update = timeout(Duration::from_secs(5), async {
        loop {
            let update = next_json(&mut ws).await;
            if !update["params"]["balances"].as_array().unwrap().is_empty() {
                return update;
            }
        }
    })
    .await
    .expect("余额变化要推送");
    assert_eq!(update["params"]["balances"][0]["amount"], "5");
}

/// 原生币隔几秒才查一次，发现变化时高度可能已经过了这笔交易所在的块，交易不能漏掉。
#[tokio::test]
async fn evm_native_tx_is_found_when_the_change_is_noticed_blocks_later() {
    let (chain, _) = evm_chain().await;
    let node = evm_socket_node(&chain).await;
    let gateway = Gateway::start(&evm_config(&node.url)).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, subscribe(&[EVM])).await;
    node.wait_for("eth_subscribe", 3).await;
    chain.lock().unwrap().block = 11;
    node.push(new_head(11));
    next_json(&mut ws).await;
    // 等这一块的余额基线取完，否则这次变化算在基线里。
    sleep(Duration::from_millis(300)).await;

    {
        let mut scene = chain.lock().unwrap();
        scene.block = 12;
        scene.native = "0x5".to_string();
        let tx = json!({"hash": "0xee", "from": OTHER, "to": EVM.to_lowercase(), "value": "0x4"});
        scene.txs.insert(12, vec![tx]);
    }
    node.push(new_head(12));
    while next_json(&mut ws).await["params"]["height"] != 12 {}
    sleep(Duration::from_millis(3100)).await;
    chain.lock().unwrap().block = 13;
    node.push(new_head(13));
    let update = timeout(Duration::from_secs(4), async {
        loop {
            let update = next_json(&mut ws).await;
            if !update["params"]["balances"].as_array().unwrap().is_empty() {
                return update;
            }
        }
    })
    .await
    .expect("余额变化要推送");
    assert_eq!(update["params"]["txs"][0]["hash"], "0xee");
}

/// 订阅之后的第一个块就带着变化时，长连接还没有基线，这笔不能丢。
#[tokio::test]
async fn evm_change_in_the_first_block_after_subscribing_is_not_lost() {
    let (chain, _) = evm_chain().await;
    let node = evm_socket_node(&chain).await;
    let gateway = Gateway::start(&evm_config(&node.url)).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, subscribe(&[EVM])).await;
    node.wait_for("eth_subscribe", 3).await;

    {
        let mut scene = chain.lock().unwrap();
        scene.block = 11;
        scene.native = "0x5".to_string();
    }
    node.push(new_head(11));
    let update = timeout(Duration::from_secs(4), async {
        loop {
            let update = next_json(&mut ws).await;
            if !update["params"]["balances"].as_array().unwrap().is_empty() {
                return update;
            }
        }
    })
    .await
    .expect("第一个块里的变化要推送");
    assert_eq!(update["params"]["balances"][0]["amount"], "5");
}

/// 账户变化由节点推送，slot 再快也没有余额请求。
#[tokio::test]
async fn solana_changes_are_pushed_by_the_node_not_polled() {
    const OWNER: &str = "11111111111111111111111111111111";
    const ACCOUNT: &str = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
    let solana = Arc::new(Mutex::new(5u64));
    let lamports = solana.clone();
    let node = SocketNode::start(
        |id| json!(id),
        move |method, _| match method {
            "getBalance" => json!({"result": {"value": *lamports.lock().unwrap()}}),
            "getTokenAccountsByOwner" => {
                json!({"result": {"value": [{"pubkey": ACCOUNT, "account": {"data": {"parsed": {"info": {
                    "mint": ACCOUNT, "tokenAmount": {"amount": "3"},
                }}}}}]}})
            }
            "getSignaturesForAddress" => ok(json!([])),
            "getSlot" => ok(json!(100)),
            _ => panic!("没有模拟 {method}"),
        },
    )
    .await;
    let config = format!(
        "ws_poll_interval_ms = 10\n[networks.solana]\nupstreams = ['{}']\ntokens = ['{ACCOUNT}']\n",
        node.url
    );
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await;
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe",
                         "params": {"channel": "addresses", "network": "solana", "addresses": [OWNER]}});
    assert!(call(&mut ws, message).await["result"].is_string());
    node.wait_for("accountSubscribe", 2).await;
    node.wait_for("logsSubscribe", 1).await;
    let accounts: Vec<Value> = node
        .methods("accountSubscribe")
        .iter()
        .map(|c| c["params"][0].clone())
        .collect();
    assert_eq!(accounts, [json!(OWNER), json!(ACCOUNT)], "代币在独立的账户上");
    assert_eq!(
        node.methods("logsSubscribe")[0]["params"][0]["mentions"],
        json!([OWNER])
    );
    assert_eq!(node.methods("logsSubscribe")[0]["params"][1]["commitment"], "confirmed");

    // 订阅生效后对账一次，之后 slot 不再触发余额查询。
    node.push(json!({"method": "slotNotification", "params": {"result": {"slot": 199}}}));
    next_json(&mut ws).await;
    sleep(Duration::from_millis(50)).await;
    let polls = node.calls.count("getBalance");
    for slot in 200..203 {
        node.push(json!({"method": "slotNotification", "params": {"result": {"slot": slot}}}));
    }
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["balances"], json!([]));
    assert_eq!(node.calls.count("getBalance"), polls, "slot 不触发余额查询");

    *solana.lock().unwrap() = 9;
    let subscription = node.methods("accountSubscribe")[1]["id"].clone();
    node.push(json!({"method": "accountNotification", "params": {"subscription": subscription, "result": {}}}));
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["balances"][0]["amount"], "9");
}

/// TRON 没有推送，按块解析。一轮的请求数和地址数无关，地址没出现在区块里就不查它。
#[tokio::test]
async fn tron_parses_blocks_instead_of_querying_each_address() {
    const OWNER: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const OWNER_HEX: &str = "41a614f803b6fd780986a42c78ec9c7f77e6ded13c";
    let tip = Arc::new(Mutex::new(100u64));
    let accounts = Arc::new(Mutex::new(0usize));
    let (height, reads) = (tip.clone(), accounts.clone());
    let (block, parts, balance_tip) = (tip.clone(), tip.clone(), tip.clone());
    let node = Router::new()
        .route(
            "/wallet/getnowblock",
            get(move || {
                let number = *height.lock().unwrap();
                async move { axum::Json(json!({"block_header": {"raw_data": {"number": number}}})) }
            }),
        )
        .route(
            "/wallet/getblockbylimitnext",
            post(move |axum::Json(body): axum::Json<Value>| {
                let tip = *block.lock().unwrap();
                async move {
                    assert_eq!(body["endNum"], tip + 1);
                    let value = json!({"owner_address": "41ffffffffffffffffffffffffffffffffffffffff", "to_address": OWNER_HEX, "amount": 1});
                    axum::Json(json!({"block": [{"transactions": [{"raw_data": {"contract": [{"parameter": {"value": value}}]}}]}]}))
                }
            }),
        )
        .route("/jsonrpc", post(|| async { axum::Json(json!({"result": []})) }))
        .route(
            "/v1/accounts/{address}",
            get(move || {
                *reads.lock().unwrap() += 1;
                // 这笔转账在 101 块到账。
                let balance = if *balance_tip.lock().unwrap() > 100 { 8 } else { 5 };
                async move { axum::Json(json!({"data": [{"balance": balance}]})) }
            }),
        )
        .route("/v1/accounts/{address}/transactions", get(|| async { axum::Json(json!({"data": []})) }))
        .route("/v1/accounts/{address}/transactions/trc20", get(|| async { axum::Json(json!({"data": []})) }))
        .route(
            "/walletsolidity/getnowblock",
            get(|| async { axum::Json(json!({"block_header": {"raw_data": {"number": 90, "timestamp": 1000}}})) }),
        );
    let url = mock(node).await;
    let config = format!("ws_poll_interval_ms = 10\n[networks.tron]\nupstreams = ['{url}']\n");
    let gateway = Gateway::start(&config).await;
    let mut ws = gateway.ws().await;
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "subscribe",
                         "params": {"channel": "addresses", "network": "tron", "addresses": [OWNER]}});
    assert!(call(&mut ws, message).await["result"].is_string());
    sleep(Duration::from_millis(100)).await;
    let baseline = *accounts.lock().unwrap();
    *parts.lock().unwrap() = 101;
    // 按块轮询的间隔是 6 秒，等待消息的上限是 5 秒。
    sleep(Duration::from_secs(4)).await;
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["height"], 101);
    assert_eq!(update["params"]["balances"][0]["amount"], "8");
    assert_eq!(*accounts.lock().unwrap(), baseline + 1, "只查区块里出现的地址，查一次");
}

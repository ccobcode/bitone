//! WebSocket 的 tx、blocks、prices 频道，受信任代理，单 IP 连接数上限。

mod common;

use axum::http::{HeaderValue, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use common::{Calls, Gateway, Ws, call, dead_url, error_code, mock, next_json, ok, rpc_node};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const FAST_POLL: &str = "ws_poll_interval_ms = 10\nfollow_poll_interval_ms = 10";

fn hash() -> String {
    format!("0x{}", "a".repeat(64))
}

fn subscribe(id: u64, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "subscribe", "params": params})
}

fn tx_subscribe(id: u64, hash: &str) -> Value {
    subscribe(id, json!({"channel": "tx", "network": "ethereum", "hash": hash}))
}

/// 可以在测试中途修改的 EVM 节点，区块标签固定：latest 100，safe 90，finalized 80。
#[derive(Clone)]
struct Node {
    receipt: Arc<Mutex<Value>>,
    tx: Arc<Mutex<Value>>,
    height: Arc<Mutex<u64>>,
    calls: Calls,
}

impl Node {
    async fn start() -> (Self, String) {
        let (receipt, tx, height) = (
            Arc::new(Mutex::new(Value::Null)),
            Arc::new(Mutex::new(Value::Null)),
            Arc::new(Mutex::new(100)),
        );
        let (r, t, h) = (receipt.clone(), tx.clone(), height.clone());
        let (url, calls) = rpc_node(move |method, params| match (method, params[0].as_str()) {
            ("eth_blockNumber", _) => ok(json!(format!("{:#x}", *h.lock().unwrap()))),
            ("eth_getBlockByNumber", Some("safe")) => ok(json!({"number": "0x5a"})),
            ("eth_getBlockByNumber", Some("finalized")) => ok(json!({"number": "0x50"})),
            ("eth_getTransactionReceipt", _) => ok(r.lock().unwrap().clone()),
            ("eth_getTransactionByHash", _) => ok(t.lock().unwrap().clone()),
            (other, _) => panic!("{other}"),
        })
        .await;
        (
            Self {
                receipt,
                tx,
                height,
                calls,
            },
            url,
        )
    }

    fn set_receipt(&self, block: u64, status: &str) {
        *self.receipt.lock().unwrap() = json!({
            "blockNumber": format!("{block:#x}"), "status": status, "gasUsed": "0x1", "effectiveGasPrice": "0x1",
        });
    }

    fn polls(&self) -> usize {
        self.calls.count("eth_getTransactionReceipt")
    }

    async fn wait_for_poll(&self) {
        let start = self.polls();
        timeout(Duration::from_secs(5), async {
            while self.polls() == start {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("网关没有轮询上游");
    }
}

fn config(url: &str, extra: &str) -> String {
    format!("{extra}\n[networks.ethereum]\nupstreams = ['{url}']\n")
}

async fn assert_silent(ws: &mut Ws) {
    assert!(
        timeout(Duration::from_millis(50), next_json(ws)).await.is_err(),
        "不应该有推送"
    );
}

#[tokio::test]
async fn tx_channel_pushes_every_status_change_and_stops_at_finality() {
    let (node, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let reply = call(&mut ws, tx_subscribe(1, &hash())).await;
    let id = reply["result"].as_str().expect("订阅 ID").to_string();

    node.wait_for_poll().await;
    assert_silent(&mut ws).await;

    *node.tx.lock().unwrap() = json!({"hash": hash(), "blockNumber": null});
    let update = next_json(&mut ws).await;
    assert_eq!(update["method"], "update");
    assert!(update.get("id").is_none());
    assert_eq!(update["params"]["subscription"], id.as_str());
    assert_eq!(update["params"]["tx"]["status"], "pending");

    node.set_receipt(95, "0x1");
    let update = next_json(&mut ws).await;
    let http = gateway.post("/v1/ethereum/tx", &json!({"hashes": [hash()]})).await.1;
    assert_eq!(update["params"]["tx"], http["txs"][0], "格式要与 HTTP 接口相同");
    assert_eq!(update["params"]["tx"]["stage"], "latest");

    node.set_receipt(85, "0x1");
    assert_eq!(next_json(&mut ws).await["params"]["tx"]["stage"], "safe");

    node.set_receipt(80, "0x1");
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["tx"]["status"], "finalized");

    // 到达最终状态后不再轮询。
    sleep(Duration::from_millis(100)).await;
    let polls = node.polls();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(node.polls(), polls);
}

#[tokio::test]
async fn tx_channel_reports_failed_and_keeps_watching_until_final() {
    let (node, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    call(&mut ws, tx_subscribe(1, &hash())).await;

    node.set_receipt(95, "0x0");
    let update = next_json(&mut ws).await;
    assert_eq!(update["params"]["tx"]["status"], "failed");
    assert_eq!(update["params"]["tx"]["stage"], "latest");

    // 重组后这笔交易可能成功，所以失败但未最终确定时要继续看。
    node.set_receipt(95, "0x1");
    assert_eq!(next_json(&mut ws).await["params"]["tx"]["status"], "included");

    node.set_receipt(80, "0x0");
    let update = next_json(&mut ws).await;
    assert_eq!(
        (&update["params"]["tx"]["status"], &update["params"]["tx"]["stage"]),
        (&json!("failed"), &json!("finalized"))
    );
    sleep(Duration::from_millis(100)).await;
    let polls = node.polls();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(node.polls(), polls);
}

#[tokio::test]
async fn tx_channel_does_not_push_for_new_blocks_alone() {
    let (node, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, FAST_POLL)).await;
    node.set_receipt(95, "0x1");
    let mut ws = gateway.ws().await;
    call(&mut ws, tx_subscribe(1, &hash())).await;
    *node.height.lock().unwrap() = 101;
    node.wait_for_poll().await;
    assert_silent(&mut ws).await;
}

#[tokio::test]
async fn tx_channel_pushes_after_a_failed_baseline() {
    let dead = dead_url().await;
    let gateway = Gateway::start(&config(&dead, FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, tx_subscribe(1, &hash())).await["result"].is_string());

    // 上游恢复：同一个端口开始监听。
    let (node, url) = Node::start().await;
    node.set_receipt(95, "0x1");
    let proxy = Router::new().fallback(move |request: axum::extract::Request| {
        let url = url.clone();
        async move {
            let body = axum::body::to_bytes(request.into_body(), 1 << 20).await.unwrap();
            let reply = reqwest::Client::new()
                .post(&url)
                .header("content-type", "application/json")
                .body(body.to_vec())
                .send()
                .await
                .unwrap();
            let bytes = reply.bytes().await.unwrap();
            ([("content-type", "application/json")], bytes.to_vec())
        }
    });
    let listener = tokio::net::TcpListener::bind(dead.trim_start_matches("http://"))
        .await
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, proxy).await });

    assert_eq!(next_json(&mut ws).await["params"]["tx"]["status"], "included");
}

#[tokio::test]
async fn tx_channel_cap_is_per_connection_and_released_on_unsubscribe() {
    let (_, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, "")).await;
    let mut ws = gateway.ws().await;
    let mut first = Value::Null;
    for i in 0..100 {
        let reply = call(&mut ws, tx_subscribe(i, &format!("0x{i:064x}"))).await;
        assert!(reply["result"].is_string(), "{reply}");
        if i == 0 {
            first = reply["result"].clone();
        }
    }
    let reply = call(&mut ws, tx_subscribe(200, &hash())).await;
    assert_eq!(reply["error"]["data"]["code"], "invalid_request");

    let unsubscribe = json!({"jsonrpc": "2.0", "id": 201, "method": "unsubscribe", "params": [first]});
    assert_eq!(call(&mut ws, unsubscribe).await["result"], true);
    assert!(call(&mut ws, tx_subscribe(202, &hash())).await["result"].is_string());

    let mut other = gateway.ws().await;
    assert!(call(&mut other, tx_subscribe(1, &hash())).await["result"].is_string());
}

#[tokio::test]
async fn tx_channel_validates_network_and_hash() {
    let (_, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, "")).await;
    let mut ws = gateway.ws().await;
    let reply = call(&mut ws, tx_subscribe(1, "0x12")).await;
    assert_eq!(reply["error"]["data"]["code"], "invalid_request");
    let reply = call(
        &mut ws,
        subscribe(2, json!({"channel": "tx", "network": "dogecoin", "hash": hash()})),
    )
    .await;
    assert_eq!(reply["error"]["data"]["code"], "unknown_network");
    let reply = call(&mut ws, subscribe(3, json!({"channel": "tx", "network": "ethereum"}))).await;
    assert_eq!(reply["error"]["code"], -32602);
    let reply = call(&mut ws, subscribe(4, json!({"channel": "blocks"}))).await;
    assert_eq!(reply["error"]["code"], -32602);
}

#[tokio::test]
async fn subscription_ids_are_not_reused_after_a_rejected_subscribe() {
    let (_, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, "")).await;
    let mut ws = gateway.ws().await;
    let first = call(&mut ws, tx_subscribe(1, &hash())).await["result"].clone();
    call(&mut ws, tx_subscribe(2, "0x12")).await;
    let second = call(&mut ws, tx_subscribe(3, &hash())).await["result"].clone();
    assert_ne!(first, second);
}

#[tokio::test]
async fn blocks_channel_pushes_new_heights() {
    let (node, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, FAST_POLL)).await;
    let mut ws = gateway.ws().await;
    let reply = call(
        &mut ws,
        subscribe(1, json!({"channel": "blocks", "network": "ethereum"})),
    )
    .await;
    let id = reply["result"].as_str().expect("订阅 ID").to_string();
    assert_silent(&mut ws).await;

    *node.height.lock().unwrap() = 123;
    // 高度缓存几秒，这里要等缓存过期。
    let update = timeout(Duration::from_secs(6), async {
        loop {
            let message = next_json(&mut ws).await;
            if message["method"] == "update" {
                return message;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(update["params"], json!({"subscription": id, "height": 123}));
}

#[tokio::test]
async fn feed_subscriptions_are_capped_per_connection() {
    let (_, url) = Node::start().await;
    let gateway = Gateway::start(&config(&url, "")).await;
    let mut ws = gateway.ws().await;
    let blocks = json!({"channel": "blocks", "network": "ethereum"});
    for i in 0..18 {
        assert!(call(&mut ws, subscribe(i, blocks.clone())).await["result"].is_string());
    }
    let reply = call(&mut ws, subscribe(99, blocks)).await;
    assert_eq!(reply["error"]["data"]["code"], "invalid_request");
    let reply = call(&mut ws, subscribe(100, json!({"channel": "prices"}))).await;
    assert_eq!(reply["error"]["data"]["code"], "invalid_request");
}

#[tokio::test]
async fn prices_channel_without_an_upstream_stays_quiet() {
    let gateway = Gateway::start(FAST_POLL).await;
    let mut ws = gateway.ws().await;
    assert!(call(&mut ws, subscribe(1, json!({"channel": "prices"}))).await["result"].is_string());
    assert_silent(&mut ws).await;
}

#[tokio::test]
async fn prices_channel_pushes_after_a_failed_baseline() {
    let replies = Arc::new(Mutex::new(vec![
        (StatusCode::OK, json!({"bitcoin": {"usd": 77693}})),
        (StatusCode::TOO_MANY_REQUESTS, json!({})),
    ]));
    let upstream = Router::new().route(
        "/",
        get(move || {
            let replies = replies.clone();
            async move {
                let (status, body) = replies.lock().unwrap().pop().unwrap();
                (status, Json(body))
            }
        }),
    );
    let url = mock(upstream).await;
    let text = format!("{FAST_POLL}\n[prices]\nurl = '{url}/'\n[prices.ids]\n'bitcoin:native' = 'bitcoin'\n");
    let gateway = Gateway::start(&text).await;
    let mut ws = gateway.ws().await;
    let id = call(&mut ws, subscribe(1, json!({"channel": "prices"}))).await["result"].clone();
    // 失败只缓存 1 秒，之后的成功结果要推送出来。
    let update = next_json(&mut ws).await;
    assert_eq!(
        update["params"],
        json!({"subscription": id, "prices": {"bitcoin:native": "77693"}})
    );
}

// ---- 受信任代理 ----

async fn limited_status(gateway: &Gateway, forwarded: Option<&str>) -> StatusCode {
    let mut request = reqwest::Client::new().get(format!("{}/v1/version", gateway.base));
    if let Some(ip) = forwarded {
        request = request.header("x-forwarded-for", ip);
    }
    request.send().await.unwrap().status()
}

#[tokio::test]
async fn forwarded_header_is_ignored_by_default() {
    let gateway = Gateway::start("rate_limit_burst = 1\nrate_limit_per_second = 0.001").await;
    assert_eq!(limited_status(&gateway, Some("1.1.1.1")).await, StatusCode::OK);
    assert_eq!(
        limited_status(&gateway, Some("2.2.2.2")).await,
        StatusCode::TOO_MANY_REQUESTS,
        "伪造转发头不能换一个桶"
    );
}

#[tokio::test]
async fn forwarded_header_is_ignored_from_other_peers() {
    let config = "rate_limit_burst = 1\nrate_limit_per_second = 0.001\ntrusted_proxies = ['10.0.0.1']";
    let gateway = Gateway::start(config).await;
    assert_eq!(limited_status(&gateway, Some("1.1.1.1")).await, StatusCode::OK);
    assert_eq!(
        limited_status(&gateway, Some("2.2.2.2")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn trusted_proxies_give_each_client_its_own_bucket() {
    let config = "rate_limit_burst = 1\nrate_limit_per_second = 0.001\ntrusted_proxies = ['127.0.0.1']";
    let gateway = Gateway::start(config).await;
    assert_eq!(limited_status(&gateway, Some("1.1.1.1")).await, StatusCode::OK);
    assert_eq!(limited_status(&gateway, Some("2.2.2.2")).await, StatusCode::OK);
    assert_eq!(
        limited_status(&gateway, Some("1.1.1.1")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // 客户端自己写在左边的内容不算数。
    assert_eq!(
        limited_status(&gateway, Some("9.9.9.9, 2.2.2.2")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // 没有转发头时就是代理自己。
    assert_eq!(limited_status(&gateway, None).await, StatusCode::OK);
    assert_eq!(limited_status(&gateway, None).await, StatusCode::TOO_MANY_REQUESTS);
}

// ---- 单 IP 连接数 ----

async fn connect_forwarded(gateway: &Gateway, ip: &str) -> Result<Ws, StatusCode> {
    let mut request = gateway.ws.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("x-forwarded-for", HeaderValue::from_str(ip).unwrap());
    match connect_async(request).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(response.status()),
        Err(error) => panic!("{error}"),
    }
}

#[tokio::test]
async fn websocket_connections_are_capped_per_ip() {
    let gateway = Gateway::start("ws_max_connections_per_ip = 2").await;
    let first = gateway.ws().await;
    let _second = gateway.ws().await;
    let error = connect_async(gateway.ws.as_str()).await.unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("{error}")
    };
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
    assert_eq!(error_code(&body), "rate_limited");

    // 连接断开后名额归还。
    drop(first);
    timeout(Duration::from_secs(5), async {
        while connect_async(gateway.ws.as_str()).await.is_err() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("名额没有归还");
}

#[tokio::test]
async fn connection_cap_uses_the_client_ip_behind_a_trusted_proxy() {
    let gateway = Gateway::start("ws_max_connections_per_ip = 1\ntrusted_proxies = ['127.0.0.1']").await;
    let _a = connect_forwarded(&gateway, "1.1.1.1").await.unwrap();
    let _b = connect_forwarded(&gateway, "2.2.2.2").await.unwrap();
    assert_eq!(
        connect_forwarded(&gateway, "1.1.1.1").await.err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
}

#[tokio::test]
async fn non_websocket_requests_do_not_take_a_slot() {
    let gateway = Gateway::start("ws_max_connections_per_ip = 1").await;
    for _ in 0..3 {
        assert_ne!(gateway.get("/v1/ws").await.status(), StatusCode::SWITCHING_PROTOCOLS);
    }
    let _ws = gateway.ws().await;
}

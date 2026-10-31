//! 集成测试共用的部分:真实监听端口的网关,本地模拟上游,WebSocket 工具.
#![allow(dead_code)]

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use bitone_gateway::config::Config;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub async fn mock(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    url
}

/// 一个已经关闭的端口,连接会被拒绝.
pub async fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

/// 节点调用过的方法名,按收到的顺序.
#[derive(Clone, Default)]
pub struct Calls(Arc<Mutex<Vec<String>>>);

impl Calls {
    pub fn record(&self, method: &str) {
        self.0.lock().unwrap().push(method.to_string());
    }

    pub fn count(&self, method: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|m| *m == method).count()
    }

    pub fn total(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

pub fn ok(result: Value) -> Value {
    json!({"result": result})
}

pub fn rpc_failure(code: i64, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

/// JSON-RPC 模拟节点.`reply` 按方法和参数返回 `ok(...)` 或 `rpc_failure(...)`.
/// 批量调用的响应故意反序,验证网关按 id 归位.
pub async fn rpc_node(reply: impl Fn(&str, &Value) -> Value + Clone + Send + Sync + 'static) -> (String, Calls) {
    let calls = Calls::default();
    let log = calls.clone();
    let handler = move |Json(body): Json<Value>| {
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
            match body.as_array() {
                Some(batch) => Json(Value::Array(batch.iter().rev().map(answer).collect())),
                None => Json(answer(&body)),
            }
        }
    };
    (mock(Router::new().route("/", post(handler))).await, calls)
}

pub struct Gateway {
    pub base: String,
    pub ws: String,
    pub stop: oneshot::Sender<()>,
    pub task: JoinHandle<io::Result<()>>,
}

impl Gateway {
    pub async fn start(config: &str) -> Self {
        let config = Config::parse(config).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let shutdown = async move {
            let _ = stopped.await;
        };
        let task = tokio::spawn(bitone_gateway::serve(config, listener, shutdown));
        Self {
            base: format!("http://{addr}"),
            ws: format!("ws://{addr}/v1/ws"),
            stop,
            task,
        }
    }

    pub async fn balances(&self, network: &str, addresses: &[&str]) -> (StatusCode, Value) {
        self.post(&format!("/v1/{network}/balances"), &json!({"addresses": addresses}))
            .await
    }

    pub async fn post(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let url = format!("{}{path}", self.base);
        let response = reqwest::Client::new().post(url).json(body).send().await.unwrap();
        (response.status(), response.json().await.unwrap())
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        reqwest::get(format!("{}{path}", self.base)).await.unwrap()
    }

    pub async fn get_json(&self, path: &str) -> (StatusCode, Value) {
        let response = self.get(path).await;
        (response.status(), response.json().await.unwrap())
    }

    pub async fn ws(&self) -> Ws {
        connect_async(self.ws.as_str()).await.unwrap().0
    }
}

pub fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub async fn send(ws: &mut Ws, message: Value) {
    ws.send(Message::text(message.to_string())).await.unwrap();
}

/// 下一条文本消息,跳过心跳.
pub async fn next_json(ws: &mut Ws) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Close(frame) => panic!("连接被关闭 {frame:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("等待消息超时")
}

pub async fn call(ws: &mut Ws, message: Value) -> Value {
    send(ws, message).await;
    next_json(ws).await
}

//! 到上游的 HTTP 和 JSON-RPC.

use super::{Broadcast, Failed};
use axum::http::{HeaderMap, Method};
use futures_util::{StreamExt, TryStreamExt, stream};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};

/// 单次查询里对同一上游的并发请求数.
const CONCURRENCY: usize = 8;

pub struct Http {
    client: Client,
    url: String,
    headers: HeaderMap,
}

/// 节点返回的 JSON-RPC 错误.
#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Value,
}

impl RpcError {
    /// 节点自身的问题不算拒绝:解析错误,方法不存在,内部错误,限流或节点不健康.
    pub fn is_rejection(&self) -> bool {
        !matches!(self.code, -32700 | -32603 | -32601 | -32600 | -32005)
    }
}

fn rpc_reply(mut reply: Value) -> Option<Result<Value, RpcError>> {
    if let Some(error) = reply.get("error").filter(|error| !error.is_null()) {
        let code = error.get("code")?.as_i64()?;
        let message = error.get("message")?.as_str()?.to_string();
        let data = error.get("data").cloned().unwrap_or(Value::Null);
        return Some(Err(RpcError { code, message, data }));
    }
    Some(Ok(reply.get_mut("result")?.take()))
}

impl Http {
    pub fn new(client: &Client, url: &str, headers: HeaderMap) -> Self {
        Self {
            client: client.clone(),
            url: url.trim_end_matches('/').to_string(),
            headers,
        }
    }

    pub fn socket_url(&self) -> String {
        self.url.replacen("http", "ws", 1)
    }

    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.url))
            .headers(self.headers.clone())
    }

    pub async fn get(&self, path: &str) -> Result<Value, Failed> {
        self.get_query(path, &[]).await
    }

    pub async fn get_query(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, Failed> {
        let response = self.request(Method::GET, path).query(query).send().await?;
        Ok(response.error_for_status()?.json().await?)
    }

    /// 404 表示上游没有这条记录,不算故障.
    pub async fn find_text(&self, path: &str) -> Result<Option<String>, Failed> {
        let response = self.request(Method::GET, path).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.text().await?))
    }

    pub async fn post(&self, body: &Value) -> Result<Value, Failed> {
        self.post_to("", body).await
    }

    pub async fn post_to(&self, path: &str, body: &Value) -> Result<Value, Failed> {
        let response = self.request(Method::POST, path).json(body).send().await?;
        Ok(response.error_for_status()?.json().await?)
    }

    /// 发一个 JSON-RPC 调用,保留连接失败的信息.节点的错误对象在内层返回.
    /// 有的服务商把节点错误放在 4xx 响应里,所以这里只把限流和 5xx 算作服务故障.
    pub async fn rpc_send(&self, method: &str, params: Value) -> Result<Result<Value, RpcError>, Broadcast> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response: Response = self.request(Method::POST, "").json(&body).send().await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(Broadcast::Unknown);
        }
        let reply: Value = response.json().await.map_err(|_| Broadcast::Unknown)?;
        rpc_reply(reply).ok_or(Broadcast::Unknown)
    }

    pub async fn rpc(&self, method: &str, params: Value) -> Result<Result<Value, RpcError>, Failed> {
        self.rpc_send(method, params).await.map_err(|_| Failed)
    }
}

/// 限制并发地跑完一组查询,结果保持输入顺序.
pub async fn run_all<F, T>(queries: Vec<F>) -> Result<Vec<T>, Failed>
where
    F: Future<Output = Result<Vec<T>, Failed>>,
{
    let nested: Vec<Vec<T>> = stream::iter(queries).buffered(CONCURRENCY).try_collect().await?;
    Ok(nested.into_iter().flatten().collect())
}

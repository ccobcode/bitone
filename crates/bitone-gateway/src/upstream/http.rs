//! 到上游的 HTTP 和 JSON-RPC.

use super::{Broadcast, Failed};
use axum::http::{HeaderMap, Method};
use futures_util::{StreamExt, TryStreamExt, stream};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::{Value, json};

/// 单次查询里对同一上游的并发请求数.
const CONCURRENCY: usize = 8;
/// 上游响应的上限,和 Electrum 的单行上限相同.超过时算这个上游这次不可用.
const MAX_RESPONSE: usize = 16 << 20;

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

/// 读完响应体.响应头里的长度可能没有,也可能是假的,所以边读边数.
async fn read_body(mut response: Response) -> Result<Vec<u8>, Failed> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err(Failed);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err(Failed);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub async fn read_json(response: Response) -> Result<Value, Failed> {
    serde_json::from_slice(&read_body(response).await?).map_err(|_| Failed)
}

pub async fn read_text(response: Response) -> Result<String, Failed> {
    Ok(String::from_utf8_lossy(&read_body(response).await?).into_owned())
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
        read_json(response.error_for_status()?).await
    }

    /// 404 表示上游没有这条记录,不算故障.
    pub async fn find_text(&self, path: &str) -> Result<Option<String>, Failed> {
        let response = self.request(Method::GET, path).send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(read_text(response.error_for_status()?).await?))
    }

    pub async fn post(&self, body: &Value) -> Result<Value, Failed> {
        self.post_to("", body).await
    }

    pub async fn post_to(&self, path: &str, body: &Value) -> Result<Value, Failed> {
        let response = self.request(Method::POST, path).json(body).send().await?;
        read_json(response.error_for_status()?).await
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
        let reply = read_json(response).await.map_err(|_| Broadcast::Unknown)?;
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

/// 和 `run_all` 一样,但每个查询各算各的,一个失败不影响其他的.
pub async fn run_each<F, T>(queries: Vec<F>) -> Vec<Result<T, Failed>>
where
    F: Future<Output = Result<T, Failed>>,
{
    stream::iter(queries).buffered(CONCURRENCY).collect().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use std::convert::Infallible;
    use tokio::net::TcpListener;

    const MIB: usize = 1 << 20;

    /// 分块发送,响应头里没有长度,只能边读边数.
    fn chunked(sizes: Vec<usize>) -> Body {
        let chunks = sizes.into_iter().map(|size| Ok::<_, Infallible>(vec![b' '; size]));
        Body::from_stream(stream::iter(chunks))
    }

    #[tokio::test]
    async fn oversized_responses_count_as_upstream_failures() {
        let router = Router::new()
            .route("/fits", get(|| async { chunked(vec![MIB; 15]) }))
            .route("/chunked", get(|| async { chunked(vec![MIB; 17]) }))
            .route("/declared", get(|| async { vec![b' '; MAX_RESPONSE + 1] }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });

        let http = Http::new(&Client::new(), &url, HeaderMap::new());
        assert!(matches!(http.find_text("/fits").await, Ok(Some(text)) if text.len() == 15 * MIB));
        assert!(http.find_text("/chunked").await.is_err());
        assert!(http.get("/declared").await.is_err());
    }
}

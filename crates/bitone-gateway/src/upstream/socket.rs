//! 到节点的长连接.

use super::Failed;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 新块推送停了这么久就当连接已经失效.比最慢的出块间隔长.
const HEAD_SILENCE: Duration = Duration::from_secs(90);

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 节点在 HTTP 地址上同时提供 WebSocket.
pub async fn open_socket(url: String) -> Result<Socket, Failed> {
    let (socket, _) = timeout(CONNECT_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| Failed)?
        .map_err(|_| Failed)?;
    Ok(socket)
}

pub async fn send_call(socket: &mut Socket, id: u64, method: &str, params: Value) -> Result<(), Failed> {
    let call = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    socket.send(Message::text(call.to_string())).await.map_err(|_| Failed)
}

/// 下一条 JSON.可以被 `select!` 打断.太久没有消息就当连接失效.
pub async fn next_message(socket: &mut Socket) -> Result<Value, Failed> {
    loop {
        let message = timeout(HEAD_SILENCE, socket.next()).await.map_err(|_| Failed)?;
        let message = message.ok_or(Failed)?.map_err(|_| Failed)?;
        // 心跳帧由库自动回复,这里跳过.
        let Ok(text) = message.to_text() else { continue };
        if let Ok(value) = serde_json::from_str(text) {
            return Ok(value);
        }
    }
}

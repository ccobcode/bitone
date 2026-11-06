//! HTTP 和 WebSocket 共用的错误响应.
//!
//! 消息默认是固定文本,不回显地址和上游内容.只有节点拒绝交易或调用时才带节点原文,
//! 这段文本由客户端自己的请求引起,只返回给它,不进日志.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// 节点原文的长度上限,上游内容不可信.
const MAX_NODE_TEXT: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    UnknownNetwork,
    InvalidAddress,
    TooManyAddresses,
    InvalidRequest,
    /// 接口不适用于这条链.
    WrongChain,
    InvalidHash,
    InvalidRawTx,
    TooManyHashes,
    TooManySubscriptions,
    BodyTooLarge,
    UpstreamUnavailable,
    RateLimited,
    /// 节点拒绝了只读调用,比如估算时合约回滚.
    CallRejected(String),
    BroadcastRejected(String),
}

fn node_text(text: &str) -> String {
    text.chars().take(MAX_NODE_TEXT).collect()
}

impl ApiError {
    pub fn call_rejected(text: &str) -> Self {
        ApiError::CallRejected(node_text(text))
    }

    pub fn broadcast_rejected(text: &str) -> Self {
        ApiError::BroadcastRejected(node_text(text))
    }

    /// HTTP 状态码,错误码,JSON-RPC 错误码.
    fn parts(&self) -> (StatusCode, &'static str, i32) {
        match self {
            ApiError::UnknownNetwork => (StatusCode::NOT_FOUND, "unknown_network", -32001),
            ApiError::InvalidAddress => (StatusCode::BAD_REQUEST, "invalid_address", -32002),
            ApiError::TooManyAddresses => (StatusCode::BAD_REQUEST, "too_many_addresses", -32003),
            ApiError::UpstreamUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "upstream_unavailable", -32004),
            ApiError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited", -32005),
            ApiError::BodyTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "invalid_request", -32006),
            ApiError::InvalidRequest
            | ApiError::WrongChain
            | ApiError::InvalidHash
            | ApiError::InvalidRawTx
            | ApiError::TooManyHashes
            | ApiError::TooManySubscriptions
            | ApiError::CallRejected(_) => (StatusCode::BAD_REQUEST, "invalid_request", -32006),
            ApiError::BroadcastRejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, "broadcast_rejected", -32007),
        }
    }

    pub fn code(&self) -> &'static str {
        self.parts().1
    }

    pub fn message(&self) -> &str {
        match self {
            ApiError::UnknownNetwork => "未知网络",
            ApiError::InvalidAddress => "地址格式错误",
            ApiError::TooManyAddresses => "地址数量超过上限",
            ApiError::InvalidRequest => "请求格式错误",
            ApiError::WrongChain => "这条链不支持此接口",
            ApiError::InvalidHash => "交易哈希格式错误",
            ApiError::InvalidRawTx => "原始交易格式错误",
            ApiError::TooManyHashes => "交易数量超过上限",
            ApiError::TooManySubscriptions => "订阅数量超过上限",
            ApiError::BodyTooLarge => "请求体过大",
            ApiError::UpstreamUnavailable => "上游不可用",
            ApiError::RateLimited => "请求过于频繁",
            ApiError::CallRejected(text) | ApiError::BroadcastRejected(text) => text,
        }
    }

    pub fn rpc_code(&self) -> i32 {
        self.parts().2
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, _) = self.parts();
        (
            status,
            Json(json!({"error": {"code": code, "message": self.message()}})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_text_is_truncated_on_a_character_boundary() {
        let error = ApiError::broadcast_rejected(&"拒".repeat(MAX_NODE_TEXT + 1));
        assert_eq!(error.message().chars().count(), MAX_NODE_TEXT);
    }
}

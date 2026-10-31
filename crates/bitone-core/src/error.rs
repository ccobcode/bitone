use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("路径格式错误")]
    PathFormat,
    #[error("路径不在白名单")]
    PathNotAllowed,
    #[error("路径与网络冲突")]
    PathNetworkMismatch,
    #[error("索引超过上限")]
    IndexTooLarge,
    #[error("公钥无效")]
    InvalidKey,
    #[error("不支持的二维码类型")]
    UnknownUrType,
    #[error("二维码帧无效")]
    InvalidFrame,
    #[error("二维码帧数超过上限")]
    TooManyFrames,
    #[error("数据超过大小上限")]
    TooLarge,
    #[error("数据嵌套过深")]
    TooDeep,
    #[error("数据解析失败")]
    Decode,
    #[error("协议版本不支持")]
    UnsupportedVersion,
    #[error("二维码生成失败")]
    QrEncode,
    #[error("签名无效")]
    BadSignature,
    #[error("清单版本过旧")]
    StaleManifest,
    #[error("请求和签名不匹配")]
    ResponseMismatch,
    #[error("交易参数无效")]
    InvalidParams,
    #[error("地址无效")]
    InvalidAddress,
    #[error("余额不足")]
    InsufficientFunds,
    #[error("金额太小,节点不会转发")]
    Dust,
}

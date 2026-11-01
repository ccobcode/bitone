//! 冷钱包专用:助记词,种子加密,派生.这个包不进网页端的构建产物.

pub mod keys;
pub mod mnemonic;
pub mod sign;
pub mod vault;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("助记词无效")]
    InvalidMnemonic,
    #[error("单词数量只能是 12,15,18,21 或 24")]
    InvalidWordCount,
    #[error("PIN 必须是 6 位数字")]
    InvalidPin,
    #[error("PIN 错误或数据损坏")]
    WrongPin,
    #[error("密文格式不支持")]
    UnsupportedVault,
    #[error("随机数生成失败")]
    Random,
    #[error("派生失败")]
    Derivation,
    #[error("这个请求不能签名")]
    Refused,
    #[error(transparent)]
    Core(#[from] bitone_core::Error),
}

//! 请求的解析和校验,公共接口和各条链的接口共用.校验都在调用上游之前,错误的请求不产生上游成本.

use super::{is_valid, is_valid_hash};
use crate::error::ApiError;
use crate::upstream::Networks;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::StatusCode;
use bitcoin::hex::FromHex;
use bitone_core::Network;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

pub const MAX_ADDRESSES: usize = 100;
pub const MAX_HASHES: usize = 100;

pub type Shared = State<Arc<Networks>>;
pub type Body = Result<Bytes, BytesRejection>;
pub type Reply = Result<Json<Value>, ApiError>;

/// 这个请求的上游成本.限流中间件放进请求扩展,里面是这个 IP 的令牌桶.
#[derive(Clone)]
pub struct Budget(Arc<dyn Fn(usize) -> bool + Send + Sync>);

impl Budget {
    /// `take` 扣掉给定数量的令牌,不够时返回 false.
    pub fn new(take: impl Fn(usize) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(take))
    }

    /// 校验通过后,在调用上游之前扣令牌,一个地址或哈希一个.请求本身的一个已经扣过.
    pub fn spend(&self, units: usize) -> Result<(), ApiError> {
        if (self.0)(units.saturating_sub(1)) {
            Ok(())
        } else {
            Err(ApiError::RateLimited)
        }
    }
}

#[derive(Deserialize)]
struct AddressesRequest {
    addresses: Vec<String>,
}

pub fn parse<T: DeserializeOwned>(body: Body) -> Result<T, ApiError> {
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::BodyTooLarge,
        _ => ApiError::InvalidRequest,
    })?;
    serde_json::from_slice(&body).map_err(|_| ApiError::InvalidRequest)
}

pub fn addresses_from(network: Network, body: Body) -> Result<Vec<String>, ApiError> {
    let request: AddressesRequest = parse(body)?;
    checked(network, request.addresses, MAX_ADDRESSES)
}

/// 校验数量和格式,再按原顺序去重.
pub fn checked(network: Network, addresses: Vec<String>, limit: usize) -> Result<Vec<String>, ApiError> {
    if addresses.len() > limit {
        return Err(ApiError::TooManyAddresses);
    }
    if !addresses.iter().all(|a| is_valid(network, a)) {
        return Err(ApiError::InvalidAddress);
    }
    let mut seen = HashSet::new();
    Ok(addresses.into_iter().filter(|a| seen.insert(a.clone())).collect())
}

/// 校验数量和格式,再按原顺序去重.
pub fn checked_hashes(network: Network, hashes: Vec<String>, limit: usize) -> Result<Vec<String>, ApiError> {
    if hashes.len() > limit {
        return Err(ApiError::TooManyHashes);
    }
    if !hashes.iter().all(|hash| is_valid_hash(network, hash)) {
        return Err(ApiError::InvalidHash);
    }
    let mut seen = HashSet::new();
    Ok(hashes.into_iter().filter(|hash| seen.insert(hash.clone())).collect())
}

pub fn checked_address(network: Network, address: String) -> Result<String, ApiError> {
    if is_valid(network, &address) {
        Ok(address)
    } else {
        Err(ApiError::InvalidAddress)
    }
}

/// 可选字段空着和不写是一回事.
pub fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

pub fn is_hex(text: &str) -> bool {
    Vec::<u8>::from_hex(text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";

    #[test]
    fn checked_hashes_rejects_and_deduplicates() {
        let hex = "ab".repeat(32);
        let same = vec![hex.clone(); 3];
        assert_eq!(checked_hashes(Network::Bitcoin, same.clone(), 3).unwrap(), vec![hex]);
        assert_eq!(checked_hashes(Network::Bitcoin, same, 2), Err(ApiError::TooManyHashes));
        assert_eq!(
            checked_hashes(Network::Bitcoin, vec!["x".into()], 2),
            Err(ApiError::InvalidHash)
        );
    }

    #[test]
    fn checked_counts_before_deduplicating() {
        let same = vec![EVM.to_string(); 3];
        assert_eq!(
            checked(Network::Ethereum, same.clone(), 3).unwrap(),
            vec![EVM.to_string()]
        );
        assert_eq!(checked(Network::Ethereum, same, 2), Err(ApiError::TooManyAddresses));
    }

    #[test]
    fn spend_takes_one_token_less_than_the_units() {
        let taken = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = taken.clone();
        let budget = Budget::new(move |extra| {
            log.lock().unwrap().push(extra);
            extra < 5
        });
        assert_eq!(budget.spend(0), Ok(()));
        assert_eq!(budget.spend(3), Ok(()));
        assert_eq!(budget.spend(6), Err(ApiError::RateLimited));
        assert_eq!(*taken.lock().unwrap(), vec![0, 2, 5]);
    }
}

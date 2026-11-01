//! 待签名请求和签名结果,见 docs/03-system.md 第 6 节.

use crate::Error;
use crate::protocol::{FrameEncoder, UR_RESPONSE, check_fields, check_limits, check_version};
use minicbor::bytes::ByteVec;
use minicbor::{Decode, Encode};

pub const KIND_TRANSACTION: u8 = 0;
pub const KIND_TYPED_DATA: u8 = 1;
/// 载荷是要签的 32 字节哈希.
pub const KIND_RAW_HASH: u8 = 2;
/// 载荷是 `personal_sign` 的消息,前缀由签名方加上.
pub const KIND_PERSONAL_SIGN: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct SignRequest {
    #[n(0)]
    pub version: u8,
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    pub id: [u8; 16],
    #[n(2)]
    #[cbor(with = "minicbor::bytes")]
    pub fingerprint: [u8; 4],
    #[n(3)]
    pub network: String,
    #[n(4)]
    pub kind: u8,
    /// 签名用的路径.Bitcoin 的路径在 PSBT 里,这里留空.
    #[n(5)]
    pub paths: Vec<String>,
    #[n(6)]
    #[cbor(with = "minicbor::bytes")]
    pub payload: Vec<u8>,
    /// 网页端的声明,比如 Solana 代币收款方的所有者.冷钱包验证通过才显示,不能作为判断依据.
    #[n(7)]
    pub hint: Option<String>,
    /// 链编号,和 `Chain` 相同.现在只随请求携带.用整数是因为旧手机要能带着以后才有的编号读完请求.
    #[n(8)]
    pub chain: Option<u8>,
}

impl SignRequest {
    pub fn encode(&self) -> Vec<u8> {
        minicbor::to_vec(self).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        check_limits(bytes)?;
        check_fields(bytes)?;
        let request: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        check_version(request.version)?;
        Ok(request)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct SignResponse {
    #[n(0)]
    pub version: u8,
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    pub id: [u8; 16],
    /// EVM,TRON 是 65 字节的 r,s,恢复位.Solana 是 64 字节.Bitcoin 每个输入一项.
    #[n(2)]
    pub signatures: Vec<ByteVec>,
}

impl SignResponse {
    pub fn encode(&self) -> Vec<u8> {
        minicbor::to_vec(self).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        check_limits(bytes)?;
        check_fields(bytes)?;
        let response: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        check_version(response.version)?;
        Ok(response)
    }

    pub fn frames(&self, fragment_bytes: usize) -> Result<FrameEncoder, Error> {
        FrameEncoder::new(&self.encode(), UR_RESPONSE, fragment_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::{DisplayHex, FromHex};
    use crate::protocol::{VERSION, with_field};

    fn request() -> SignRequest {
        SignRequest {
            version: VERSION,
            id: [7; 16],
            fingerprint: [1, 2, 3, 4],
            network: "ethereum".into(),
            kind: KIND_TRANSACTION,
            paths: vec!["m/44'/60'/0'/0/0".into()],
            payload: vec![2, 1, 0],
            hint: None,
            chain: None,
        }
    }

    /// 加 `chain` 字段之前写下的字节,不能跟着代码变.
    const OLD_REQUEST: &str = concat!(
        "a7000101500707070707070707070707070707070702440102030403",
        "68657468657265756d04000581706d2f3434272f3630272f30272f302f300643020100",
    );

    fn response() -> SignResponse {
        SignResponse {
            version: VERSION,
            id: [7; 16],
            signatures: vec![ByteVec::from(vec![1u8; 65])],
        }
    }

    #[test]
    fn request_round_trip() {
        assert_eq!(SignRequest::decode(&request().encode()).unwrap(), request());
        let hinted = SignRequest {
            hint: Some("owner".into()),
            ..request()
        };
        assert_eq!(SignRequest::decode(&hinted.encode()).unwrap(), hinted);
    }

    #[test]
    fn response_round_trip() {
        assert_eq!(SignResponse::decode(&response().encode()).unwrap(), response());
    }

    #[test]
    fn a_newer_clients_fields_do_not_block_an_old_phone() {
        let bytes = request().encode();
        for key in [9, 10] {
            assert_eq!(SignRequest::decode(&with_field(&bytes, key, 1)), Ok(request()), "{key}");
        }
        let bytes = response().encode();
        assert_eq!(SignResponse::decode(&with_field(&bytes, 3, 0)), Ok(response()));
    }

    #[test]
    fn an_old_reader_accepts_a_higher_version_with_extra_keys() {
        let newer = SignRequest {
            version: VERSION + 8,
            ..request()
        };
        let bytes = with_field(&with_field(&newer.encode(), 20, 1), 21, 2);
        assert_eq!(SignRequest::decode(&bytes), Ok(newer));
        let newer = SignResponse {
            version: VERSION + 8,
            ..response()
        };
        assert_eq!(SignResponse::decode(&with_field(&newer.encode(), 20, 1)), Ok(newer));
    }

    #[test]
    fn version_zero_and_missing_required_fields_still_fail() {
        let bytes = SignRequest {
            version: 0,
            ..request()
        }
        .encode();
        assert_eq!(SignRequest::decode(&bytes), Err(Error::UnsupportedVersion));
        // 去掉最后一项的载荷,其余字段都在.
        let mut missing = request().encode();
        missing.truncate(missing.len() - 5);
        missing[0] -= 1;
        assert_eq!(SignRequest::decode(&missing), Err(Error::Decode));
    }

    #[test]
    fn an_old_request_without_chain_decodes_and_keeps_its_bytes() {
        let old = Vec::<u8>::from_hex(OLD_REQUEST).unwrap();
        assert_eq!(SignRequest::decode(&old), Ok(request()));
        assert_eq!(request().encode().to_lower_hex_string(), old.to_lower_hex_string());
    }

    #[test]
    fn chain_is_carried_even_when_this_build_does_not_know_the_number() {
        for chain in [1, 200] {
            let carried = SignRequest {
                chain: Some(chain),
                ..request()
            };
            assert_eq!(SignRequest::decode(&carried.encode()), Ok(carried));
        }
    }

    #[test]
    fn duplicate_fields_are_rejected() {
        let bytes = request().encode();
        assert_eq!(
            SignRequest::decode(&with_field(&bytes, 4, KIND_TYPED_DATA)),
            Err(Error::Decode)
        );
        let bytes = response().encode();
        assert_eq!(
            SignResponse::decode(&with_field(&bytes, 0, VERSION)),
            Err(Error::Decode)
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = with_field(&request().encode(), 8, 0);
        bytes.push(0);
        assert_eq!(SignRequest::decode(&bytes), Err(Error::Decode));
    }
}

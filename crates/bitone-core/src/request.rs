//! 待签名请求和签名结果.

use crate::Error;
use crate::protocol::{FrameEncoder, UR_RESPONSE, VERSION, check_limits};
use minicbor::bytes::ByteVec;
use minicbor::{Decode, Encode};

pub const KIND_TRANSACTION: u8 = 0;
pub const KIND_TYPED_DATA: u8 = 1;

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
}

impl SignRequest {
    pub fn encode(&self) -> Vec<u8> {
        minicbor::to_vec(self).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        check_limits(bytes)?;
        let request: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        if request.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
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
        let response: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        if response.version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        Ok(response)
    }

    pub fn frames(&self, fragment_bytes: usize) -> Result<FrameEncoder, Error> {
        FrameEncoder::new(&self.encode(), UR_RESPONSE, fragment_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let request = SignRequest {
            version: VERSION,
            id: [7; 16],
            fingerprint: [1, 2, 3, 4],
            network: "ethereum".into(),
            kind: KIND_TRANSACTION,
            paths: vec!["m/44'/60'/0'/0/0".into()],
            payload: vec![2, 1, 0],
            hint: None,
        };
        assert_eq!(SignRequest::decode(&request.encode()).unwrap(), request);
    }

    #[test]
    fn response_round_trip() {
        let response = SignResponse {
            version: VERSION,
            id: [7; 16],
            signatures: vec![ByteVec::from(vec![1u8; 65])],
        };
        assert_eq!(SignResponse::decode(&response.encode()).unwrap(), response);
    }
}

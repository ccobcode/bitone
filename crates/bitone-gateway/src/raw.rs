//! 已签名的原始交易。广播前在本地解析并算出哈希：
//! 节点报「已知」时响应里没有哈希，幂等返回要用这里算出的值，格式错误的数据也不用发给上游。

use crate::error::ApiError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::{DisplayHex, FromHex};
use bitone_core::{Chain, Network};
use sha3::{Digest, Keccak256};

/// Solana 一个签名的长度。
const SIGNATURE: usize = 64;

pub struct RawTx {
    pub hash: String,
    pub bytes: Vec<u8>,
}

impl RawTx {
    pub fn parse(network: Network, text: &str) -> Result<Self, ApiError> {
        let text = text.strip_prefix("0x").unwrap_or(text);
        let bytes = Vec::<u8>::from_hex(text).map_err(|_| ApiError::InvalidRawTx)?;
        let hash = match network.chain() {
            Chain::Bitcoin => bitcoin_hash(&bytes),
            Chain::Evm => evm_hash(&bytes),
            Chain::Tron => tron_hash(&bytes),
            Chain::Solana => solana_hash(&bytes),
        }
        .ok_or(ApiError::InvalidRawTx)?;
        Ok(Self { hash, bytes })
    }

    pub fn hex(&self) -> String {
        self.bytes.to_lower_hex_string()
    }

    pub fn base64(&self) -> String {
        STANDARD.encode(&self.bytes)
    }
}

fn bitcoin_hash(bytes: &[u8]) -> Option<String> {
    let tx: bitcoin::Transaction = bitcoin::consensus::encode::deserialize(bytes).ok()?;
    Some(tx.compute_txid().to_string())
}

/// 交易类型字节在 0x01 到 0x7f 之间，旧式交易是 RLP 列表，首字节至少 0xc0。
fn evm_hash(bytes: &[u8]) -> Option<String> {
    match bytes.first()? {
        0x01..=0x7f | 0xc0.. => Some(format!("0x{}", Keccak256::digest(bytes).to_lower_hex_string())),
        _ => None,
    }
}

/// protobuf 的变长整数，返回值和剩余字节。
fn varint(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let mut value = 0usize;
    for (index, byte) in bytes.iter().enumerate().take(5) {
        value |= usize::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, &bytes[index + 1..]));
        }
    }
    None
}

/// TRON 的交易 ID 是 raw_data 的 SHA-256。raw_data 是 Transaction 消息的第 1 个字段。
fn tron_hash(bytes: &[u8]) -> Option<String> {
    let (&tag, rest) = bytes.split_first()?;
    if tag != 0x0a {
        return None;
    }
    let (length, rest) = varint(rest)?;
    let raw_data = rest.get(..length)?;
    Some(sha256::Hash::hash(raw_data).to_byte_array().to_lower_hex_string())
}

/// Solana 的交易哈希是第一个签名。开头是紧凑编码的签名数量，这里只接受 127 个以内。
fn solana_hash(bytes: &[u8]) -> Option<String> {
    let (&count, rest) = bytes.split_first()?;
    if count == 0 || count > 0x7f || rest.len() <= usize::from(count) * SIGNATURE {
        return None;
    }
    Some(bitcoin::base58::encode(&rest[..SIGNATURE]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 创世块的 coinbase。
    const GENESIS: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";

    #[test]
    fn bitcoin_uses_the_display_txid() {
        let raw = RawTx::parse(Network::Bitcoin, GENESIS).unwrap();
        assert_eq!(
            raw.hash,
            "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        );
        assert_eq!(raw.hex(), GENESIS);
        // 尾部多余的字节不是合法交易。
        assert!(RawTx::parse(Network::Bitcoin, &format!("{GENESIS}00")).is_err());
    }

    #[test]
    fn evm_hashes_the_whole_encoding() {
        // EIP-155 文档里的旧式交易示例。
        let raw = "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83";
        let expected = "0x33469b22e9f636356c4160a87eb19df52b7412e8eac32a4a55ffe88ea8350788";
        assert_eq!(RawTx::parse(Network::Ethereum, raw).unwrap().hash, expected);
        assert_eq!(
            RawTx::parse(Network::Sepolia, &format!("0x{raw}")).unwrap().hash,
            expected
        );
        assert!(RawTx::parse(Network::Ethereum, "80").is_err());
        assert!(RawTx::parse(Network::Ethereum, "").is_err());
    }

    #[test]
    fn tron_hashes_raw_data_only() {
        // 主网的一笔真实转账，交易 ID 取自 TronGrid。
        let raw_data = "0a02507e2208db1b9b7d595deec340b095e5e091345a65080112610a2d747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e73666572436f6e747261637412300a154192689ecfbfcdffa3444961571f2b822d3967d0aa12154188403ac26730e33164eeac403291b48f300b782f180270b0f3cfcf9134";
        let length = raw_data.len() / 2;
        let signed = format!(
            "0a{:02x}{:02x}{raw_data}1241{}",
            (length & 0x7f) | 0x80,
            length >> 7,
            "ab".repeat(65)
        );
        let raw = RawTx::parse(Network::Tron, &signed).unwrap();
        assert_eq!(
            raw.hash,
            "22a58240f0260c4df379060e67dde86e14324d15a64738b7b5fb69c8e30dff50"
        );
        for bad in ["", "0a", "0a7f00", "1241ab"] {
            assert!(RawTx::parse(Network::Tron, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn solana_takes_the_first_signature() {
        let mut bytes = vec![1u8];
        bytes.extend([7u8; SIGNATURE]);
        bytes.extend([0x80, 0x01, 0x02]);
        let raw = RawTx::parse(Network::Solana, &bytes.to_lower_hex_string()).unwrap();
        assert_eq!(raw.hash, bitcoin::base58::encode(&[7u8; SIGNATURE]));
        assert_eq!(raw.base64(), STANDARD.encode(&bytes));
        // 没有签名，或者长度不够。
        assert!(RawTx::parse(Network::Solana, "00").is_err());
        assert!(RawTx::parse(Network::Solana, &[1u8; SIGNATURE].to_lower_hex_string()).is_err());
    }

    #[test]
    fn rejects_non_hex() {
        for network in [Network::Bitcoin, Network::Ethereum, Network::Tron, Network::Solana] {
            assert_eq!(RawTx::parse(network, "zz").err(), Some(ApiError::InvalidRawTx));
        }
    }
}

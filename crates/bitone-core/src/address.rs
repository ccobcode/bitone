//! 从公钥算地址.网页端和冷钱包用同一份代码,冷钱包用它核对请求里的地址.

use crate::{AccountKind, Chain, Error, Network};
use bitcoin::address::NetworkUnchecked;
use bitcoin::bip32::{ChildNumber, Xpub};
use bitcoin::hex::DisplayHex;
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, CompressedPublicKey, KnownHrp, NetworkKind};
use sha3::{Digest, Keccak256};

/// 校验收款地址属于这个网络,返回规范写法.
pub fn validate(network: Network, text: &str) -> Result<String, Error> {
    match network.chain() {
        Chain::Bitcoin => {
            let target = match network {
                Network::Bitcoin => bitcoin::Network::Bitcoin,
                Network::BtcSignet => bitcoin::Network::Signet,
                Network::BtcTestnet4 => bitcoin::Network::Testnet4,
                _ => bitcoin::Network::Testnet,
            };
            let address: Address<NetworkUnchecked> = text.parse().map_err(|_| Error::InvalidAddress)?;
            Ok(address
                .require_network(target)
                .map_err(|_| Error::InvalidAddress)?
                .to_string())
        }
        Chain::Evm => parse_evm_address(text).map(|bytes| evm_text(&bytes)),
        Chain::Tron => parse_tron_address(text).map(|bytes| tron_text(&bytes)),
        Chain::Solana => parse_solana_address(text).map(|bytes| bitcoin::base58::encode(&bytes)),
    }
}

fn hrp(network: Network) -> KnownHrp {
    match network {
        Network::Bitcoin => KnownHrp::Mainnet,
        _ => KnownHrp::Testnets,
    }
}

fn network_kind(network: Network) -> NetworkKind {
    match network {
        Network::Bitcoin => NetworkKind::Main,
        _ => NetworkKind::Test,
    }
}

/// 从账户扩展公钥派生 Bitcoin 地址.`change` 为真时是找零地址.
pub fn bitcoin_address(
    xpub: &[u8],
    kind: AccountKind,
    change: bool,
    index: u32,
    network: Network,
) -> Result<String, Error> {
    let secp = Secp256k1::verification_only();
    let account = Xpub::decode(xpub).map_err(|_| Error::InvalidKey)?;
    let path = [
        ChildNumber::from_normal_idx(u32::from(change)).map_err(|_| Error::InvalidKey)?,
        ChildNumber::from_normal_idx(index).map_err(|_| Error::InvalidKey)?,
    ];
    let key = account.derive_pub(&secp, &path).map_err(|_| Error::InvalidKey)?;
    bitcoin_address_from_key(&key.public_key.serialize(), kind, network)
}

pub fn bitcoin_address_from_key(compressed: &[u8], kind: AccountKind, network: Network) -> Result<String, Error> {
    let secp = Secp256k1::verification_only();
    let key = PublicKey::from_slice(compressed).map_err(|_| Error::InvalidKey)?;
    let public = CompressedPublicKey(key);
    let address = match kind {
        AccountKind::SegWit => Address::p2wpkh(&public, hrp(network)),
        AccountKind::Taproot => Address::p2tr(&secp, key.x_only_public_key().0, None, hrp(network)),
        AccountKind::NestedSegWit => Address::p2shwpkh(&public, network_kind(network)),
        AccountKind::Legacy => Address::p2pkh(public, network_kind(network)),
        AccountKind::Single => return Err(Error::InvalidKey),
    };
    Ok(address.to_string())
}

/// 公钥对应的 20 字节地址,EVM 和 TRON 相同.
pub fn eth_address_bytes(compressed: &[u8]) -> Result<[u8; 20], Error> {
    let key = PublicKey::from_slice(compressed).map_err(|_| Error::InvalidKey)?;
    let digest = Keccak256::digest(&key.serialize_uncompressed()[1..]);
    Ok(digest[12..].try_into().expect("20 字节"))
}

/// EVM 地址,带 EIP-55 校验大小写.
pub fn evm_address(compressed: &[u8]) -> Result<String, Error> {
    Ok(evm_text(&eth_address_bytes(compressed)?))
}

pub fn evm_text(address: &[u8; 20]) -> String {
    let hex = address.to_lower_hex_string();
    let digest = Keccak256::digest(hex.as_bytes());
    let mut out = String::from("0x");
    for (i, c) in hex.chars().enumerate() {
        let nibble = (digest[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0xf;
        out.push(if nibble >= 8 { c.to_ascii_uppercase() } else { c });
    }
    out
}

/// 解析 EVM 地址.全大写或全小写不带校验,大小写混合时必须符合 EIP-55.
pub fn parse_evm_address(text: &str) -> Result<[u8; 20], Error> {
    let hex = text.strip_prefix("0x").ok_or(Error::InvalidAddress)?;
    // `from_str_radix` 接受开头的 `+`,所以先逐字符检查.
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidAddress);
    }
    let mut bytes = [0u8; 20];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| Error::InvalidAddress)?;
    }
    let mixed = hex.chars().any(|c| c.is_ascii_lowercase()) && hex.chars().any(|c| c.is_ascii_uppercase());
    if mixed && evm_text(&bytes) != text {
        return Err(Error::InvalidAddress);
    }
    Ok(bytes)
}

pub fn tron_address(compressed: &[u8]) -> Result<String, Error> {
    Ok(tron_text(&eth_address_bytes(compressed)?))
}

pub fn tron_text(address: &[u8; 20]) -> String {
    let mut payload = vec![0x41];
    payload.extend_from_slice(address);
    bitcoin::base58::encode_check(&payload)
}

pub fn parse_tron_address(text: &str) -> Result<[u8; 20], Error> {
    let bytes = bitcoin::base58::decode_check(text).map_err(|_| Error::InvalidAddress)?;
    match bytes.split_first() {
        Some((0x41, rest)) => rest.try_into().map_err(|_| Error::InvalidAddress),
        _ => Err(Error::InvalidAddress),
    }
}

pub fn parse_solana_address(text: &str) -> Result<[u8; 32], Error> {
    let bytes = bitcoin::base58::decode(text).map_err(|_| Error::InvalidAddress)?;
    bytes.as_slice().try_into().map_err(|_| Error::InvalidAddress)
}

pub fn solana_address(public_key: &[u8]) -> Result<String, Error> {
    if public_key.len() != 32 {
        return Err(Error::InvalidKey);
    }
    Ok(bitcoin::base58::encode(public_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_must_belong_to_the_network() {
        let main = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
        assert!(validate(Network::Bitcoin, main).is_ok());
        assert!(validate(Network::BtcTestnet4, main).is_err());
        assert!(validate(Network::BtcTestnet4, "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx").is_ok());
        assert!(validate(Network::Ethereum, "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu").is_err());
        assert_eq!(
            validate(Network::Ethereum, "0x9858effd232b4033e47d90003d41ec34ecaeda94").unwrap(),
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
        );
    }

    #[test]
    fn evm_checksum_is_enforced_on_mixed_case() {
        let good = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
        assert!(parse_evm_address(good).is_ok());
        assert!(parse_evm_address(&good.to_ascii_lowercase()).is_ok());
        assert!(parse_evm_address("0x9858efFD232B4033E47d90003D41EC34EcaEda94").is_err());
        assert!(parse_evm_address("9858EfFD232B4033E47d90003D41EC34EcaEda94").is_err());
        assert!(parse_evm_address("0x9858").is_err());
    }

    #[test]
    fn evm_address_rejects_signs_and_non_hex() {
        let zeros = "00".repeat(19);
        assert!(parse_evm_address(&format!("0x0a{zeros}")).is_ok());
        assert!(parse_evm_address(&format!("0x+a{zeros}")).is_err());
        assert!(parse_evm_address(&format!("0xzz{zeros}")).is_err());
    }

    #[test]
    fn tron_round_trip_and_checksum() {
        let text = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";
        assert_eq!(tron_text(&parse_tron_address(text).unwrap()), text);
        assert!(parse_tron_address("TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdX").is_err());
    }

    #[test]
    fn solana_address_must_be_32_bytes() {
        assert!(parse_solana_address("HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk").is_ok());
        assert!(parse_solana_address("abc").is_err());
    }
}

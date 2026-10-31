//! 按链校验地址。地址校验通过后才会拼进上游 URL，这也是防注入的边界。

use crate::error::ApiError;
use bitcoin::address::NetworkUnchecked;
use bitcoin::base58;
use bitone_core::{Chain, Network};
use std::collections::HashSet;

/// 最长的合法地址是 bech32 的 90 个字符。base58 解码是平方复杂度，先挡住超长输入。
const MAX_LENGTH: usize = 90;
/// Solana 签名的 base58 文本不超过 88 个字符。
const MAX_SIGNATURE_LENGTH: usize = 88;

pub fn is_valid(network: Network, address: &str) -> bool {
    if address.len() > MAX_LENGTH {
        return false;
    }
    match network.chain() {
        Chain::Bitcoin => {
            // Testnet4、Signet、Testnet3 的地址格式相同，都按测试网校验。
            let expected = match network {
                Network::Bitcoin => bitcoin::Network::Bitcoin,
                _ => bitcoin::Network::Testnet,
            };
            address
                .parse::<bitcoin::Address<NetworkUnchecked>>()
                .is_ok_and(|parsed| parsed.is_valid_for_network(expected))
        }
        // ponytail: 不校验 EIP-55 大小写，只读查询写错也花不了钱。转账校验在网页端和冷钱包。
        Chain::Evm => address
            .strip_prefix("0x")
            .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())),
        Chain::Tron => base58::decode_check(address).is_ok_and(|p| p.len() == 21 && p[0] == 0x41),
        Chain::Solana => base58::decode(address).is_ok_and(|key| key.len() == 32),
    }
}

/// 校验数量和格式，再按原顺序去重。
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

/// 交易哈希的格式。Esplora 的哈希会拼进上游 URL，这里同样是防注入的边界。
pub fn is_valid_hash(network: Network, hash: &str) -> bool {
    let hex64 = |text: &str| text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit());
    match network.chain() {
        Chain::Bitcoin | Chain::Tron => hex64(hash),
        Chain::Evm => hash.strip_prefix("0x").is_some_and(hex64),
        Chain::Solana => {
            hash.len() <= MAX_SIGNATURE_LENGTH && base58::decode(hash).is_ok_and(|signature| signature.len() == 64)
        }
    }
}

/// 校验数量和格式，再按原顺序去重。
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

#[cfg(test)]
mod tests {
    use super::*;

    const SEGWIT: &str = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
    const TAPROOT: &str = "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr";
    const LEGACY: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";
    const TESTNET: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
    const EVM: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
    const TRON: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const SOLANA: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    #[test]
    fn bitcoin_networks_do_not_mix() {
        for address in [SEGWIT, TAPROOT, LEGACY] {
            assert!(is_valid(Network::Bitcoin, address));
            assert!(!is_valid(Network::BtcTestnet4, address));
        }
        assert!(!is_valid(Network::Bitcoin, TESTNET));
        for network in [Network::BtcTestnet4, Network::BtcSignet, Network::BtcTestnet3] {
            assert!(is_valid(network, TESTNET));
        }
        assert!(!is_valid(
            Network::Bitcoin,
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
        ));
    }

    #[test]
    fn evm_rules() {
        assert!(is_valid(Network::Ethereum, EVM));
        assert!(is_valid(Network::Sepolia, &EVM.to_lowercase()));
        assert!(!is_valid(Network::Ethereum, &EVM[2..]));
        assert!(!is_valid(Network::Ethereum, &EVM[..41]));
        assert!(!is_valid(
            Network::Ethereum,
            "0xZ858EfFD232B4033E47d90003D41EC34EcaEda94"
        ));
    }

    #[test]
    fn tron_rules() {
        assert!(is_valid(Network::Tron, TRON));
        assert!(is_valid(Network::TronNile, TRON));
        // 校验和错误。
        assert!(!is_valid(Network::Tron, "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6u"));
        // Bitcoin 的 base58check 地址，前缀不是 0x41。
        assert!(!is_valid(Network::Tron, LEGACY));
    }

    #[test]
    fn solana_rules() {
        assert!(is_valid(Network::Solana, SOLANA));
        assert!(is_valid(Network::Solana, "11111111111111111111111111111111"));
        assert!(!is_valid(Network::Solana, "abc"));
        assert!(!is_valid(Network::Solana, "0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl"));
    }

    #[test]
    fn chains_do_not_mix() {
        assert!(!is_valid(Network::Ethereum, TRON));
        assert!(!is_valid(Network::Tron, EVM));
        assert!(!is_valid(Network::Solana, EVM));
        assert!(!is_valid(Network::Bitcoin, EVM));
    }

    #[test]
    fn overlong_input_is_rejected_before_decoding() {
        assert!(!is_valid(Network::Solana, &"1".repeat(60_000)));
    }

    #[test]
    fn hashes_follow_the_chain_format() {
        let hex = "ab".repeat(32);
        let signature = base58::encode(&[7u8; 64]);
        assert!(is_valid_hash(Network::Bitcoin, &hex));
        assert!(is_valid_hash(Network::Tron, &hex));
        assert!(is_valid_hash(Network::Ethereum, &format!("0x{hex}")));
        assert!(is_valid_hash(Network::Solana, &signature));

        assert!(!is_valid_hash(Network::Ethereum, &hex));
        assert!(!is_valid_hash(Network::Bitcoin, &format!("0x{hex}")));
        assert!(!is_valid_hash(Network::Bitcoin, "../../address/x"));
        assert!(!is_valid_hash(Network::Bitcoin, &"zz".repeat(32)));
        assert!(!is_valid_hash(Network::Solana, &hex));
        assert!(!is_valid_hash(Network::Solana, &"1".repeat(60_000)));
    }

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
}

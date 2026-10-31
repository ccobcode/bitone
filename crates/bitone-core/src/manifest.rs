//! 内置清单。图标、符号、小数位都只按「网络加合约地址」查，没有按符号查找的入口。
//! 见 docs/04-security.md 第 2.8、2.9 节。

use crate::Error;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 厂商公钥。这是开发用的密钥，发布前必须换成离线保管的正式密钥。
pub const VENDOR_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a, 0x0e, 0xe1, 0x72,
    0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub network: String,
    pub contract: String,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub icon: String,
    pub standard: String,
    pub min_fraction: u8,
}

impl Asset {
    /// 图标用的键，`网络:合约`。
    pub fn key(&self) -> String {
        format!("{}:{}", self.network, self.contract)
    }
}

/// 已知的函数。只影响展示，不能降低风险类别。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownFunction {
    pub network: String,
    pub contract: String,
    pub selector: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub functions: Vec<KnownFunction>,
}

static BUILTIN: OnceLock<Manifest> = OnceLock::new();

impl Manifest {
    pub fn builtin() -> &'static Manifest {
        BUILTIN.get_or_init(|| serde_json::from_str(include_str!("../assets/manifest.json")).expect("内置清单有效"))
    }

    /// EVM 地址不区分大小写，Base58 地址区分。
    pub fn lookup(&self, network: &str, contract: &str) -> Option<&Asset> {
        let hex = contract.starts_with("0x");
        self.assets.iter().find(|a| {
            a.network == network
                && if hex {
                    a.contract.eq_ignore_ascii_case(contract)
                } else {
                    a.contract == contract
                }
        })
    }

    pub fn native(&self, network: &str) -> Option<&Asset> {
        self.lookup(network, "native")
    }

    pub fn function(&self, network: &str, contract: &str, selector: &str) -> Option<&KnownFunction> {
        self.functions.iter().find(|f| {
            f.network == network
                && f.contract.eq_ignore_ascii_case(contract)
                && f.selector.eq_ignore_ascii_case(selector)
        })
    }

    /// 校验清单包。版本只增不减，签名不对或版本回退都拒绝。
    pub fn from_package(package: &[u8], key: &[u8; 32], current: u32) -> Result<Self, Error> {
        if package.len() <= 64 {
            return Err(Error::Decode);
        }
        let (signature, body) = package.split_at(64);
        let signature = Signature::from_slice(signature).map_err(|_| Error::Decode)?;
        let key = VerifyingKey::from_bytes(key).map_err(|_| Error::InvalidKey)?;
        key.verify(body, &signature).map_err(|_| Error::BadSignature)?;
        let manifest: Self = serde_json::from_slice(body).map_err(|_| Error::Decode)?;
        if manifest.version <= current {
            return Err(Error::StaleManifest);
        }
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_needs_the_contract_address() {
        let manifest = Manifest::builtin();
        let usdt = manifest
            .lookup("ethereum", "0xdac17f958d2ee523a2206206994597c13d831ec7")
            .unwrap();
        assert_eq!((usdt.symbol.as_str(), usdt.decimals), ("USDT", 6));
        assert!(manifest.lookup("ethereum", "USDT").is_none());
        assert!(
            manifest
                .lookup("polygon", "0xdac17f958d2ee523a2206206994597c13d831ec7")
                .is_none()
        );
    }

    #[test]
    fn base58_addresses_are_case_sensitive() {
        let manifest = Manifest::builtin();
        assert!(manifest.lookup("tron", "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").is_some());
        assert!(manifest.lookup("tron", "tr7nhqjekqxgtci8q8zy4pl8otszgjlj6t").is_none());
    }

    #[test]
    fn same_symbol_can_have_different_decimals() {
        let manifest = Manifest::builtin();
        let bsc = manifest
            .lookup("bsc", "0x55d398326f99059fF775485246999027B3197955")
            .unwrap();
        assert_eq!(bsc.decimals, 18);
    }

    #[test]
    fn every_network_has_a_native_asset() {
        for network in crate::Network::ALL {
            assert!(Manifest::builtin().native(network.id()).is_some(), "{}", network.id());
        }
    }
}

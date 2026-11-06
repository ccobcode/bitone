//! 内置清单.图标,符号,小数位都只按"网络加合约地址"查,没有按符号查找的入口.

use crate::{Error, Network};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{DerSignature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::OnceLock;

/// 厂商公钥,P-256 的 SPKI DER.和 APK 签名证书是同一把密钥,换公钥就是替换这个文件.
pub const VENDOR_KEY: [u8; 91] = *include_bytes!("../assets/manifest-key.der");

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
    /// Token-2022 的 mint 声明的扩展.扩展是链上状态,声明过才可能是普通转账.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
}

impl Asset {
    /// 图标用的键,`网络:合约`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.network, self.contract)
    }

    /// 清单没有这个网络的原生币时用的数据,只来自链本身,没有图标.
    pub fn plain_native(network: Network) -> Self {
        let (symbol, decimals) = network.native_coin();
        Self {
            network: network.id().to_string(),
            contract: "native".to_string(),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals,
            icon: String::new(),
            standard: String::new(),
            min_fraction: decimals,
            extensions: None,
        }
    }
}

/// 一个字段的展示方式,格式和 ERC-7730 的 `display.formats` 相同.`path` 是顶层参数名.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayField {
    pub path: String,
    pub label: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub params: DisplayParams,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayParams {
    #[serde(default)]
    pub token_path: Option<String>,
    #[serde(default)]
    pub threshold: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayFormat {
    pub intent: String,
    #[serde(default)]
    pub fields: Vec<DisplayField>,
}

/// 描述文件.只决定字段名和展示,类别由策略判定.只通过验签的清单到达.
/// 按网络,合约绑定.选择器由 `signature` 算出,`signature` 是带参数名的函数,比如 `stake(uint256 amount)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub network: String,
    pub contract: String,
    pub signature: String,
    #[serde(flatten)]
    pub format: DisplayFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub descriptors: Vec<Descriptor>,
}

static BUILTIN: OnceLock<Manifest> = OnceLock::new();

impl Manifest {
    pub fn builtin() -> &'static Manifest {
        BUILTIN.get_or_init(|| serde_json::from_str(include_str!("../assets/manifest.json")).expect("内置清单有效"))
    }

    /// EVM 地址不区分大小写,Base58 地址区分.
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

    /// 绑定到这个合约的描述文件.
    pub fn descriptors<'a>(&'a self, network: &'a str, contract: &'a str) -> impl Iterator<Item = &'a Descriptor> {
        let hex = contract.starts_with("0x");
        self.descriptors.iter().filter(move |d| {
            d.network == network
                && if hex {
                    d.contract.eq_ignore_ascii_case(contract)
                } else {
                    d.contract == contract
                }
        })
    }

    /// 校验清单包.版本只增不减,签名不对或版本回退都拒绝.通过后整体覆盖内置清单.
    /// 包的格式是 DER 编码的 ECDSA 签名加 JSON 原文,签名的长度写在它的第二个字节.
    pub fn from_package(package: &[u8], key: &[u8], current: u32) -> Result<Self, Error> {
        let length = usize::from(*package.get(1).ok_or(Error::Decode)?) + 2;
        let (signature, body) = package.split_at_checked(length).ok_or(Error::Decode)?;
        let signature = DerSignature::try_from(signature).map_err(|_| Error::Decode)?;
        let key = VerifyingKey::from_public_key_der(key).map_err(|_| Error::InvalidKey)?;
        key.verify(body, &signature).map_err(|_| Error::BadSignature)?;
        let manifest: Self = serde_json::from_slice(body).map_err(|_| Error::Decode)?;
        if manifest.version <= current {
            return Err(Error::StaleManifest);
        }
        Ok(manifest)
    }

    /// 生效的清单:内置清单和已导入的清单包里版本高的一份.升级到内置版本更高的 APK 后,旧包不再生效.
    /// 包每次都重新验签,本机文件也不信任.包缺失或损坏时用内置清单,清单不影响能不能签名.
    pub fn active(package: Option<&[u8]>, key: &[u8]) -> Cow<'static, Self> {
        let builtin = Self::builtin();
        package
            .and_then(|package| Self::from_package(package, key, builtin.version).ok())
            .map_or(Cow::Borrowed(builtin), Cow::Owned)
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

    #[test]
    fn plain_chain_data_matches_the_builtin_rows() {
        for network in crate::Network::ALL {
            let builtin = Manifest::builtin().native(network.id()).unwrap();
            let plain = Asset::plain_native(network);
            assert_eq!(plain.key(), builtin.key());
            assert_eq!((&plain.symbol, plain.decimals), (&builtin.symbol, builtin.decimals));
        }
    }

    #[test]
    fn a_signed_manifest_without_descriptors_still_parses() {
        let manifest: Manifest = serde_json::from_str("{\"version\":9,\"assets\":[]}").unwrap();
        assert!(manifest.descriptors.is_empty());
    }

    #[test]
    fn a_descriptor_keeps_the_erc7730_shape() {
        let text = "{\"version\":9,\"assets\":[],\"descriptors\":[{\"network\":\"ethereum\",\"contract\":\"0xAB\",
            \"signature\":\"stake(uint256 amount)\",\"intent\":\"Stake\",
            \"fields\":[{\"path\":\"amount\",\"label\":\"Amount\",\"format\":\"tokenAmount\",
            \"params\":{\"tokenPath\":\"@.to\"}}]}]}";
        let manifest: Manifest = serde_json::from_str(text).unwrap();
        let found: Vec<_> = manifest.descriptors("ethereum", "0xab").collect();
        assert_eq!(found[0].format.fields[0].params.token_path.as_deref(), Some("@.to"));
        assert_eq!(manifest.descriptors("polygon", "0xab").count(), 0);
    }

    #[test]
    fn a_missing_or_corrupt_package_is_the_builtin_list() {
        let builtin = Manifest::builtin();
        assert_eq!(*Manifest::active(None, &VENDOR_KEY), *builtin);
        assert_eq!(*Manifest::active(Some(&[]), &VENDOR_KEY), *builtin);
        assert_eq!(*Manifest::active(Some(&[0x30, 0x02, 0x01]), &VENDOR_KEY), *builtin);
    }
}

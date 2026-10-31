//! 派生路径和白名单.
//!
//! 路径决定链和网络,这是测试网签名不能花主网资金的依据.

use crate::{Chain, Error, Mode, Network};
use minicbor::{Decode, Encode};
use std::fmt;

const HARDENED: u32 = 0x8000_0000;

/// Bitcoin 地址索引的硬上限,防止资金被放到用户找不到的位置.
pub const MAX_ADDRESS_INDEX: u32 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationPath(pub Vec<u32>);

impl DerivationPath {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut parts = text.split('/');
        if parts.next() != Some("m") {
            return Err(Error::PathFormat);
        }
        let levels = parts
            .map(|part| {
                let (digits, hardened) = match part.strip_suffix('\'') {
                    Some(digits) => (digits, true),
                    None => (part, false),
                };
                let value: u32 = digits.parse().map_err(|_| Error::PathFormat)?;
                if value >= HARDENED {
                    return Err(Error::PathFormat);
                }
                Ok(if hardened { value | HARDENED } else { value })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(levels))
    }
}

impl fmt::Display for DerivationPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "m")?;
        for level in &self.0 {
            if level & HARDENED != 0 {
                write!(f, "/{}'", level & !HARDENED)?;
            } else {
                write!(f, "/{level}")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Encode, Decode)]
#[cbor(index_only)]
pub enum AccountKind {
    #[n(0)]
    SegWit,
    #[n(1)]
    Taproot,
    /// 只用于转出导入钱包的旧资产.
    #[n(2)]
    NestedSegWit,
    /// 只用于转出导入钱包的旧资产.
    #[n(3)]
    Legacy,
    /// EVM,TRON,Solana 的单地址账户.
    #[n(4)]
    Single,
}

impl AccountKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::SegWit => "segwit",
            Self::Taproot => "taproot",
            Self::NestedSegWit => "nested-segwit",
            Self::Legacy => "legacy",
            Self::Single => "single",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::SegWit,
            Self::Taproot,
            Self::NestedSegWit,
            Self::Legacy,
            Self::Single,
        ]
        .into_iter()
        .find(|kind| kind.id() == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathInfo {
    pub chain: Chain,
    /// EVM 的主网和测试网路径相同,靠签名内的 Chain ID 隔离,所以是 `None`.
    pub mode: Option<Mode>,
    pub kind: AccountKind,
}

fn h(value: u32) -> u32 {
    value | HARDENED
}

fn bitcoin_mode(coin: u32) -> Option<Mode> {
    match coin {
        c if c == h(0) => Some(Mode::Mainnet),
        c if c == h(1) => Some(Mode::Testnet),
        _ => None,
    }
}

/// 判断路径是否在白名单里.不在白名单的路径没有 `Ok` 结果.
pub fn classify(path: &DerivationPath) -> Result<PathInfo, Error> {
    let info = |chain, mode, kind| Ok(PathInfo { chain, mode, kind });
    match path.0.as_slice() {
        [purpose, coin, account, change, index]
            if *purpose == h(84) || *purpose == h(86) || *purpose == h(44) || *purpose == h(49) =>
        {
            if *purpose == h(44) && *coin == h(60) {
                return if path.0[2..] == [h(0), 0, 0] {
                    info(Chain::Evm, None, AccountKind::Single)
                } else {
                    Err(Error::PathNotAllowed)
                };
            }
            if *purpose == h(44) && *coin == h(195) {
                return if path.0[2..] == [h(0), 0, 0] {
                    info(Chain::Tron, Some(Mode::Mainnet), AccountKind::Single)
                } else {
                    Err(Error::PathNotAllowed)
                };
            }
            if *purpose == h(44) && *coin == h(1) {
                return if path.0[2..] == [h(0), 0, 0] {
                    info(Chain::Tron, Some(Mode::Testnet), AccountKind::Single)
                } else {
                    Err(Error::PathNotAllowed)
                };
            }
            let mode = bitcoin_mode(*coin).ok_or(Error::PathNotAllowed)?;
            if *account != h(0) || *change > 1 {
                return Err(Error::PathNotAllowed);
            }
            if *index >= MAX_ADDRESS_INDEX {
                return Err(Error::IndexTooLarge);
            }
            let kind = match *purpose {
                p if p == h(84) => AccountKind::SegWit,
                p if p == h(86) => AccountKind::Taproot,
                p if p == h(49) => AccountKind::NestedSegWit,
                _ => AccountKind::Legacy,
            };
            if matches!(kind, AccountKind::NestedSegWit | AccountKind::Legacy) && mode != Mode::Mainnet {
                return Err(Error::PathNotAllowed);
            }
            info(Chain::Bitcoin, Some(mode), kind)
        }
        [purpose, coin, account, change] if *purpose == h(44) && *account == h(0) && *change == h(0) => match *coin {
            c if c == h(501) => info(Chain::Solana, Some(Mode::Mainnet), AccountKind::Single),
            c if c == h(1) => info(Chain::Solana, Some(Mode::Testnet), AccountKind::Single),
            _ => Err(Error::PathNotAllowed),
        },
        _ => Err(Error::PathNotAllowed),
    }
}

/// 校验路径可以用在这个网络上.主网路径配测试网声明,Bitcoin 路径配 EVM 请求都会被拒绝.
pub fn validate_for_network(path: &DerivationPath, network: Network) -> Result<PathInfo, Error> {
    let info = classify(path)?;
    if info.chain != network.chain() {
        return Err(Error::PathNetworkMismatch);
    }
    if let Some(mode) = info.mode
        && mode != network.mode()
    {
        return Err(Error::PathNetworkMismatch);
    }
    Ok(info)
}

/// 一个账户要导出的路径和公钥类型.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSpec {
    pub chain: Chain,
    pub kind: AccountKind,
    /// Bitcoin 是账户级路径,导出扩展公钥.其余是完整路径,导出账户公钥.
    pub path: &'static str,
}

/// 冷钱包在这个模式下导出的账户.`imported` 为真时额外导出 BIP44 和 BIP49,用于找回旧资产.
pub fn account_specs(mode: Mode, imported: bool) -> Vec<AccountSpec> {
    let spec = |chain, kind, path| AccountSpec { chain, kind, path };
    let mut specs = match mode {
        Mode::Mainnet => vec![
            spec(Chain::Bitcoin, AccountKind::SegWit, "m/84'/0'/0'"),
            spec(Chain::Bitcoin, AccountKind::Taproot, "m/86'/0'/0'"),
            spec(Chain::Evm, AccountKind::Single, "m/44'/60'/0'/0/0"),
            spec(Chain::Tron, AccountKind::Single, "m/44'/195'/0'/0/0"),
            spec(Chain::Solana, AccountKind::Single, "m/44'/501'/0'/0'"),
        ],
        Mode::Testnet => vec![
            spec(Chain::Bitcoin, AccountKind::SegWit, "m/84'/1'/0'"),
            spec(Chain::Bitcoin, AccountKind::Taproot, "m/86'/1'/0'"),
            spec(Chain::Evm, AccountKind::Single, "m/44'/60'/0'/0/0"),
            spec(Chain::Tron, AccountKind::Single, "m/44'/1'/0'/0/0"),
            spec(Chain::Solana, AccountKind::Single, "m/44'/1'/0'/0'"),
        ],
    };
    if imported && mode == Mode::Mainnet {
        specs.push(spec(Chain::Bitcoin, AccountKind::NestedSegWit, "m/49'/0'/0'"));
        specs.push(spec(Chain::Bitcoin, AccountKind::Legacy, "m/44'/0'/0'"));
    }
    specs
}

/// 冷钱包导出的全部账户.主网和测试网私钥不同的链各有一份,EVM 共用一份.
pub fn all_account_specs(imported: bool) -> Vec<AccountSpec> {
    let mut specs = account_specs(Mode::Mainnet, imported);
    for spec in account_specs(Mode::Testnet, imported) {
        if !specs.iter().any(|s| s.path == spec.path) {
            specs.push(spec);
        }
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> DerivationPath {
        DerivationPath::parse(text).unwrap()
    }

    #[test]
    fn parse_and_display_round_trip() {
        for text in ["m/84'/0'/0'/1/7", "m/44'/501'/0'/0'", "m"] {
            assert_eq!(p(text).to_string(), text);
        }
        for bad in ["84'/0'", "m/x", "m/2147483648", "m//1", ""] {
            assert_eq!(DerivationPath::parse(bad), Err(Error::PathFormat));
        }
    }

    #[test]
    fn whitelist_accepts_every_documented_path() {
        let cases = [
            ("m/84'/0'/0'/0/5", Network::Bitcoin),
            ("m/86'/0'/0'/1/0", Network::Bitcoin),
            ("m/84'/1'/0'/0/0", Network::BtcTestnet4),
            ("m/86'/1'/0'/0/0", Network::BtcSignet),
            ("m/49'/0'/0'/0/0", Network::Bitcoin),
            ("m/44'/0'/0'/0/0", Network::Bitcoin),
            ("m/44'/60'/0'/0/0", Network::Ethereum),
            ("m/44'/60'/0'/0/0", Network::Sepolia),
            ("m/44'/60'/0'/0/0", Network::Amoy),
            ("m/44'/195'/0'/0/0", Network::Tron),
            ("m/44'/1'/0'/0/0", Network::TronNile),
            ("m/44'/501'/0'/0'", Network::Solana),
            ("m/44'/1'/0'/0'", Network::SolanaDevnet),
        ];
        for (path, network) in cases {
            assert!(validate_for_network(&p(path), network).is_ok(), "{path} on {network:?}");
        }
    }

    #[test]
    fn mainnet_path_cannot_be_labelled_testnet() {
        let cases = [
            ("m/44'/195'/0'/0/0", Network::TronNile),
            ("m/44'/501'/0'/0'", Network::SolanaDevnet),
            ("m/84'/0'/0'/0/0", Network::BtcTestnet4),
            ("m/44'/1'/0'/0/0", Network::Tron),
            ("m/44'/1'/0'/0'", Network::Solana),
        ];
        for (path, network) in cases {
            assert_eq!(
                validate_for_network(&p(path), network),
                Err(Error::PathNetworkMismatch),
                "{path} on {network:?}"
            );
        }
    }

    #[test]
    fn path_of_one_chain_cannot_sign_for_another() {
        assert_eq!(
            validate_for_network(&p("m/84'/0'/0'/0/0"), Network::Ethereum),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(
            validate_for_network(&p("m/44'/60'/0'/0/0"), Network::Tron),
            Err(Error::PathNetworkMismatch)
        );
    }

    #[test]
    fn rejects_paths_outside_the_whitelist() {
        for path in [
            "m/84'/0'/1'/0/0",
            "m/84'/0'/0'/2/0",
            "m/44'/60'/0'/0/1",
            "m/44'/60'/1'/0/0",
            "m/44'/195'/0'/0/1",
            "m/44'/501'/1'/0'",
            "m/44'/1'/0'/1/0",
            "m/44'/2'/0'/0/0",
            "m/49'/1'/0'/0/0",
            "m/44'/1'/0'/0/1",
            "m/0",
            "m",
        ] {
            assert!(classify(&p(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn rejects_huge_change_index() {
        assert_eq!(classify(&p("m/84'/0'/0'/1/999999999")), Err(Error::IndexTooLarge));
        assert!(classify(&p("m/84'/0'/0'/1/99999")).is_ok());
    }

    #[test]
    fn imported_wallets_export_old_paths_on_mainnet_only() {
        assert_eq!(account_specs(Mode::Mainnet, false).len(), 5);
        assert_eq!(account_specs(Mode::Mainnet, true).len(), 7);
        assert_eq!(account_specs(Mode::Testnet, true).len(), 5);
    }

    #[test]
    fn every_exported_path_passes_the_whitelist() {
        for mode in [Mode::Mainnet, Mode::Testnet] {
            for spec in account_specs(mode, true) {
                let full = if spec.chain == Chain::Bitcoin {
                    format!("{}/0/0", spec.path)
                } else {
                    spec.path.to_string()
                };
                let info = classify(&p(&full)).unwrap();
                assert_eq!(info.chain, spec.chain);
                assert_eq!(info.kind, spec.kind);
            }
        }
    }
}

//! 派生路径决定链和私钥类型.路径不设白名单,不是本机导出的账户时页面显示路径并告警.

use crate::{Chain, Error, Mode, Network};
use minicbor::{Decode, Encode};
use std::fmt;

const HARDENED: u32 = 0x8000_0000;

/// 派生路径的层数上限,BIP-32 的深度只有一个字节.
pub const MAX_PATH_DEPTH: usize = 255;

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

/// 认出路径属于哪条链,用哪种地址.认不出来时没有 `Ok` 结果,Bitcoin 的账户,找零链和索引,
/// EVM,TRON 主网 coin type 下的路径都不限制.
pub fn classify(path: &DerivationPath) -> Result<PathInfo, Error> {
    let info = |chain, mode, kind| Ok(PathInfo { chain, mode, kind });
    match path.0.as_slice() {
        [purpose, coin, ..] if *purpose == h(44) && *coin == h(60) => info(Chain::Evm, None, AccountKind::Single),
        [purpose, coin, ..] if *purpose == h(44) && *coin == h(195) => {
            info(Chain::Tron, Some(Mode::Mainnet), AccountKind::Single)
        }
        [purpose, coin, _, _, _]
            if *purpose == h(84) || *purpose == h(86) || *purpose == h(44) || *purpose == h(49) =>
        {
            if *purpose == h(44) && *coin == h(1) {
                return if path.0[2..] == [h(0), 0, 0] {
                    info(Chain::Tron, Some(Mode::Testnet), AccountKind::Single)
                } else {
                    Err(Error::PathNotAllowed)
                };
            }
            let mode = bitcoin_mode(*coin).ok_or(Error::PathNotAllowed)?;
            let kind = match *purpose {
                p if p == h(84) => AccountKind::SegWit,
                p if p == h(86) => AccountKind::Taproot,
                p if p == h(49) => AccountKind::NestedSegWit,
                _ => AccountKind::Legacy,
            };
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

/// Bitcoin 路径里的 coin type 能说明的私钥类型.`0'` 是主网,`1'` 是测试币,其他值说明不了.
pub fn bitcoin_path_mode(path: &DerivationPath) -> Option<Mode> {
    bitcoin_mode(*path.0.get(1)?)
}

/// Bitcoin 请求里的路径有没有矛盾.只有路径明确属于另一种私钥或另一条链才算,
/// 认不出来的路径不是矛盾.本机不认识的网络不做这项检查.
pub fn check_bitcoin_path(path: &DerivationPath, network: Option<Network>) -> Result<(), Error> {
    let Some(network) = network else {
        return Ok(());
    };
    match (bitcoin_path_mode(path), classify(path)) {
        (Some(mode), _) if mode != network.mode() => Err(Error::PathNetworkMismatch),
        (None, Ok(info)) if info.chain != Chain::Bitcoin => Err(Error::PathNetworkMismatch),
        _ => Ok(()),
    }
}

/// EVM,TRON 请求里的路径有没有矛盾.路径不设白名单,只有用途和 coin type 都属于本钱包其他链的主网账户才算.
/// 这些私钥上有真实资产,裸哈希和未知载荷都能签下它们的交易.coin type `1'` 不只属于一条链,说明不了什么.
pub fn check_foreign_path(path: &DerivationPath, chain: Chain) -> Result<(), Error> {
    let foreign = account_specs(Mode::Mainnet, true)
        .iter()
        .filter(|spec| spec.chain != chain)
        .filter_map(|spec| DerivationPath::parse(spec.path).ok())
        .any(|account| path.0.get(..2) == account.0.get(..2));
    if foreign {
        return Err(Error::PathNetworkMismatch);
    }
    Ok(())
}

/// 每一级都是硬化的.SLIP-0010 的 ed25519 没有非硬化派生.
pub fn is_all_hardened(path: &DerivationPath) -> bool {
    path.0.iter().all(|level| level & HARDENED != 0)
}

/// Solana 路径里的 coin type 能说明的私钥类型.`501'` 是主网,`1'` 是测试币,其他值说明不了.
pub fn solana_path_mode(path: &DerivationPath) -> Option<Mode> {
    match path.0.as_slice() {
        [purpose, coin, ..] if *purpose == h(44) && *coin == h(501) => Some(Mode::Mainnet),
        [purpose, coin, ..] if *purpose == h(44) && *coin == h(1) => Some(Mode::Testnet),
        _ => None,
    }
}

/// Solana 请求里的路径有没有矛盾.全部硬化的路径都可以签,只有非硬化的路径算不出私钥,
/// 以及路径的 coin type 和本机认识的网络属于不同的私钥才算.
pub fn check_solana_path(path: &DerivationPath, network: Option<Network>) -> Result<(), Error> {
    if !is_all_hardened(path) {
        let foreign = classify(path).is_ok_and(|info| info.chain != Chain::Solana);
        return Err(if foreign {
            Error::PathNetworkMismatch
        } else {
            Error::PathNotAllowed
        });
    }
    match (solana_path_mode(path), network) {
        (Some(mode), Some(network)) if mode != network.mode() => Err(Error::PathNetworkMismatch),
        _ => Ok(()),
    }
}

/// TRON 请求里的路径有没有矛盾.`195'` 是主网私钥,`1'` 是测试币私钥,本机认识的网络要和它一致.
pub fn check_tron_path(path: &DerivationPath, network: Option<Network>) -> Result<(), Error> {
    check_foreign_path(path, Chain::Tron)?;
    let coin = path.0.get(1).copied();
    let mode = match coin {
        Some(coin) if coin == h(195) => Some(Mode::Mainnet),
        Some(coin) if coin == h(1) => Some(Mode::Testnet),
        _ => None,
    };
    match (mode, network) {
        (Some(mode), Some(network)) if mode != network.mode() => Err(Error::PathNetworkMismatch),
        _ => Ok(()),
    }
}

fn is_exported(path: &DerivationPath, chain: Chain) -> bool {
    let text = path.to_string();
    all_account_specs(true)
        .iter()
        .any(|spec| spec.chain == chain && spec.path == text)
}

/// 路径是不是本机导出的 EVM 账户.其余路径可以签名,但要告警.
pub fn is_usual_evm_path(path: &DerivationPath) -> bool {
    is_exported(path, Chain::Evm)
}

/// 路径是不是本机导出的 TRON 账户.其余路径可以签名,但要告警.
pub fn is_usual_tron_path(path: &DerivationPath) -> bool {
    is_exported(path, Chain::Tron)
}

/// 路径是不是本机导出的 Solana 账户.其余路径可以签名,但要告警.
pub fn is_usual_solana_path(path: &DerivationPath) -> bool {
    is_exported(path, Chain::Solana)
}

/// 路径是否落在本机导出的 Bitcoin 账户下,地址类型也和账户一致.其余路径可以花费,但要告警.
pub fn is_usual_bitcoin_path(path: &DerivationPath, kind: AccountKind) -> bool {
    let [purpose, coin, account, change, index] = path.0.as_slice() else {
        return false;
    };
    let parent = [*purpose, *coin, *account];
    *change <= 1
        && *index < HARDENED
        && all_account_specs(true).iter().any(|spec| {
            spec.chain == Chain::Bitcoin
                && spec.kind == kind
                && DerivationPath::parse(spec.path).is_ok_and(|account| account.0 == parent)
        })
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
            "m/44'/501'/1'/0'",
            "m/44'/1'/0'/1/0",
            "m/44'/2'/0'/0/0",
            "m/44'/1'/0'/0/1",
            "m/0",
            "m",
        ] {
            assert!(classify(&p(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn every_path_under_coin_type_60_is_evm_with_no_mode() {
        for path in [
            "m/44'/60'/0'/0/0",
            "m/44'/60'/0'/0/1",
            "m/44'/60'/7'/0/0",
            "m/44'/60'/3'",
            "m/44'/60'",
        ] {
            let info = classify(&p(path)).unwrap();
            assert_eq!((info.chain, info.mode), (Chain::Evm, None), "{path}");
            assert!(validate_for_network(&p(path), Network::Sepolia).is_ok(), "{path}");
        }
    }

    #[test]
    fn any_evm_path_is_free_except_the_mainnet_keys_of_other_chains() {
        for free in [
            "m/44'/60'/9'/0/1",
            "m/44'/966'/0'/0/0",
            "m/44'/1'/0'/0/0",
            "m/84'/1'/0'/0/0",
            "m/0",
            "m",
        ] {
            assert_eq!(check_foreign_path(&p(free), Chain::Evm), Ok(()), "{free}");
        }
        for taken in [
            "m/84'/0'/0'/0/0",
            "m/86'/0'/5'/1/2",
            "m/44'/195'/0'/0/0",
            "m/44'/501'/0'/0'",
            "m/44'/0'/0'/0/0",
        ] {
            assert_eq!(
                check_foreign_path(&p(taken), Chain::Evm),
                Err(Error::PathNetworkMismatch),
                "{taken}"
            );
        }
    }

    #[test]
    fn every_path_under_coin_type_195_is_tron_on_mainnet() {
        for path in ["m/44'/195'/0'/0/0", "m/44'/195'/0'/0/1", "m/44'/195'/3'", "m/44'/195'"] {
            let info = classify(&p(path)).unwrap();
            assert_eq!((info.chain, info.mode), (Chain::Tron, Some(Mode::Mainnet)), "{path}");
            assert!(validate_for_network(&p(path), Network::Tron).is_ok(), "{path}");
            assert_eq!(
                validate_for_network(&p(path), Network::TronNile),
                Err(Error::PathNetworkMismatch),
                "{path}"
            );
        }
    }

    #[test]
    fn any_tron_path_is_free_except_the_mainnet_keys_of_other_chains() {
        for free in ["m/44'/195'/9'/0/1", "m/44'/966'/0'/0/0", "m/84'/1'/0'/0/0", "m/0", "m"] {
            assert_eq!(check_tron_path(&p(free), None), Ok(()), "{free}");
        }
        for taken in [
            "m/84'/0'/0'/0/0",
            "m/86'/0'/5'/1/2",
            "m/44'/0'/0'/0/0",
            "m/49'/0'/0'/0/0",
            "m/44'/60'/0'/0/0",
            "m/44'/501'/0'/0'",
        ] {
            assert_eq!(
                check_tron_path(&p(taken), None),
                Err(Error::PathNetworkMismatch),
                "{taken}"
            );
            assert_eq!(
                check_tron_path(&p(taken), Some(Network::Tron)),
                Err(Error::PathNetworkMismatch),
                "{taken}"
            );
        }
    }

    #[test]
    fn known_tron_networks_keep_their_keys_apart() {
        let check = |path: &str, network| check_tron_path(&p(path), Some(network));
        assert_eq!(check("m/44'/195'/0'/0/1", Network::Tron), Ok(()));
        assert_eq!(check("m/44'/1'/0'/0/7", Network::TronNile), Ok(()));
        assert_eq!(check("m/44'/1'/0'/0/0", Network::Tron), Err(Error::PathNetworkMismatch));
        assert_eq!(
            check("m/44'/195'/0'/0/0", Network::TronShasta),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(check("m/44'/966'/0'/0/0", Network::TronNile), Ok(()));
    }

    #[test]
    fn only_the_exported_evm_account_is_usual() {
        assert!(is_usual_evm_path(&p("m/44'/60'/0'/0/0")));
        assert!(!is_usual_evm_path(&p("m/44'/60'/0'/0/1")));
        assert!(!is_usual_evm_path(&p("m/44'/195'/0'/0/0")));
    }

    #[test]
    fn any_hardened_solana_path_signs_on_its_own_key() {
        for free in ["m/44'/501'/7'/0'", "m/44'/501'/0'", "m/0'", "m"] {
            assert_eq!(check_solana_path(&p(free), Some(Network::Solana)), Ok(()), "{free}");
            assert_eq!(check_solana_path(&p(free), None), Ok(()), "{free}");
        }
        assert_eq!(
            check_solana_path(&p("m/44'/501'/7'/0'"), Some(Network::SolanaDevnet)),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(
            check_solana_path(&p("m/44'/1'/0'/0'"), Some(Network::Solana)),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(check_solana_path(&p("m/44'/1'/0'/0'"), None), Ok(()));
    }

    #[test]
    fn a_solana_path_with_a_normal_level_has_no_key() {
        assert_eq!(
            check_solana_path(&p("m/44'/501'/0'/0"), None),
            Err(Error::PathNotAllowed)
        );
        assert_eq!(
            check_solana_path(&p("m/44'/60'/0'/0/0"), Some(Network::Solana)),
            Err(Error::PathNetworkMismatch)
        );
    }

    #[test]
    fn only_the_exported_solana_accounts_are_usual() {
        assert!(is_usual_solana_path(&p("m/44'/501'/0'/0'")));
        assert!(is_usual_solana_path(&p("m/44'/1'/0'/0'")));
        assert!(!is_usual_solana_path(&p("m/44'/501'/1'/0'")));
    }

    #[test]
    fn only_the_exported_tron_accounts_are_usual() {
        assert!(is_usual_tron_path(&p("m/44'/195'/0'/0/0")));
        assert!(is_usual_tron_path(&p("m/44'/1'/0'/0/0")));
        assert!(!is_usual_tron_path(&p("m/44'/195'/0'/0/1")));
        assert!(!is_usual_tron_path(&p("m/44'/60'/0'/0/0")));
    }

    #[test]
    fn bitcoin_accounts_and_indexes_are_not_limited() {
        for path in [
            "m/84'/0'/9'/0/0",
            "m/84'/0'/0'/7/1",
            "m/84'/0'/0'/1/999999999",
            "m/49'/1'/0'/0/0",
        ] {
            assert_eq!(classify(&p(path)).unwrap().chain, Chain::Bitcoin, "{path}");
        }
    }

    #[test]
    fn only_another_key_or_chain_contradicts_a_bitcoin_path() {
        let check = |path: &str, network| check_bitcoin_path(&p(path), network);
        assert_eq!(
            check("m/84'/1'/0'/0/0", Some(Network::Bitcoin)),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(
            check("m/84'/0'/0'/0/0", Some(Network::BtcSignet)),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(
            check("m/44'/60'/0'/0/0", Some(Network::Bitcoin)),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(check("m/44'/1'/0'/0/0", Some(Network::BtcSignet)), Ok(()));
        for free in ["m/9'/7'/0'", "m/0", "m", "m/84'/0'/5'/9/9"] {
            assert_eq!(check(free, Some(Network::Bitcoin)), Ok(()), "{free}");
        }
        assert_eq!(check("m/44'/60'/0'/0/0", None), Ok(()));
    }

    #[test]
    fn usual_means_under_an_exported_account_of_the_same_kind() {
        let usual = |path: &str, kind| is_usual_bitcoin_path(&p(path), kind);
        assert!(usual("m/84'/0'/0'/1/999999999", AccountKind::SegWit));
        assert!(usual("m/44'/0'/0'/0/0", AccountKind::Legacy));
        assert!(!usual("m/84'/0'/1'/0/0", AccountKind::SegWit));
        assert!(!usual("m/84'/0'/0'/2/0", AccountKind::SegWit));
        assert!(!usual("m/84'/0'/0'/0/0", AccountKind::Legacy));
        assert!(!usual("m/84'/0'/0'/0/5'", AccountKind::SegWit));
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

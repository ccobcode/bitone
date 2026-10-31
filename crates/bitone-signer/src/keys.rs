//! 从种子派生账户公钥。私钥只在这里出现，用完即弃。

use crate::Error;
use bitcoin::NetworkKind;
use bitcoin::bip32::{DerivationPath as BipPath, Xpriv, Xpub};
use bitcoin::secp256k1::{All, Secp256k1};
use bitone_core::path::{AccountSpec, all_account_specs};
use bitone_core::protocol::{self, AccountsExport, ExportedAccount};
use bitone_core::{Chain, DerivationPath, Mode};
use ed25519_dalek::SigningKey;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha512;
use std::str::FromStr;
use zeroize::Zeroizing;

const HARDENED: u32 = 0x8000_0000;

/// 解锁后的钱包。只持有种子，随对象销毁而清除。
pub struct Wallet {
    seed: Zeroizing<[u8; 64]>,
    secp: Secp256k1<All>,
}

impl Wallet {
    pub fn new(entropy: &[u8], passphrase: &str) -> Result<Self, Error> {
        Ok(Self {
            seed: crate::mnemonic::seed(entropy, passphrase)?,
            secp: Secp256k1::new(),
        })
    }

    fn master(&self, mode: Mode) -> Result<Xpriv, Error> {
        let kind = match mode {
            Mode::Mainnet => NetworkKind::Main,
            Mode::Testnet => NetworkKind::Test,
        };
        Xpriv::new_master(kind, self.seed.as_ref()).map_err(|_| Error::Derivation)
    }

    pub fn fingerprint(&self) -> Result<[u8; 4], Error> {
        Ok(self.master(Mode::Mainnet)?.fingerprint(&self.secp).to_bytes())
    }

    pub(crate) fn secp(&self) -> &Secp256k1<All> {
        &self.secp
    }

    pub(crate) fn secp_key(&self, path: &str, mode: Mode) -> Result<Xpriv, Error> {
        let path = BipPath::from_str(path).map_err(|_| Error::Derivation)?;
        self.master(mode)?
            .derive_priv(&self.secp, &path)
            .map_err(|_| Error::Derivation)
    }

    /// 种子不离开这个模块，所以 ed25519 私钥从这里取。
    pub(crate) fn ed25519_key(&self, path: &DerivationPath) -> Result<Zeroizing<[u8; 32]>, Error> {
        slip10_ed25519(self.seed.as_ref(), path)
    }

    fn solana_public_key(&self, path: &str) -> Result<[u8; 32], Error> {
        let key = slip10_ed25519(self.seed.as_ref(), &DerivationPath::parse(path)?)?;
        Ok(SigningKey::from_bytes(&key).verifying_key().to_bytes())
    }

    fn exported_key(&self, spec: &AccountSpec, mode: Mode) -> Result<Vec<u8>, Error> {
        match spec.chain {
            Chain::Bitcoin => {
                let xpriv = self.secp_key(spec.path, mode)?;
                Ok(Xpub::from_priv(&self.secp, &xpriv).encode().to_vec())
            }
            Chain::Evm | Chain::Tron => {
                let xpriv = self.secp_key(spec.path, mode)?;
                Ok(xpriv.private_key.public_key(&self.secp).serialize().to_vec())
            }
            Chain::Solana => Ok(self.solana_public_key(spec.path)?.to_vec()),
        }
    }

    /// 生成导出给网页端的账户信息，只含公钥。一次导出全部账户，网页端自己按网络分开。
    pub fn export(&self, imported: bool) -> Result<AccountsExport, Error> {
        let accounts = all_account_specs(imported)
            .iter()
            .map(|spec| {
                // 测试币账户的路径用 coin type 1'，扩展公钥的版本前缀也跟着变。
                let mode = if spec.path.contains("/1'/") {
                    Mode::Testnet
                } else {
                    Mode::Mainnet
                };
                Ok(ExportedAccount {
                    chain: spec.chain,
                    kind: spec.kind,
                    path: spec.path.to_string(),
                    key: self.exported_key(spec, mode)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(AccountsExport {
            version: protocol::VERSION,
            fingerprint: self.fingerprint()?,
            accounts,
        })
    }
}

/// SLIP-0010 的 ed25519 派生，只支持硬化路径。
fn slip10_ed25519(seed: &[u8], path: &DerivationPath) -> Result<Zeroizing<[u8; 32]>, Error> {
    let split = |mac: Hmac<Sha512>| {
        let output = Zeroizing::new(<[u8; 64]>::from(mac.finalize().into_bytes()));
        let key = Zeroizing::new(<[u8; 32]>::try_from(&output[..32]).expect("32 字节"));
        let chain = Zeroizing::new(<[u8; 32]>::try_from(&output[32..]).expect("32 字节"));
        (key, chain)
    };
    let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(b"ed25519 seed").map_err(|_| Error::Derivation)?;
    mac.update(seed);
    let (mut key, mut chain) = split(mac);
    for &level in &path.0 {
        if level & HARDENED == 0 {
            return Err(Error::Derivation);
        }
        let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(chain.as_ref()).map_err(|_| Error::Derivation)?;
        mac.update(&[0]);
        mac.update(key.as_ref());
        mac.update(&level.to_be_bytes());
        (key, chain) = split(mac);
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitone_core::address::{bitcoin_address, evm_address, solana_address, tron_address};
    use bitone_core::path::classify;
    use bitone_core::{AccountKind, Network};
    use hex_literal::hex;

    // BIP84 和 BIP86 的测试向量都用这组助记词。
    fn abandon() -> Wallet {
        Wallet::new(&[0u8; 16], "").unwrap()
    }

    fn key(export: &AccountsExport, path: &str) -> Vec<u8> {
        export.accounts.iter().find(|a| a.path == path).unwrap().key.clone()
    }

    #[test]
    fn master_fingerprint_matches_bip84_vector() {
        assert_eq!(abandon().fingerprint().unwrap(), hex!("73c5da0a"));
    }

    #[test]
    fn bip84_vectors() {
        let export = abandon().export(false).unwrap();
        let xpub = key(&export, "m/84'/0'/0'");
        let address =
            |change, index| bitcoin_address(&xpub, AccountKind::SegWit, change, index, Network::Bitcoin).unwrap();
        assert_eq!(address(false, 0), "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        assert_eq!(address(false, 1), "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g");
        assert_eq!(address(true, 0), "bc1q8c6fshw2dlwun7ekn9qwf37cu2rn755upcp6el");
    }

    #[test]
    fn bip86_vectors() {
        let export = abandon().export(false).unwrap();
        let xpub = key(&export, "m/86'/0'/0'");
        let address =
            |change, index| bitcoin_address(&xpub, AccountKind::Taproot, change, index, Network::Bitcoin).unwrap();
        assert_eq!(
            address(false, 0),
            "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr"
        );
        assert_eq!(
            address(false, 1),
            "bc1p4qhjn9zdvkux4e44uhx8tc55attvtyu358kutcqkudyccelu0was9fqzwh"
        );
        assert_eq!(
            address(true, 0),
            "bc1p3qkhfews2uk44qtvauqyr2ttdsw7svhkl9nkm9s9c3x4ax5h60wqwruhk7"
        );
    }

    #[test]
    fn imported_old_paths_derive_known_addresses() {
        let export = abandon().export(true).unwrap();
        let legacy = key(&export, "m/44'/0'/0'");
        let nested = key(&export, "m/49'/0'/0'");
        assert_eq!(
            bitcoin_address(&legacy, AccountKind::Legacy, false, 0, Network::Bitcoin).unwrap(),
            "1LqBGSKuX5yYUonjxT5qGfpUsXKYYWeabA"
        );
        assert_eq!(
            bitcoin_address(&nested, AccountKind::NestedSegWit, false, 0, Network::Bitcoin).unwrap(),
            "37VucYSaXLCAsxYyAPfbSi9eh4iEcbShgf"
        );
    }

    #[test]
    fn evm_vector() {
        let export = abandon().export(false).unwrap();
        assert_eq!(
            evm_address(&key(&export, "m/44'/60'/0'/0/0")).unwrap(),
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
        );
    }

    #[test]
    fn slip10_ed25519_vector() {
        let seed = hex!("000102030405060708090a0b0c0d0e0f");
        let path = DerivationPath::parse("m/0'/1'").unwrap();
        assert_eq!(
            *slip10_ed25519(&seed, &path).unwrap(),
            hex!("b1d0bad404bf35da785a64ca1ac54b2617211d2777696fbffaf208f746ae84f2")
        );
    }

    #[test]
    fn solana_vector() {
        let export = abandon().export(false).unwrap();
        assert_eq!(
            solana_address(&key(&export, "m/44'/501'/0'/0'")).unwrap(),
            "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk"
        );
    }

    #[test]
    fn testnet_keys_differ_from_mainnet_on_tron_and_solana() {
        let wallet = abandon();
        let main = wallet.export(false).unwrap();
        let test = wallet.export(false).unwrap();
        assert_ne!(
            tron_address(&key(&main, "m/44'/195'/0'/0/0")).unwrap(),
            tron_address(&key(&test, "m/44'/1'/0'/0/0")).unwrap()
        );
        assert_ne!(key(&main, "m/44'/501'/0'/0'"), key(&test, "m/44'/1'/0'/0'"));
    }

    // TRON 和 Solana 没有官方向量，这几个值用独立实现算出（@scure/bip32、Node 的 ed25519）。
    #[test]
    fn tron_vectors() {
        let wallet = abandon();
        let main = wallet.export(false).unwrap();
        let test = wallet.export(false).unwrap();
        assert_eq!(
            tron_address(&key(&main, "m/44'/195'/0'/0/0")).unwrap(),
            "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH"
        );
        assert_eq!(
            tron_address(&key(&test, "m/44'/1'/0'/0/0")).unwrap(),
            "TS8utE69gMxZo48E2fhTBNtkWBsEPZWLGc"
        );
    }

    #[test]
    fn solana_testnet_vector() {
        let test = abandon().export(false).unwrap();
        assert_eq!(
            solana_address(&key(&test, "m/44'/1'/0'/0'")).unwrap(),
            "9BbGG7UMnNRJGMk1m52F4Q4gCSUnSVBosiT9kJhmgWvQ"
        );
    }

    #[test]
    fn passphrase_changes_the_wallet() {
        let plain = abandon().fingerprint().unwrap();
        let hidden = Wallet::new(&[0u8; 16], "TREZOR").unwrap().fingerprint().unwrap();
        assert_ne!(plain, hidden);
    }

    #[test]
    fn export_contains_only_public_material() {
        let export = abandon().export(true).unwrap();
        for account in &export.accounts {
            match account.chain {
                Chain::Bitcoin => assert_eq!(account.key.len(), 78),
                Chain::Evm | Chain::Tron => assert_eq!(account.key.len(), 33),
                Chain::Solana => assert_eq!(account.key.len(), 32),
            }
        }
        let bytes = export.encode();
        assert!(!bytes.windows(4).any(|w| w == hex!("0488ade4")), "不能出现 xprv 版本号");
    }

    #[test]
    fn every_exported_account_decodes_on_the_other_side() {
        let export = abandon().export(true).unwrap();
        assert_eq!(export.accounts.len(), 11);
        let decoded = AccountsExport::decode(&export.encode()).unwrap();
        assert_eq!(decoded, export);
        for account in decoded.accounts {
            let path = DerivationPath::parse(&account.path).unwrap();
            if account.chain != Chain::Bitcoin {
                assert!(classify(&path).is_ok(), "{}", account.path);
            }
        }
    }
}

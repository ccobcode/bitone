//! 冷钱包保存的公钥。核对交易时不需要解锁，只有签名才要 PIN，所以公钥单独存一份。
//! 里面没有任何私钥。

use crate::path::DerivationPath;
use crate::protocol::{AccountsExport, ExportedAccount};
use crate::protocol::{chain_from_code, kind_from_code};
use crate::review::Forbid;
use crate::{AccountKind, Chain, Error, Mode, Network, address};
use bitcoin::bip32::{ChildNumber, Xpub};
use bitcoin::secp256k1::Secp256k1;

/// 首页显示的账户。Bitcoin 是第一个收款地址。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountAddress {
    pub chain: Chain,
    pub kind: AccountKind,
    pub path: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKeys {
    pub fingerprint: [u8; 4],
    accounts: Vec<ExportedAccount>,
}

impl PublicKeys {
    /// 可以传入多份导出，指纹必须相同。
    pub fn new(exports: &[AccountsExport]) -> Result<Self, Error> {
        let first = exports.first().ok_or(Error::Decode)?;
        if exports.iter().any(|e| e.fingerprint != first.fingerprint) {
            return Err(Error::Decode);
        }
        let mut accounts: Vec<ExportedAccount> = Vec::new();
        for account in exports.iter().flat_map(|e| &e.accounts) {
            if !accounts.iter().any(|a| a.path == account.path) {
                accounts.push(account.clone());
            }
        }
        Ok(Self {
            fingerprint: first.fingerprint,
            accounts,
        })
    }

    /// 序列化后写入本机，下次启动不用解锁也能核对。
    pub fn encode(exports: &[AccountsExport]) -> Vec<u8> {
        minicbor::to_vec(exports).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        crate::protocol::check_limits(bytes)?;
        let exports: Vec<AccountsExport> = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        Self::new(&exports)
    }

    /// 首页的账户和地址。测试币账户不列出来，用到时请求里有路径。
    pub fn addresses(&self) -> Result<Vec<AccountAddress>, Error> {
        let bitcoin_network = Network::Bitcoin;
        let mut list = Vec::new();
        for spec in crate::path::account_specs(Mode::Mainnet, true) {
            let Some(account) = self.accounts.iter().find(|a| a.path == spec.path) else {
                continue;
            };
            let chain = chain_from_code(account.chain)?;
            let kind = kind_from_code(account.kind)?;
            let text = match chain {
                Chain::Bitcoin => address::bitcoin_address(&account.key, kind, false, 0, bitcoin_network)?,
                Chain::Evm => address::evm_address(&account.key)?,
                Chain::Tron => address::tron_address(&account.key)?,
                Chain::Solana => address::solana_address(&account.key)?,
            };
            list.push(AccountAddress {
                chain,
                kind,
                path: account.path.clone(),
                address: text,
            });
        }
        Ok(list)
    }

    /// 账户的第 `index` 个收款地址。只有 Bitcoin 有多个，其他链的 `index` 必须是 0。
    pub fn receive_address(&self, account_path: &str, index: u32) -> Result<String, Error> {
        let account = self
            .accounts
            .iter()
            .find(|a| a.path == account_path)
            .ok_or(Error::PathNotAllowed)?;
        let chain = chain_from_code(account.chain)?;
        let kind = kind_from_code(account.kind)?;
        if chain != Chain::Bitcoin && index != 0 {
            return Err(Error::InvalidParams);
        }
        if index >= crate::path::MAX_ADDRESS_INDEX {
            return Err(Error::IndexTooLarge);
        }
        let bitcoin_network = if account_path.contains("/1'/") {
            Network::BtcTestnet4
        } else {
            Network::Bitcoin
        };
        match chain {
            Chain::Bitcoin => address::bitcoin_address(&account.key, kind, false, index, bitcoin_network),
            Chain::Evm => address::evm_address(&account.key),
            Chain::Tron => address::tron_address(&account.key),
            Chain::Solana => address::solana_address(&account.key),
        }
    }

    fn exact(&self, path: &DerivationPath) -> Option<&ExportedAccount> {
        let text = path.to_string();
        self.accounts.iter().find(|a| a.path == text)
    }

    /// secp256k1 压缩公钥。Bitcoin 路径从账户扩展公钥继续派生。
    pub fn secp256k1(&self, path: &DerivationPath) -> Result<[u8; 33], Forbid> {
        if let Some(account) = self.exact(path) {
            return account.key.as_slice().try_into().map_err(|_| Forbid::AddressMismatch);
        }
        let [a, b, c, change, index] = path.0.as_slice() else {
            return Err(Forbid::PathNotAllowed);
        };
        let parent = DerivationPath(vec![*a, *b, *c]);
        let account = self.exact(&parent).ok_or(Forbid::PathNotAllowed)?;
        let xpub = Xpub::decode(&account.key).map_err(|_| Forbid::AddressMismatch)?;
        let children = [
            ChildNumber::from_normal_idx(*change).map_err(|_| Forbid::PathNotAllowed)?,
            ChildNumber::from_normal_idx(*index).map_err(|_| Forbid::PathNotAllowed)?,
        ];
        let child = xpub
            .derive_pub(&Secp256k1::verification_only(), &children)
            .map_err(|_| Forbid::AddressMismatch)?;
        Ok(child.public_key.serialize())
    }

    pub fn ed25519(&self, path: &DerivationPath) -> Result<[u8; 32], Forbid> {
        let account = self.exact(path).ok_or(Forbid::PathNotAllowed)?;
        account.key.as_slice().try_into().map_err(|_| Forbid::AddressMismatch)
    }
}

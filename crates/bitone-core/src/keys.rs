//! 冷钱包保存的公钥.核对交易时不需要解锁,只有签名才要 PIN,所以公钥单独存一份.
//! 里面没有任何私钥.

use crate::path::DerivationPath;
use crate::protocol::{AccountsExport, ExportedAccount};
use crate::review::Forbid;
use crate::{AccountKind, Chain, Error, Mode, Network, address};
use bitcoin::bip32::{ChildNumber, Xpub};
use bitcoin::secp256k1::Secp256k1;

/// 首页显示的账户.Bitcoin 是第一个收款地址.
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
    pub fn new(export: AccountsExport) -> Self {
        Self {
            fingerprint: export.fingerprint,
            accounts: export.accounts,
        }
    }

    /// 序列化后写入本机,下次启动不用解锁也能核对.存的是只有一项的数组.
    pub fn encode(export: &AccountsExport) -> Vec<u8> {
        minicbor::to_vec([export]).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        crate::protocol::check_limits(bytes)?;
        let exports: Vec<AccountsExport> = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        let [export]: [AccountsExport; 1] = exports.try_into().map_err(|_| Error::Decode)?;
        Ok(Self::new(export))
    }

    /// 首页的账户和地址.测试币账户不列出来,用到时请求里有路径.
    pub fn addresses(&self) -> Result<Vec<AccountAddress>, Error> {
        let bitcoin_network = Network::Bitcoin;
        let mut list = Vec::new();
        for spec in crate::path::account_specs(Mode::Mainnet, true) {
            let Some(account) = self.accounts.iter().find(|a| a.path == spec.path) else {
                continue;
            };
            let text = match account.chain {
                Chain::Bitcoin => address::bitcoin_address(&account.key, account.kind, false, 0, bitcoin_network)?,
                Chain::Evm => address::evm_address(&account.key)?,
                Chain::Tron => address::tron_address(&account.key)?,
                Chain::Solana => address::solana_address(&account.key)?,
            };
            list.push(AccountAddress {
                chain: account.chain,
                kind: account.kind,
                path: account.path.clone(),
                address: text,
            });
        }
        Ok(list)
    }

    /// 账户的第 `index` 个收款地址.只有 Bitcoin 有多个,其他链的 `index` 必须是 0.
    pub fn receive_address(&self, account_path: &str, index: u32) -> Result<String, Error> {
        let account = self
            .accounts
            .iter()
            .find(|a| a.path == account_path)
            .ok_or(Error::PathNotAllowed)?;
        if account.chain != Chain::Bitcoin && index != 0 {
            return Err(Error::InvalidParams);
        }
        let bitcoin_network = if account_path.contains("/1'/") {
            Network::BtcTestnet4
        } else {
            Network::Bitcoin
        };
        match account.chain {
            Chain::Bitcoin => address::bitcoin_address(&account.key, account.kind, false, index, bitcoin_network),
            Chain::Evm => address::evm_address(&account.key),
            Chain::Tron => address::tron_address(&account.key),
            Chain::Solana => address::solana_address(&account.key),
        }
    }

    fn exact(&self, path: &DerivationPath) -> Option<&ExportedAccount> {
        let text = path.to_string();
        self.accounts.iter().find(|a| a.path == text)
    }

    /// secp256k1 压缩公钥.Bitcoin 路径从账户扩展公钥继续派生.
    /// 没有导出的硬化路径算不出来,结果是 `PathNotAllowed`,这样的路径解锁后才能核对.
    pub fn secp256k1(&self, path: &DerivationPath) -> Result<[u8; 33], Forbid> {
        let bitcoin = self.accounts.iter().filter(|a| a.chain == Chain::Bitcoin);
        let parent = bitcoin
            .filter_map(|account| Some((account, DerivationPath::parse(&account.path).ok()?)))
            .find(|(_, parent)| path.0.starts_with(&parent.0));
        let Some((account, parent)) = parent else {
            let account = self.exact(path).ok_or(Forbid::PathNotAllowed)?;
            return account.key.as_slice().try_into().map_err(|_| Forbid::AddressMismatch);
        };
        let xpub = Xpub::decode(&account.key).map_err(|_| Forbid::AddressMismatch)?;
        let children = path.0[parent.0.len()..]
            .iter()
            .map(|level| ChildNumber::from_normal_idx(*level).map_err(|_| Forbid::PathNotAllowed))
            .collect::<Result<Vec<_>, _>>()?;
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

//! 链的注册表.每条链一个目录,解码,审核,构建,核验和规则表都在自己的目录里,目录之间互不引用.
//! 共享层不出现具体的链,按 `Chain` 分发只在这里.加一条链加一个目录,再在这里接上.
//! 约定由 `tests/architecture.rs` 检查.

pub mod bitcoin;
pub mod evm;
pub mod solana;
pub mod tron;

use crate::keys::PublicKeys;
use crate::path::{AccountKind, DerivationPath};
use crate::policy::RuleTable;
use crate::request::{SignRequest, SignResponse};
use crate::review::{Forbid, Review};
use crate::tx::{ReviewContext, Verified};
use crate::{Chain, Error, Network};

/// 审核一个请求.`network` 为空表示本机不认识请求里的网络名,链来自请求的链字段,路径和载荷.
pub(crate) fn review(
    chain: Chain,
    network: Option<Network>,
    request: &SignRequest,
    ctx: &ReviewContext,
) -> Result<Review, Forbid> {
    match chain {
        Chain::Bitcoin => bitcoin::review(network, request, ctx),
        Chain::Evm => evm::review(network, request, ctx),
        Chain::Tron => tron::review(network, request, ctx),
        Chain::Solana => solana::review(network, request, ctx),
    }
}

/// 核验签名并装回交易.Bitcoin 的网络决定地址格式,本机不认识网络时不能核验.
pub(crate) fn verify(
    chain: Chain,
    network: Option<Network>,
    request: &SignRequest,
    response: &SignResponse,
    signer: &[u8],
) -> Result<Verified, Error> {
    match chain {
        Chain::Bitcoin => network.map_or(Err(Error::InvalidParams), |network| {
            bitcoin::verify(network, request, response)
        }),
        Chain::Evm => evm::verify(request, response, signer),
        Chain::Tron => tron::verify(request, response, signer),
        Chain::Solana => solana::verify(request, response, signer),
    }
}

/// 由保存的公钥派生地址.
pub(crate) fn derive_address(
    network: Network,
    keys: &PublicKeys,
    path: &DerivationPath,
    kind: AccountKind,
) -> Result<String, Forbid> {
    let derived = match network.chain() {
        Chain::Bitcoin => crate::address::bitcoin_address_from_key(&keys.secp256k1(path)?, kind, network),
        Chain::Evm => crate::address::evm_address(&keys.secp256k1(path)?),
        Chain::Tron => crate::address::tron_address(&keys.secp256k1(path)?),
        Chain::Solana => crate::address::solana_address(&keys.ed25519(path)?),
    };
    derived.map_err(|_| Forbid::AddressMismatch)
}

/// 请求里只有一条路径,没有链字段,本机也不认识网络名时,哪条链认领这条路径和载荷.
/// 顺序就是判断顺序:coin type `1'` 不只属于一条链,EVM 和 TRON 的载荷能认出来,Solana 最后.
pub(crate) fn claimed_by(path: &str, payload: &[u8]) -> Option<Chain> {
    if evm::claims(path, payload) {
        Some(Chain::Evm)
    } else if tron::claims(path, payload) {
        Some(Chain::Tron)
    } else if solana::claims(path, payload) {
        Some(Chain::Solana)
    } else {
        None
    }
}

/// 没有路径的请求只有 Bitcoin,路径在 PSBT 里.
pub(crate) fn claimed_without_path(payload: &[u8]) -> Option<Chain> {
    bitcoin::is_psbt(payload).then_some(Chain::Bitcoin)
}

pub(crate) fn rules(chain: Chain) -> &'static RuleTable {
    match chain {
        Chain::Bitcoin => bitcoin::policy::rules(),
        Chain::Evm => evm::policy::rules(),
        Chain::Tron => tron::policy::rules(),
        Chain::Solana => solana::policy::rules(),
    }
}

//! 交易解析,分类,构建和核验.
//!
//! 冷钱包的处理顺序见 docs/03-system.md 第 6 节:核对指纹,校验路径和网络,解析载荷,分类,展示.
//! 任何一步失败都变成"禁止签名",不存在解析失败却能继续的路径.

pub mod abi;
pub mod bitcoin;
pub mod evm;
pub mod solana;
pub mod tron;

use crate::hex::DisplayHex;
use crate::keys::PublicKeys;
use crate::manifest::Manifest;
use crate::path::{DerivationPath, validate_for_network};
use crate::protocol::AddressCheck;
use crate::request::{SignRequest, SignResponse};
use crate::review::{Forbid, RawLine, Review};
use crate::{AccountKind, Chain, Error, Mode, Network};

/// 本机已签过的最高找零索引,见 docs/04-security.md 第 2.6 节.
/// 测试币的索引不能放宽主网的告警,所以分开记.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighestChange {
    pub mainnet: u32,
    pub testnet: u32,
}

impl HighestChange {
    pub fn of(self, network: Network) -> u32 {
        match network.mode() {
            Mode::Mainnet => self.mainnet,
            Mode::Testnet => self.testnet,
        }
    }
}

pub struct ReviewContext<'a> {
    pub keys: &'a PublicKeys,
    pub manifest: &'a Manifest,
    pub highest_change: HighestChange,
}

pub fn review(request: &SignRequest, ctx: &ReviewContext) -> Review {
    try_review(request, ctx).unwrap_or_else(|reason| Review::forbidden(&request.network, reason))
}

fn try_review(request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    if request.fingerprint != ctx.keys.fingerprint {
        return Err(Forbid::FingerprintMismatch);
    }
    let network = Network::from_id(&request.network).ok_or(Forbid::UnknownNetwork)?;
    match network.chain() {
        Chain::Bitcoin => bitcoin::review(network, request, ctx),
        Chain::Evm => evm::review(network, request, ctx),
        Chain::Tron => tron::review(network, request, ctx),
        Chain::Solana => solana::review(network, request, ctx),
    }
}

/// 冷钱包核对收款地址的结果.见 docs/04-security.md 第 4 节.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressResult {
    pub belongs: bool,
    pub network: String,
    pub path: String,
    /// 二维码里的地址.
    pub address: String,
    /// 冷钱包从路径派生出的地址.
    pub derived: String,
    pub kind: AccountKind,
}

/// 从路径重新派生,再和二维码里的地址比对.
pub fn check_address(check: &AddressCheck, keys: &PublicKeys) -> Result<AddressResult, Forbid> {
    if check.fingerprint != keys.fingerprint {
        return Err(Forbid::FingerprintMismatch);
    }
    let network = Network::from_id(&check.network).ok_or(Forbid::UnknownNetwork)?;
    let path = DerivationPath::parse(&check.path).map_err(|_| Forbid::PathNotAllowed)?;
    let info = validate_for_network(&path, network).map_err(|_| Forbid::PathNotAllowed)?;
    let derived = match network.chain() {
        Chain::Bitcoin => crate::address::bitcoin_address_from_key(&keys.secp256k1(&path)?, info.kind, network),
        Chain::Evm => crate::address::evm_address(&keys.secp256k1(&path)?),
        Chain::Tron => crate::address::tron_address(&keys.secp256k1(&path)?),
        Chain::Solana => crate::address::solana_address(&keys.ed25519(&path)?),
    }
    .map_err(|_| Forbid::AddressMismatch)?;
    Ok(AddressResult {
        belongs: derived == check.address,
        network: check.network.clone(),
        path: check.path.clone(),
        address: check.address.clone(),
        derived,
        kind: info.kind,
    })
}

/// 网页端广播前的结果.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// 可以直接广播的原始交易.
    pub raw: Vec<u8>,
    /// 交易哈希,用于跟踪.
    pub hash: String,
}

/// 用公钥验证签名,并把签名装回构建记录.见 docs/03-system.md 第 8 节.
/// `signer` 是付款账户的公钥,Bitcoin 的公钥在 PSBT 里,可以留空.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    if request.id != response.id {
        return Err(Error::ResponseMismatch);
    }
    let network = Network::from_id(&request.network).ok_or(Error::InvalidParams)?;
    match network.chain() {
        Chain::Bitcoin => bitcoin::verify(network, request, response),
        Chain::Evm => evm::verify(request, response, signer),
        Chain::Tron => tron::verify(request, response, signer),
        Chain::Solana => solana::verify(request, response, signer),
    }
}

/// 单路径的请求(EVM,TRON,Solana)取出并校验路径.
pub(crate) fn signer_path(request: &SignRequest, network: Network) -> Result<DerivationPath, Forbid> {
    let [path] = request.paths.as_slice() else {
        return Err(Forbid::PathNotAllowed);
    };
    let path = DerivationPath::parse(path).map_err(|_| Forbid::PathNotAllowed)?;
    validate_for_network(&path, network).map_err(|e| match e {
        Error::PathNetworkMismatch => Forbid::PathNetworkMismatch,
        _ => Forbid::PathNotAllowed,
    })?;
    Ok(path)
}

/// 验证 65 字节的 r,s,恢复位,要求低 s.EVM 和 TRON 都不接受高 s.
pub(crate) fn check_signature(hash: &[u8; 32], signature: &[u8; 65], signer: &[u8]) -> Result<(), Error> {
    use ::bitcoin::secp256k1::{Message, PublicKey, Secp256k1, ecdsa};
    let recovery = ecdsa::RecoveryId::from_i32(signature[64] as i32).map_err(|_| Error::BadSignature)?;
    let recoverable =
        ecdsa::RecoverableSignature::from_compact(&signature[..64], recovery).map_err(|_| Error::BadSignature)?;
    let recovered = Secp256k1::verification_only()
        .recover_ecdsa(&Message::from_digest(*hash), &recoverable)
        .map_err(|_| Error::BadSignature)?;
    if recovered != PublicKey::from_slice(signer).map_err(|_| Error::InvalidKey)? {
        return Err(Error::BadSignature);
    }
    let original = recoverable.to_standard();
    let mut normal = original;
    normal.normalize_s();
    if normal != original {
        return Err(Error::BadSignature);
    }
    Ok(())
}

/// 原始数据分行:选择器一行,之后每 32 字节一行,标出疑似地址和大额数值.
pub fn raw_lines(data: &[u8]) -> Vec<RawLine> {
    let hex = |bytes: &[u8]| bytes.to_lower_hex_string();
    if data.len() < 4 {
        return vec![RawLine {
            text: hex(data),
            note: None,
        }];
    }
    let mut lines = vec![RawLine {
        text: hex(&data[..4]),
        note: Some("方法选择器".into()),
    }];
    for chunk in data[4..].chunks(32) {
        let note = if chunk.len() == 32 && chunk[..12] == [0; 12] && chunk[12..16] != [0; 4] {
            Some("疑似地址".to_string())
        } else if chunk.len() == 32 && chunk[0] == 0xff {
            Some("大额数值".to_string())
        } else {
            None
        };
        lines.push(RawLine { text: hex(chunk), note });
    }
    lines
}

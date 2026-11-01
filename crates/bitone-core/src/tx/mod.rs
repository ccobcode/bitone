//! 交易解析,分类,构建和核验.
//!
//! 冷钱包的处理顺序:核对指纹,校验路径和网络,解析载荷,分类,展示.
//! 任何一步失败都变成"禁止签名",不存在解析失败却能继续的路径.

pub mod abi;
pub mod bitcoin;
pub mod evm;
pub mod solana;
mod summary;
pub mod tron;

use crate::decode::solana::{SolanaPayload, parse_solana};
use crate::decode::{Decoded, EvmPayload, Failure, is_psbt, parse_evm};
use crate::keys::PublicKeys;
use crate::manifest::{Asset, Manifest};
use crate::path::{DerivationPath, classify, is_all_hardened, solana_path_mode, validate_for_network};
use crate::policy::{self, AssetFact, Facts};
use crate::protocol::{AddressCheck, VERSION};
use crate::request::{SignRequest, SignResponse};
use crate::review::{Forbid, Review};
use crate::{AccountKind, Chain, Error, Mode, Network};
use std::borrow::Cow;

/// 本机已签过的最高找零索引.
/// 测试币的索引不能放宽主网的告警,所以分开记.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighestChange {
    pub mainnet: u32,
    pub testnet: u32,
}

impl HighestChange {
    pub fn of(self, mode: Mode) -> u32 {
        match mode {
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

impl ReviewContext<'_> {
    /// 原生币的符号和小数位.清单没有这一行时用链本身的数据,不拒绝签名.
    fn native(&self, network: Network) -> Cow<'_, Asset> {
        self.manifest
            .native(network.id())
            .map_or_else(|| Cow::Owned(Asset::plain_native(network)), Cow::Borrowed)
    }
}

impl Facts for ReviewContext<'_> {
    fn asset(&self, network: &str, contract: &str) -> Option<AssetFact> {
        let asset = self.manifest.lookup(network, contract)?;
        Some(AssetFact {
            symbol: asset.symbol.clone(),
            decimals: asset.decimals,
        })
    }

    fn highest_change(&self, mode: Mode) -> u32 {
        self.highest_change.of(mode)
    }
}

pub fn review(request: &SignRequest, ctx: &ReviewContext) -> Review {
    let review = try_review(request, ctx).unwrap_or_else(|reason| Review::forbidden(&request.network, reason));
    if request.version > VERSION {
        review.alerted("这个请求来自更高的协议版本,本机可能不认识其中的部分内容")
    } else {
        review
    }
}

/// 解码失败只有格式已识别而字节不合规一种情况.
pub fn review_decoded(decoded: Result<Decoded, Failure>, network: &str, ctx: &ReviewContext) -> Review {
    match decoded {
        Ok(decoded) => Review::assessed(&decoded, policy::assess(&decoded, ctx)),
        Err(failure) => Review::undecodable(network, &failure),
    }
}

/// 路径的 coin type 是 `60'`,或者载荷就是一笔 EVM 交易.路径不设白名单,所以两个都要看.
fn looks_like_evm(path: &str, payload: &[u8]) -> bool {
    let by_path = DerivationPath::parse(path)
        .ok()
        .and_then(|path| classify(&path).ok())
        .is_some_and(|info| info.chain == Chain::Evm);
    by_path || matches!(parse_evm(payload), Ok(EvmPayload::Transaction(_)))
}

/// coin type `501'` 只属于 Solana.`1'` 不只属于一条链,所以路径认不出来时看载荷.
fn looks_like_solana(path: &str, payload: &[u8]) -> bool {
    let Ok(path) = DerivationPath::parse(path) else {
        return false;
    };
    let by_path = solana_path_mode(&path) == Some(Mode::Mainnet);
    let by_payload = is_all_hardened(&path)
        && matches!(
            parse_solana(payload),
            Ok(SolanaPayload::Message(_) | SolanaPayload::Partial { .. } | SolanaPayload::Offchain { .. })
        );
    by_path || by_payload
}

/// 路径的 coin type 是 `195'`,或者载荷就是一笔 TRON 交易.coin type `1'` 说明不了是哪条链,要看载荷.
fn looks_like_tron(path: &str, payload: &[u8]) -> bool {
    let by_path = DerivationPath::parse(path)
        .ok()
        .and_then(|path| classify(&path).ok())
        .is_some_and(|info| info.chain == Chain::Tron && info.mode == Some(Mode::Mainnet));
    by_path || tron::UnsignedTx::decode(payload).is_ok()
}

fn chain_of_path(path: &str, payload: &[u8]) -> Option<Chain> {
    if looks_like_evm(path, payload) {
        Some(Chain::Evm)
    } else if looks_like_tron(path, payload) {
        Some(Chain::Tron)
    } else if looks_like_solana(path, payload) {
        Some(Chain::Solana)
    } else {
        None
    }
}

/// 请求属于哪条链.网络是本机认识的就看网络,否则看请求的链字段,没有时按路径和载荷判断.
/// Bitcoin 的路径在 PSBT 里,所以 `paths` 是空的.
pub fn request_chain(request: &SignRequest) -> Option<Chain> {
    match (Network::from_id(&request.network), request.chain) {
        (Some(network), _) => Some(network.chain()),
        (None, Some(number)) => Chain::from_number(number),
        (None, None) => match request.paths.as_slice() {
            [] => is_psbt(&request.payload).then_some(Chain::Bitcoin),
            [path] => chain_of_path(path, &request.payload),
            [_, _, ..] => None,
        },
    }
}

fn try_review(request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    if request.fingerprint != ctx.keys.fingerprint {
        return Err(Forbid::FingerprintMismatch);
    }
    let Some(network) = Network::from_id(&request.network) else {
        return match request_chain(request) {
            Some(Chain::Bitcoin) => bitcoin::review(None, request, ctx),
            Some(Chain::Evm) => evm::review(None, request, ctx),
            Some(Chain::Solana) => solana::review(None, request, ctx),
            Some(Chain::Tron) => tron::review(None, request, ctx),
            None => Err(Forbid::UnknownNetwork),
        };
    };
    let review = match network.chain() {
        Chain::Bitcoin => bitcoin::review(Some(network), request, ctx),
        Chain::Evm => evm::review(Some(network), request, ctx),
        Chain::Tron => tron::review(Some(network), request, ctx),
        Chain::Solana => solana::review(Some(network), request, ctx),
    }?;
    Ok(native_alert(review, network, ctx))
}

/// 清单缺失不影响签名,只影响名称和图标.
pub(crate) fn native_alert(review: Review, network: Network, ctx: &ReviewContext) -> Review {
    if ctx.manifest.native(network.id()).is_some() {
        review
    } else {
        review.alerted("清单里没有这个网络的原生币,金额按链本身的数据显示")
    }
}

/// 冷钱包核对收款地址的结果.
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
    /// 可以直接广播的原始交易.`signature_only` 时是 65 字节的签名.
    pub raw: Vec<u8>,
    /// 交易哈希,用于跟踪.`signature_only` 时是签名原文的哈希.
    pub hash: String,
    /// Bitcoin 的手续费没有经过前序交易确认,签名也不能替它担保.
    pub fee_unconfirmed: bool,
    /// 消息和未知的交易类型没有可以广播的形式,结果只有核验过的签名.
    pub signature_only: bool,
}

/// 用公钥验证签名,并把签名装回构建记录.
/// `signer` 是付款账户的公钥,Bitcoin 的公钥在 PSBT 里,可以留空.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    if request.id != response.id {
        return Err(Error::ResponseMismatch);
    }
    let Some(network) = Network::from_id(&request.network) else {
        return match request_chain(request) {
            Some(Chain::Evm) => evm::verify(request, response, signer),
            Some(Chain::Solana) => solana::verify(request, response, signer),
            Some(Chain::Tron) => tron::verify(request, response, signer),
            Some(Chain::Bitcoin) | None => Err(Error::InvalidParams),
        };
    };
    match network.chain() {
        Chain::Bitcoin => bitcoin::verify(network, request, response),
        Chain::Evm => evm::verify(request, response, signer),
        Chain::Tron => tron::verify(request, response, signer),
        Chain::Solana => solana::verify(request, response, signer),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode;
    use crate::decode::fixture::{PARTIAL, Records, UNKNOWN_FORMAT, input};
    use crate::review::{Confirm, Forbid, Risk};

    fn keys() -> PublicKeys {
        PublicKeys::new(crate::protocol::AccountsExport {
            version: crate::protocol::VERSION,
            fingerprint: [0; 4],
            accounts: Vec::new(),
        })
    }

    fn context(keys: &PublicKeys) -> ReviewContext<'_> {
        ReviewContext {
            keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange { mainnet: 7, testnet: 9 },
        }
    }

    #[test]
    fn facts_come_from_the_manifest_and_the_change_index() {
        let keys = keys();
        let ctx = context(&keys);
        assert_eq!(ctx.asset("ethereum", "native").unwrap().symbol, "ETH");
        assert_eq!(ctx.asset("ethereum", "0xdead"), None);
        assert_eq!(Facts::highest_change(&ctx, Mode::Mainnet), 7);
        assert_eq!(Facts::highest_change(&ctx, Mode::Testnet), 9);
    }

    #[test]
    fn a_higher_version_reaches_the_review_with_a_warning() {
        let keys = keys();
        let ctx = context(&keys);
        let mut request = SignRequest {
            version: VERSION,
            id: [1; 16],
            fingerprint: [0; 4],
            network: "ethereum".into(),
            kind: 0,
            paths: Vec::new(),
            payload: Vec::new(),
            hint: None,
            chain: None,
        };
        assert_eq!(review(&request, &ctx).alerts.len(), 1);
        request.version = VERSION + 1;
        let alerts = review(&request, &ctx).alerts;
        assert_eq!(alerts.len(), 2);
        assert!(alerts[1].contains("协议版本"));
    }

    #[test]
    fn unknown_content_reaches_the_user_as_signable() {
        let keys = keys();
        let ctx = context(&keys);
        let cases = [
            (vec![UNKNOWN_FORMAT, 1], Confirm::HoldConfirm),
            (PARTIAL.to_vec(), Confirm::Hold),
        ];
        for (payload, confirm) in cases {
            let review = review_decoded(decode(&Records, &input(&payload)), "ethereum", &ctx);
            assert_eq!((review.risk, review.confirm), (Risk::Unknown, confirm));
            assert!(review.is_signable());
            assert!(!review.fields.is_empty());
            assert!(!review.alerts.is_empty());
        }
    }

    fn evm_request(network: &str, chain: Option<u8>, paths: &[&str]) -> SignRequest {
        SignRequest {
            version: VERSION,
            id: [1; 16],
            fingerprint: [0; 4],
            network: network.into(),
            kind: 0,
            paths: paths.iter().map(ToString::to_string).collect(),
            payload: vec![2, 0xc0],
            hint: None,
            chain,
        }
    }

    #[test]
    fn the_chain_field_wins_then_the_path_and_the_payload_say_evm() {
        assert_eq!(
            request_chain(&evm_request("local", Some(1), &["m/0"])),
            Some(Chain::Evm)
        );
        assert_eq!(
            request_chain(&evm_request("local", None, &["m/44'/60'/4'/0/0"])),
            Some(Chain::Evm)
        );
        assert_eq!(
            request_chain(&evm_request("tron", Some(1), &["m/0"])),
            Some(Chain::Tron)
        );
        let unclear = SignRequest {
            payload: vec![9],
            ..evm_request("local", None, &["m/44'/1'/0'/0/0"])
        };
        assert_eq!(request_chain(&unclear), None);
    }

    #[test]
    fn a_whole_evm_transaction_says_evm_for_any_path() {
        let tx = evm::build_call(&evm::CallParams {
            network: Network::Ethereum,
            to: None,
            value: "0",
            data: vec![1],
            nonce: 0,
            gas_limit: 1,
            max_fee: 1,
            max_priority: 1,
        })
        .unwrap();
        let request = SignRequest {
            payload: tx.encode(),
            ..evm_request("local", None, &["m/0"])
        };
        assert_eq!(request_chain(&request), Some(Chain::Evm));
    }

    #[test]
    fn a_failed_check_is_the_only_refusal_here() {
        let keys = keys();
        let ctx = context(&keys);
        let review = review_decoded(decode(&Records, &input(&[1, 5])), "ethereum", &ctx);
        assert_eq!(review.forbidden, Some(Forbid::ParseFailed));
        assert!(!review.is_signable());
        assert!(review.fields.is_empty());
    }
}

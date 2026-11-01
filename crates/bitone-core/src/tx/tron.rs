//! TRON:核对,签名原文,构建和核验.解码在 `decode`,这里算出策略要的事实.
//!
//! 载荷是 `raw_data` 的 protobuf 字节,签名哈希是它的 SHA-256.第 1.1 节:未知的合约类型和字段都可以签,
//! 解码和规则只负责让用户看清楚.`owner_address` 不是本账户时,只有 TRON 的多签才能完成签名.

use crate::address::{eth_address_bytes, parse_tron_address, tron_text};
use crate::decode::{
    self, Coin, Decoded, Decoder, Input, Node, RawHashWalker, TronPersonalWalker, TronWalker, TypedFlavor, TypedWalker,
    parse_typed, tron_contract, tron_message_hash, tron_owner, tron_permission_id, tron_signed, tron_type_url,
};
use crate::manifest::Asset;
use crate::path::{DerivationPath, check_tron_path, is_usual_tron_path};
use crate::policy::{self, OwnerFact, TronFacts, expiration_in_window};
use crate::request::{KIND_PERSONAL_SIGN, KIND_RAW_HASH, KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
use crate::review::{Forbid, Review, Summary};
use crate::tx::abi::{self, Style};
use crate::tx::evm::{MAX_PATH_DEPTH, be_from_decimal, typed};
use crate::tx::{ReviewContext, Verified, check_signature, summary};
use crate::{Chain, Error, Mode, Network};
use bitcoin::hex::DisplayHex;
use prost::Message;
use sha2::{Digest, Sha256};
use types::{Any, Contract, ContractType, Raw};

pub use crate::decode::read_varint;

/// 官方类型.构建和测试用.
pub mod types {
    pub use crate::decode::tron_any::Any;
    pub use crate::decode::tron_types::transaction::contract::ContractType;
    pub use crate::decode::tron_types::transaction::{Contract, Raw};
    pub use crate::decode::tron_types::*;
}

const ADDRESS_PREFIX: u8 = 0x41;
/// 清单条目的标准.TRC-10 的编号只和这种条目比较.
const TRC10: &str = "TRC-10";

fn prefixed(address: &[u8; 20]) -> Vec<u8> {
    [&[ADDRESS_PREFIX], address.as_slice()].concat()
}

/// 树里的地址已经检查过前缀和长度.
fn address(bytes: &[u8]) -> Option<[u8; 20]> {
    bytes.get(1..)?.try_into().ok()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// 只认交易.解码器保证每个字段都进了树,重新编码回完全相同的字节.
fn checked(payload: &[u8]) -> Result<Decoded, decode::Failure> {
    decode::decode(
        &TronWalker { coin: None, call: &[] },
        &Input {
            chain: Chain::Tron,
            mode: None,
            network: Chain::Tron.id(),
            kind: KIND_TRANSACTION,
            paths: &[],
            payload,
            signer: None,
        },
    )
}

/// 未签名的交易.只能由检查过的载荷得到,官方类型跳过的未知字段都已经进了树.
#[derive(Clone, Debug, PartialEq)]
pub struct UnsignedTx(Raw);

impl UnsignedTx {
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        checked(payload).map_err(|_| Forbid::ParseFailed)?;
        Raw::decode(payload).map(Self).map_err(|_| Forbid::ParseFailed)
    }

    pub fn expiration(&self) -> i64 {
        self.0.expiration
    }

    pub fn fee_limit(&self) -> i64 {
        self.0.fee_limit
    }
}

/// 路径不设白名单.只有一条路径才知道用哪把私钥.
fn request_path(request: &SignRequest, network: Option<Network>) -> Result<DerivationPath, Forbid> {
    let [path] = request.paths.as_slice() else {
        return Err(Forbid::PathNotAllowed);
    };
    let path = DerivationPath::parse(path).map_err(|_| Forbid::PathNotAllowed)?;
    if path.0.len() > MAX_PATH_DEPTH {
        return Err(Forbid::PathNotAllowed);
    }
    check_tron_path(&path, network).map_err(|_| Forbid::PathNetworkMismatch)?;
    Ok(path)
}

/// 付款地址.没有导出的路径保存的公钥算不出来,解锁后才能核对.
fn signer_address(ctx: &ReviewContext, path: &DerivationPath) -> Result<Option<[u8; 20]>, Forbid> {
    match ctx.keys.secp256k1(path) {
        Ok(key) => Ok(Some(eth_address_bytes(&key).map_err(|_| Forbid::AddressMismatch)?)),
        Err(Forbid::PathNotAllowed) => Ok(None),
        Err(reason) => Err(reason),
    }
}

/// 这笔请求的网络.本机不认识时网络名只是请求里的声明.
struct Target<'a> {
    network: Option<Network>,
    name: &'a str,
}

impl Target<'_> {
    fn id(&self) -> &str {
        match self.network {
            Some(network) => network.id(),
            None => self.name,
        }
    }
}

/// 解码之前算好的合约调用.解码器不读清单,调用数据的字段要先解出来.类别和告警都来自策略.
struct Prepared {
    summary: Summary,
    /// 描述文件解出的字段.
    call: Vec<Node>,
}

fn call_prepared(value: &[u8], target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Option<Prepared> {
    let call = types::TriggerSmartContract::decode(value).ok()?;
    let style = Style {
        chain: Chain::Tron,
        network: target.id(),
        manifest: ctx.manifest,
        format: &tron_text,
    };
    let contract = address(&call.contract_address)?;
    let has_value = call.call_value != 0 || call.call_token_value != 0;
    let classified = abi::classify(&style, &contract, &call.data, has_value);
    facts.calls = classified.facts();
    facts.call_note = Some(classified.note());
    Some(Prepared {
        summary: Summary {
            amount: classified.amount,
            counterparty_label: classified.counterparty_label,
            counterparty: classified.counterparty,
            ..Summary::default()
        },
        call: classified.fields,
    })
}

/// TRC-10 的编号只和清单里的 TRC-10 条目比较,`native` 和 TRC-20 的地址都不是编号.
fn trc10<'a>(ctx: &ReviewContext<'a>, target: &Target<'_>, id: &str) -> Option<&'a Asset> {
    let asset = ctx.manifest.lookup(target.id(), id)?;
    (asset.standard == TRC10).then_some(asset)
}

/// 页面顶部.调用数据不在树的字段里,其余合约都按规则表读树.
fn tree_summary(decoded: &Decoded, target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Summary {
    let Some((kind, value)) = tron_contract(decoded.root()) else {
        return Summary::default();
    };
    let native = target.network.map(|network| ctx.native(network));
    let effect = summary::effect(Chain::Tron, kind, 0, value, native.as_deref(), |id, _| {
        trc10(ctx, target, id)
    });
    facts.permission_shared = effect.shared;
    if kind == policy::ASSET_ACTION {
        let amount = effect.amount.as_ref();
        facts.listed_asset = amount.and_then(|view| Some([view.text.as_str(), view.symbol.as_deref()?].join(" ")));
    }
    summary::headline(vec![(effect, 0)])
}

fn prepared(raw: &Raw, target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Option<Prepared> {
    let contract = raw.contract.first()?;
    let any = contract.parameter.as_ref()?;
    match ContractType::try_from(contract.r#type) {
        Ok(ContractType::TriggerSmartContract) => call_prepared(&any.value, target, ctx, facts),
        _ => None,
    }
}

/// `owner_address` 不是本账户时,`Permission_id` 为 0 的签名节点不接受,不为 0 是 TRON 的多签.
fn owner_fact(decoded: &Decoded, signer: Option<&str>) -> Result<OwnerFact, Forbid> {
    let multisig = tron_permission_id(decoded.root()) != 0;
    let Some(owner) = tron_owner(decoded.root()).map(str::to_string) else {
        return Ok(OwnerFact::Own);
    };
    match (signer, multisig) {
        (Some(signer), _) if signer == owner => Ok(OwnerFact::Own),
        (Some(_), true) => Ok(OwnerFact::Foreign(owner)),
        (Some(_), false) => Err(Forbid::AddressMismatch),
        (None, true) => Ok(OwnerFact::UnverifiedMultisig(owner)),
        (None, false) => Ok(OwnerFact::Unverified(owner)),
    }
}

/// 这笔请求的公共部分.
struct Reviewing<'a> {
    request: &'a SignRequest,
    ctx: &'a ReviewContext<'a>,
    declared: Option<Network>,
    target: Target<'a>,
    path: DerivationPath,
    signer: Option<String>,
}

impl Reviewing<'_> {
    fn facts(&self) -> TronFacts {
        TronFacts {
            signer: self.signer.clone(),
            unusual_path: (!is_usual_tron_path(&self.path)).then(|| self.path.to_string()),
            ..TronFacts::default()
        }
    }

    fn input(&self, mode: Mode) -> Input<'_> {
        Input {
            chain: Chain::Tron,
            mode: Some(mode),
            network: self.target.id(),
            kind: self.request.kind,
            paths: &self.request.paths,
            payload: &self.request.payload,
            signer: self.signer.as_deref(),
        }
    }

    fn summarized(&self, review: Review, summary: Summary) -> Review {
        review.summarized(Summary {
            payer: self.signer.clone(),
            ..summary
        })
    }

    /// 消息和裸哈希不绑定链,网络名只是声明,所以一直有主网提示.
    fn unbound(&self, decoder: &impl Decoder) -> Review {
        match decode::decode(decoder, &self.input(Mode::Mainnet)) {
            Ok(decoded) => {
                let assessment = policy::assess_tron_message(&decoded, &self.facts());
                let review = Review {
                    mainnet: true,
                    ..Review::assessed(&decoded, assessment)
                };
                self.summarized(review, Summary::default())
            }
            Err(failure) => Review::undecodable(&self.request.network, &failure),
        }
    }

    /// TIP-712.和 EIP-712 的差别只有地址,`trcToken` 和 `chainId` 的取值,见第 5.2 节.
    fn typed(&self) -> Review {
        let parsed = match parse_typed(&self.request.payload, TypedFlavor::Tip712) {
            Ok(parsed) => parsed,
            Err(reason) => {
                let digest = decode::payload_digest(&self.request.payload);
                return Review::undecodable(&self.request.network, &decode::Failure { reason, digest });
            }
        };
        let expected = self.declared.and_then(Network::tip712_chain_id);
        let found = parsed.chain_id().and_then(|id| u64::try_from(id).ok());
        let proven = found.filter(|id| expected == Some(*id)).is_some();
        let mode = self.declared.filter(|_| proven).map_or(Mode::Mainnet, Network::mode);
        let style = Style {
            chain: Chain::Tron,
            network: self.target.id(),
            manifest: self.ctx.manifest,
            format: &tron_text,
        };
        let view = typed::view(&parsed, &style);
        let labels = typed::labels(&parsed, &style);
        let walker = TypedWalker {
            parsed: &parsed,
            chain_id: expected,
            format: &tron_text,
            labels: labels.as_deref(),
        };
        match decode::decode(&walker, &self.input(mode)) {
            Ok(decoded) => {
                let facts = TronFacts {
                    permit: view.permit,
                    ..self.facts()
                };
                let assessment = policy::assess_tron_message(&decoded, &facts);
                let review = Review {
                    mainnet: mode == Mode::Mainnet,
                    ..Review::assessed(&decoded, assessment)
                };
                let summary = Summary {
                    amount: view.amount,
                    counterparty_label: view.counterparty_label,
                    counterparty: view.counterparty,
                    ..Summary::default()
                };
                self.summarized(review, summary)
            }
            Err(failure) => Review::undecodable(&self.request.network, &failure),
        }
    }
}

pub fn review(declared: Option<Network>, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = request_path(request, declared)?;
    let signer = signer_address(ctx, &path)?;
    let reviewing = Reviewing {
        request,
        ctx,
        declared,
        target: Target {
            network: declared,
            name: &request.network,
        },
        path,
        signer: signer.as_ref().map(tron_text),
    };
    match request.kind {
        KIND_TRANSACTION => transaction(&reviewing),
        KIND_TYPED_DATA => Ok(reviewing.typed()),
        KIND_RAW_HASH => Ok(reviewing.unbound(&RawHashWalker)),
        KIND_PERSONAL_SIGN => Ok(reviewing.unbound(&TronPersonalWalker)),
        _ => Err(Forbid::Unsupported),
    }
}

fn transaction(reviewing: &Reviewing<'_>) -> Result<Review, Forbid> {
    let Reviewing {
        request,
        ctx,
        declared,
        target,
        ..
    } = reviewing;
    let declared = *declared;
    let coin = declared
        .map(Network::native_coin)
        .map(|(unit, decimals)| Coin { unit, decimals });
    let mut facts = reviewing.facts();
    // 解码器不读清单,调用数据的字段要先算出来.载荷不合规时下面的解码会拒绝,这里的结果不会用到.
    let raw = Raw::decode(request.payload.as_slice()).map_err(|_| Forbid::ParseFailed);
    let call = raw.as_ref().ok().and_then(|raw| prepared(raw, target, ctx, &mut facts));
    let fields = call.as_ref().map_or(&[][..], |call| call.call.as_slice());
    let decoded = decode::decode(
        &TronWalker { coin, call: fields },
        &reviewing.input(declared.map_or(Mode::Mainnet, Network::mode)),
    );
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(failure) => return Ok(Review::undecodable(&request.network, &failure)),
    };
    facts.owner = owner_fact(&decoded, reviewing.signer.as_deref())?;
    let raw = raw?;
    facts.fee_high = declared.is_some_and(|network| i128::from(raw.fee_limit) > policy::fee_ceiling(network) as i128);
    let read = tree_summary(&decoded, target, ctx, &mut facts);
    let mut summary = call.map_or(read, |call| call.summary);
    let assessment = policy::assess_tron(&decoded, &facts);
    // 这一行装着解出的参数,全部资产授权的"无限"不在别的行里.
    let trigger = decoded
        .actions()
        .into_iter()
        .find(|(_, _, kind)| *kind == policy::TRIGGER_ACTION);
    summary.field_ids.extend(trigger.map(|(id, _, _)| id));
    Ok(reviewing.summarized(Review::assessed(&decoded, assessment), summary))
}

/// `signer` 能不能替这笔交易签名.发起方不是它时只有 `Permission_id` 不为 0 的多签才行.
/// 保存的公钥算不出来的路径到解锁之后才有结论.
pub fn check_signer(payload: &[u8], signer: &[u8]) -> Result<(), Error> {
    let decoded = checked(payload).map_err(|_| Error::InvalidParams)?;
    let address = tron_text(&eth_address_bytes(signer).map_err(|_| Error::InvalidKey)?);
    let root = decoded.root();
    if tron_owner(root).is_some_and(|owner| owner != address) && tron_permission_id(root) == 0 {
        return Err(Error::BadSignature);
    }
    Ok(())
}

/// 签名原文的哈希.`request` 已经过核对.
pub fn signing_hash(request: &SignRequest) -> Result<[u8; 32], Forbid> {
    match request.kind {
        KIND_TRANSACTION => {
            checked(&request.payload).map_err(|_| Forbid::ParseFailed)?;
            Ok(sha256(&request.payload))
        }
        KIND_TYPED_DATA => {
            let parsed = parse_typed(&request.payload, TypedFlavor::Tip712).map_err(|_| Forbid::ParseFailed)?;
            Ok(parsed.digest.0)
        }
        KIND_RAW_HASH => request.payload.as_slice().try_into().map_err(|_| Forbid::ParseFailed),
        KIND_PERSONAL_SIGN => Ok(tron_message_hash(&request.payload).0),
        _ => Err(Forbid::Unsupported),
    }
}

pub struct TransferParams<'a> {
    pub network: Network,
    pub from: &'a str,
    pub to: &'a str,
    /// 最小单位的十进制字符串.
    pub amount: &'a str,
    /// TRC-20 合约.留空是 TRX 转账.
    pub contract: Option<&'a str>,
    pub ref_block_bytes: [u8; 2],
    pub ref_block_hash: [u8; 8],
    pub expiration_ms: u64,
    pub timestamp_ms: u64,
    /// 只有 TRC-20 转账写入 `raw_data`.
    pub fee_limit: u64,
}

fn native_amount_param(text: &str) -> Result<i64, Error> {
    let bytes = be_from_decimal(text)?;
    if bytes.len() > 8 {
        return Err(Error::InvalidParams);
    }
    let amount = bytes.iter().fold(0u64, |acc, b| acc << 8 | u64::from(*b));
    i64::try_from(amount).map_err(|_| Error::InvalidParams)
}

fn transfer_data(to: &[u8; 20], amount: &str) -> Result<Vec<u8>, Error> {
    let amount = be_from_decimal(amount)?;
    let mut data = vec![0xa9, 0x05, 0x9c, 0xbb];
    data.extend_from_slice(&[0; 12]);
    data.extend_from_slice(to);
    data.resize(data.len() + 32 - amount.len(), 0);
    data.extend_from_slice(&amount);
    Ok(data)
}

fn contract(kind: ContractType, value: Vec<u8>) -> Contract {
    Contract {
        r#type: kind as i32,
        parameter: Some(Any {
            type_url: tron_type_url(kind.as_str_name()),
            value,
        }),
        ..Contract::default()
    }
}

fn millis(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::InvalidParams)
}

/// 构建转账,返回 `raw_data` 的字节.
pub fn build_transfer(params: &TransferParams) -> Result<Vec<u8>, Error> {
    if params.network.chain() != Chain::Tron {
        return Err(Error::InvalidParams);
    }
    let owner = prefixed(&parse_tron_address(params.from)?);
    let to = parse_tron_address(params.to)?;
    let (call, fee_limit) = match params.contract {
        None => {
            let value = types::TransferContract {
                owner_address: owner,
                to_address: prefixed(&to),
                amount: native_amount_param(params.amount)?,
            };
            (contract(ContractType::TransferContract, value.encode_to_vec()), 0)
        }
        Some(token) => {
            let value = types::TriggerSmartContract {
                owner_address: owner,
                contract_address: prefixed(&parse_tron_address(token)?),
                data: transfer_data(&to, params.amount)?,
                ..types::TriggerSmartContract::default()
            };
            let fee_limit = millis(params.fee_limit)?;
            (
                contract(ContractType::TriggerSmartContract, value.encode_to_vec()),
                fee_limit,
            )
        }
    };
    let expiration = millis(params.expiration_ms)?;
    let timestamp = millis(params.timestamp_ms)?;
    // 这笔交易是这里构建的,过期时间不合理就不构建.收到别处构建的交易时只告警.
    if !expiration_in_window(expiration, timestamp) {
        return Err(Error::InvalidParams);
    }
    let raw = Raw {
        ref_block_bytes: params.ref_block_bytes.to_vec(),
        ref_block_hash: params.ref_block_hash.to_vec(),
        expiration,
        contract: vec![call],
        timestamp,
        fee_limit,
        ..Raw::default()
    }
    .encode_to_vec();
    // 冷钱包不接受的交易这里就不构建.
    checked(&raw).map_err(|_| Error::InvalidParams)?;
    Ok(raw)
}

/// `signer` 是付款账户的压缩公钥.多签时它不是 `raw_data` 里的发起方.
/// 消息和裸哈希没有可以广播的形式,结果只有核验过的签名.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 65] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    if request.kind != KIND_TRANSACTION {
        let digest = signing_hash(request).map_err(|_| Error::InvalidParams)?;
        check_signature(&digest, signature, signer)?;
        return Ok(Verified {
            raw: signature.to_vec(),
            hash: digest.to_lower_hex_string(),
            fee_unconfirmed: false,
            signature_only: true,
        });
    }
    check_signer(&request.payload, signer)?;
    let hash = sha256(&request.payload);
    check_signature(&hash, signature, signer)?;
    Ok(Verified {
        raw: tron_signed(&request.payload, signature),
        hash: hash.to_lower_hex_string(),
        fee_unconfirmed: false,
        signature_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Forbid;

    const OWNER: [u8; 20] = [0x11; 20];
    const PEER: [u8; 20] = [0x22; 20];
    const USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const NOW: u64 = 1_800_000_000_000;

    fn transfer(amount: i64) -> Vec<u8> {
        let value = types::TransferContract {
            owner_address: prefixed(&OWNER),
            to_address: prefixed(&PEER),
            amount,
        };
        Raw {
            ref_block_bytes: vec![0xab, 0xcd],
            ref_block_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            expiration: NOW as i64 + 1_800_000,
            contract: vec![contract(ContractType::TransferContract, value.encode_to_vec())],
            timestamp: NOW as i64,
            ..Raw::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn a_checked_payload_keeps_its_bytes() {
        let bytes = transfer(5);
        assert_eq!(UnsignedTx::decode(&bytes).unwrap().0.encode_to_vec(), bytes);
    }

    #[test]
    fn length_past_the_end_is_refused() {
        let mut bytes = transfer(5);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(UnsignedTx::decode(&bytes), Err(Forbid::ParseFailed));
        let huge = [vec![0x0a], vec![0xff; 9], vec![0x01]].concat();
        assert_eq!(UnsignedTx::decode(&huge), Err(Forbid::ParseFailed));
    }

    #[test]
    fn oversized_tags_are_refused() {
        // Java 的 protobuf 只读低 32 位,这个标签在节点上会变成字段 2,也就是第二个收款方.
        let tag = [0x92, 0x80, 0x80, 0x80, 0x10];
        let hidden = [transfer(5), tag.to_vec(), vec![21], prefixed(&PEER)].concat();
        assert_eq!(UnsignedTx::decode(&hidden), Err(Forbid::ParseFailed));
    }

    #[test]
    fn a_transaction_the_chain_may_not_accept_can_still_be_decoded() {
        // 这里只看字节.过期时间由策略告警,见 `policy::expiration_in_window`.
        let value = types::TransferContract {
            owner_address: prefixed(&OWNER),
            to_address: prefixed(&PEER),
            amount: 5,
        };
        let raw = Raw {
            ref_block_bytes: vec![0, 1],
            ref_block_hash: vec![0; 8],
            expiration: 1,
            contract: vec![contract(ContractType::TransferContract, value.encode_to_vec())],
            timestamp: NOW as i64,
            ..Raw::default()
        };
        assert!(UnsignedTx::decode(&raw.encode_to_vec()).is_ok());
    }

    #[test]
    fn build_refuses_what_it_should_not_build() {
        let params = |expiration_ms, from, amount| TransferParams {
            network: Network::Tron,
            from,
            to: USDT,
            amount,
            contract: None,
            ref_block_bytes: [0; 2],
            ref_block_hash: [0; 8],
            expiration_ms,
            timestamp_ms: NOW,
            fee_limit: 0,
        };
        let owner = tron_text(&OWNER);
        assert!(build_transfer(&params(NOW + 1000, &owner, "1")).is_ok());
        assert_eq!(build_transfer(&params(NOW, &owner, "1")), Err(Error::InvalidParams));
        assert_eq!(
            build_transfer(&params(NOW + 1000, "nope", "1")),
            Err(Error::InvalidAddress)
        );
        assert_eq!(
            build_transfer(&params(NOW + 1000, &owner, "-1")),
            Err(Error::InvalidParams)
        );
        assert_eq!(
            build_transfer(&params(NOW + 1000, &owner, "9223372036854775808")),
            Err(Error::InvalidParams)
        );
        assert!(build_transfer(&params(NOW + 1000, &owner, "9223372036854775807")).is_ok());
        let mut wrong_chain = params(NOW + 1000, &owner, "1");
        wrong_chain.network = Network::Solana;
        assert_eq!(build_transfer(&wrong_chain), Err(Error::InvalidParams));
    }
}

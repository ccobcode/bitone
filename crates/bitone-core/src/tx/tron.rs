//! TRON:核对,签名原文,构建和核验.解码在 `decode`,这里算出策略要的事实.
//!
//! 载荷是 `raw_data` 的 protobuf 字节,签名哈希是它的 SHA-256.第 1.1 节:未知的合约类型和字段都可以签,
//! 解码和规则只负责让用户看清楚.`owner_address` 不是本账户时,只有 TRON 的多签才能完成签名.

use crate::address::{eth_address_bytes, parse_tron_address, tron_text};
use crate::decode::{
    self, Coin, Decoded, Decoder, Input, Node, RawHashWalker, TronPersonalWalker, TronWalker, TypedFlavor, TypedWalker,
    parse_typed, tron_message_hash, tron_owner, tron_permission_id, tron_signed, tron_type_url,
};
use crate::path::{DerivationPath, check_tron_path, is_usual_tron_path};
use crate::policy::{self, OwnerFact, TronFacts, expiration_in_window};
use crate::request::{KIND_PERSONAL_SIGN, KIND_RAW_HASH, KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
use crate::review::{ActionView, AmountView, Forbid, RawLine, Review, Risk, Row};
use crate::tx::abi::{self, Style};
use crate::tx::evm::{MAX_PATH_DEPTH, be_from_decimal, call_note, typed};
use crate::tx::{ReviewContext, Verified, check_signature, raw_lines};
use crate::units::format_units;
use crate::{Chain, Error, Mode, Network};
use bitcoin::hex::DisplayHex;
use prost::Message;
use sha2::{Digest, Sha256};
use types::{Any, Contract, ContractType, Permission, Raw};

pub use crate::decode::read_varint;

/// 官方类型.构建和测试用.
pub mod types {
    pub use crate::decode::tron_any::Any;
    pub use crate::decode::tron_types::transaction::contract::ContractType;
    pub use crate::decode::tron_types::transaction::{Contract, Raw};
    pub use crate::decode::tron_types::*;
}

const ADDRESS_PREFIX: u8 = 0x41;

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

/// 毫秒时间戳转成 UTC 时间,算法见 Howard Hinnant 的 civil_from_days.
pub(crate) fn utc_time(ms: u64) -> String {
    let seconds = ms / 1000;
    let z = (seconds / 86_400) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rest = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

fn permission_rows(label: &str, permission: &Permission) -> Vec<Row> {
    let mut rows = vec![Row::text(&format!("{label}阈值"), permission.threshold.to_string())];
    rows.extend(permission.keys.iter().filter_map(|key| {
        Some(Row::mono(
            &format!("{label}地址"),
            format!("{} 权重 {}", tron_text(&address(&key.address)?), key.weight),
        ))
    }));
    rows
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

/// 页面顶部的摘要和旧字段,第 9 节删除.类别和告警都来自策略.
#[derive(Default)]
struct Summary {
    amount: Option<AmountView>,
    counterparty_label: Option<String>,
    counterparty: Option<String>,
    /// 已经有类别的动作.其余合约的动作类别就是这笔交易的类别.
    actions: Vec<ActionView>,
    raw: Vec<RawLine>,
    /// 描述文件解出的字段.
    call: Vec<Node>,
}

fn native_amount(target: &Target<'_>, ctx: &ReviewContext, sun: i64) -> AmountView {
    let raw = sun.to_string();
    // 负数没有小数位的含义,显示原始整数.
    match target.network.filter(|_| sun >= 0) {
        Some(network) => {
            let native = ctx.native(network);
            AmountView {
                text: format_units(&raw, native.decimals),
                symbol: Some(native.symbol.clone()),
                asset: Some(native.key()),
                unlimited: false,
            }
        }
        None => AmountView {
            text: raw,
            symbol: None,
            asset: None,
            unlimited: false,
        },
    }
}

fn transfer_summary(value: &[u8], target: &Target<'_>, ctx: &ReviewContext) -> Option<Summary> {
    let call = types::TransferContract::decode(value).ok()?;
    Some(Summary {
        amount: Some(native_amount(target, ctx, call.amount)),
        counterparty_label: Some("收款地址".into()),
        counterparty: address(&call.to_address).as_ref().map(tron_text),
        actions: vec![ActionView {
            risk: Risk::Transfer,
            title: Risk::Transfer.label().to_string(),
            rows: Vec::new(),
        }],
        ..Summary::default()
    })
}

/// 清单内的 TRC-10 是普通转账,清单外金额是原始整数.
fn asset_summary(value: &[u8], target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Option<Summary> {
    let call = types::TransferAssetContract::decode(value).ok()?;
    let id = std::str::from_utf8(&call.asset_name).unwrap_or_default();
    let raw = call.amount.to_string();
    let amount = match ctx.manifest.lookup(target.id(), id) {
        Some(asset) => {
            let text = format_units(&raw, asset.decimals);
            facts.listed_asset = Some(format!("{text} {}", asset.symbol));
            AmountView {
                text,
                symbol: Some(asset.symbol.clone()),
                asset: Some(asset.key()),
                unlimited: false,
            }
        }
        None => AmountView {
            text: raw,
            symbol: None,
            asset: None,
            unlimited: false,
        },
    };
    Some(Summary {
        amount: Some(amount),
        counterparty_label: Some("收款地址".into()),
        counterparty: address(&call.to_address).as_ref().map(tron_text),
        ..Summary::default()
    })
}

fn call_summary(value: &[u8], target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Option<Summary> {
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
    let actions = classified.actions();
    facts.calls = classified.facts();
    facts.call_note = call_note(&actions);
    Some(Summary {
        amount: classified.amount,
        counterparty_label: classified.counterparty_label,
        counterparty: classified.counterparty,
        actions,
        raw: raw_lines(&call.data),
        call: classified.fields,
    })
}

/// A6d 页面.除了第一个地址,其他地址同样能动用资金,所以全部列出.
fn permission_summary(value: &[u8], facts: &mut TronFacts) -> Option<Summary> {
    let call = types::AccountPermissionUpdateContract::decode(value).ok()?;
    let new_owner = call.owner.as_ref()?;
    let controller = new_owner.keys.first().and_then(|key| address(&key.address))?;
    let owner = address(&call.owner_address)?;
    let mut rows = vec![
        Row::mono("当前控制方", tron_text(&owner)),
        Row::mono("新控制方", tron_text(&controller)),
    ];
    rows.extend(permission_rows("owner 权限", new_owner));
    if let Some(witness) = &call.witness {
        rows.extend(permission_rows("witness 权限", witness));
    }
    for active in &call.actives {
        rows.extend(permission_rows("active 权限", active));
    }
    let holders = std::iter::once(new_owner)
        .chain(&call.actives)
        .flat_map(|permission| &permission.keys);
    facts.permission_shared = holders.into_iter().any(|key| address(&key.address) != Some(controller));
    Some(Summary {
        counterparty_label: Some("新控制方".into()),
        counterparty: Some(tron_text(&controller)),
        actions: vec![ActionView {
            risk: Risk::PermissionChange,
            title: "账户权限更新".to_string(),
            rows,
        }],
        ..Summary::default()
    })
}

/// `raw_data` 不是 ABI 编码,方法选择器和疑似地址这些提示不适用.
fn plain_lines(bytes: &[u8]) -> Vec<RawLine> {
    let mut lines = raw_lines(bytes);
    lines.iter_mut().for_each(|line| line.note = None);
    lines
}

fn contract_of(raw: &Raw) -> (Option<ContractType>, &[u8]) {
    let contract = raw.contract.first();
    let kind = contract.and_then(|contract| ContractType::try_from(contract.r#type).ok());
    let any = contract.and_then(|contract| contract.parameter.as_ref());
    (kind, any.map_or(&[], |any| any.value.as_slice()))
}

fn summarize(raw: &Raw, payload: &[u8], target: &Target<'_>, ctx: &ReviewContext, facts: &mut TronFacts) -> Summary {
    let (kind, value) = contract_of(raw);
    let summary = match kind {
        Some(ContractType::TransferContract) => transfer_summary(value, target, ctx),
        Some(ContractType::TransferAssetContract) => asset_summary(value, target, ctx, facts),
        Some(ContractType::TriggerSmartContract) => call_summary(value, target, ctx, facts),
        Some(ContractType::AccountPermissionUpdateContract) => permission_summary(value, facts),
        _ => None,
    };
    let mut summary = summary.unwrap_or_default();
    if summary.raw.is_empty() {
        summary.raw = plain_lines(payload);
    }
    summary
}

fn contract_title(raw: &Raw) -> String {
    let number = raw.contract.first().map_or(0, |contract| contract.r#type);
    ContractType::try_from(number).map_or_else(|_| number.to_string(), |kind| kind.as_str_name().to_string())
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

    fn rows(&self, extra: Vec<Row>) -> Vec<Row> {
        let signer = self.signer.iter().map(|signer| Row::mono("付款账户", signer));
        signer.chain(extra).collect()
    }

    /// 消息和裸哈希不绑定链,网络名只是声明,所以一直有主网提示.
    fn unbound(&self, decoder: &impl Decoder) -> Review {
        match decode::decode(decoder, &self.input(Mode::Mainnet)) {
            Ok(decoded) => {
                let assessment = policy::assess_tron_message(&decoded, &self.facts());
                Review {
                    rows: self.rows(Vec::new()),
                    mainnet: true,
                    ..Review::assessed(&decoded, assessment)
                }
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
                Review {
                    amount: view.amount,
                    counterparty_label: view.counterparty_label,
                    counterparty: view.counterparty,
                    actions: view.actions,
                    rows: self.rows(view.rows),
                    mainnet: mode == Mode::Mainnet,
                    ..Review::assessed(&decoded, assessment)
                }
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
    let summary = raw.as_ref().map_or_else(
        |_| Summary::default(),
        |raw| summarize(raw, &request.payload, target, ctx, &mut facts),
    );
    let decoded = decode::decode(
        &TronWalker {
            coin,
            call: &summary.call,
        },
        &reviewing.input(declared.map_or(Mode::Mainnet, Network::mode)),
    );
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(failure) => return Ok(Review::undecodable(&request.network, &failure)),
    };
    facts.owner = owner_fact(&decoded, reviewing.signer.as_deref())?;
    let raw = raw?;
    facts.fee_high = declared.is_some_and(|network| i128::from(raw.fee_limit) > policy::fee_ceiling(network) as i128);
    let assessment = policy::assess_tron(&decoded, &facts);
    let actions = if summary.actions.is_empty() {
        vec![ActionView {
            risk: assessment.risk,
            title: contract_title(&raw),
            rows: Vec::new(),
        }]
    } else {
        summary.actions
    };
    Ok(Review {
        amount: summary.amount,
        counterparty_label: summary.counterparty_label,
        counterparty: summary.counterparty,
        actions,
        rows: reviewing.rows(Vec::new()),
        raw: summary.raw,
        ..Review::assessed(&decoded, assessment)
    })
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

    #[test]
    fn utc_time_matches_known_dates() {
        assert_eq!(utc_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc_time(951_782_400_000), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc_time(1_800_000_000_000), "2027-01-15 08:00:00 UTC");
    }
}

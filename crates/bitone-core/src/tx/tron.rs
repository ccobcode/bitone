//! TRON 交易:严格解析,分类,构建和核验.规则见 docs/04-security.md 第 2.7 节.
//!
//! 载荷是 `raw_data` 的 protobuf 字节,签名哈希是它的 SHA-256.解析后必须能重新编码回完全相同的字节,
//! 这样冷钱包展示的内容和实际签名的内容不可能不一致.

use crate::address::{eth_address_bytes, parse_tron_address, tron_text};
use crate::policy;
use crate::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use crate::review::{AmountView, Forbid, RawLine, Review, ReviewBuilder, Risk, Row};
use crate::tx::abi::{self, Style};
use crate::tx::evm::be_from_decimal;
use crate::tx::{ReviewContext, Verified, check_signature, raw_lines, signer_path};
use crate::units::format_units;
use crate::{Chain, Error, Network};
use bitcoin::hex::DisplayHex;
use sha2::{Digest, Sha256};

const ADDRESS_PREFIX: u8 = 0x41;
const TYPE_URL_PREFIX: &str = "type.googleapis.com/protocol.";
/// 协议允许的最长有效期.
const MAX_VALIDITY_MS: u64 = 24 * 60 * 60 * 1000;

const TRANSFER_CONTRACT: u64 = 1;
const TRIGGER_CONTRACT: u64 = 31;
const PERMISSION_UPDATE_CONTRACT: u64 = 46;

/// 合约类型的名字和 `owner_address` 所在字段.大多数是字段 1,但 TransferAsset,AccountUpdate,
/// SetAccountId 是字段 2.不在表里的类型确认不了发起方,一律拒绝.
fn contract_info(kind: u64) -> Option<(&'static str, u64)> {
    Some(match kind {
        0 => ("AccountCreateContract", 1),
        1 => ("TransferContract", 1),
        2 => ("TransferAssetContract", 2),
        3 => ("VoteAssetContract", 1),
        4 => ("VoteWitnessContract", 1),
        5 => ("WitnessCreateContract", 1),
        6 => ("AssetIssueContract", 1),
        8 => ("WitnessUpdateContract", 1),
        9 => ("ParticipateAssetIssueContract", 1),
        10 => ("AccountUpdateContract", 2),
        11 => ("FreezeBalanceContract", 1),
        12 => ("UnfreezeBalanceContract", 1),
        13 => ("WithdrawBalanceContract", 1),
        14 => ("UnfreezeAssetContract", 1),
        15 => ("UpdateAssetContract", 1),
        16 => ("ProposalCreateContract", 1),
        17 => ("ProposalApproveContract", 1),
        18 => ("ProposalDeleteContract", 1),
        19 => ("SetAccountIdContract", 2),
        30 => ("CreateSmartContract", 1),
        31 => ("TriggerSmartContract", 1),
        33 => ("UpdateSettingContract", 1),
        41 => ("ExchangeCreateContract", 1),
        42 => ("ExchangeInjectContract", 1),
        43 => ("ExchangeWithdrawContract", 1),
        44 => ("ExchangeTransactionContract", 1),
        45 => ("UpdateEnergyLimitContract", 1),
        46 => ("AccountPermissionUpdateContract", 1),
        48 => ("ClearABIContract", 1),
        49 => ("UpdateBrokerageContract", 1),
        52 => ("MarketSellAssetContract", 1),
        53 => ("MarketCancelOrderContract", 1),
        54 => ("FreezeBalanceV2Contract", 1),
        55 => ("UnfreezeBalanceV2Contract", 1),
        56 => ("WithdrawExpireUnfreezeContract", 1),
        57 => ("DelegateResourceContract", 1),
        58 => ("UnDelegateResourceContract", 1),
        59 => ("CancelAllUnfreezeV2Contract", 1),
        _ => return None,
    })
}

/// protobuf 的变长整数.多余的编码一律拒绝,同一笔交易不能有两种写法.
pub fn read_varint(buf: &mut &[u8]) -> Result<u64, Forbid> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = buf.split_first().ok_or(Forbid::ParseFailed)?;
        *buf = rest;
        let bits = (byte & 0x7f) as u64;
        // 第 10 个字节只能放最高位,多出来的位会被其他解析器丢掉.
        if shift == 63 && bits > 1 {
            return Err(Forbid::ParseFailed);
        }
        value |= bits << shift;
        if byte & 0x80 == 0 {
            if byte == 0 && shift > 0 {
                return Err(Forbid::ParseFailed);
            }
            return Ok(value);
        }
    }
    Err(Forbid::ParseFailed)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// proto3 不写默认值,所以 0 不编码.
fn put_int(out: &mut Vec<u8>, number: u64, value: u64) {
    if value != 0 {
        put_varint(out, number << 3);
        put_varint(out, value);
    }
}

fn put_bytes(out: &mut Vec<u8>, number: u64, value: &[u8]) {
    if !value.is_empty() {
        put_varint(out, number << 3 | 2);
        put_varint(out, value.len() as u64);
        out.extend_from_slice(value);
    }
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

struct Fields<'a>(Vec<(u64, Value<'a>)>);

impl<'a> Fields<'a> {
    /// 只接受 varint 和 length-delimited.TRON 的系统合约不用其他线格式.
    fn parse(mut bytes: &'a [u8]) -> Result<Self, Forbid> {
        let mut fields = Vec::new();
        while !bytes.is_empty() {
            let tag = read_varint(&mut bytes)?;
            let number = tag >> 3;
            // 节点只读标签的低 32 位,更长的标签在节点上是另一个字段.
            if number == 0 || tag > u32::MAX as u64 {
                return Err(Forbid::ParseFailed);
            }
            let value = match tag & 7 {
                0 => Value::Varint(read_varint(&mut bytes)?),
                2 => {
                    let len = usize::try_from(read_varint(&mut bytes)?).map_err(|_| Forbid::ParseFailed)?;
                    if len > bytes.len() {
                        return Err(Forbid::ParseFailed);
                    }
                    let (value, rest) = bytes.split_at(len);
                    bytes = rest;
                    Value::Bytes(value)
                }
                _ => return Err(Forbid::ParseFailed),
            };
            fields.push((number, value));
        }
        Ok(Self(fields))
    }

    /// 未知字段不接受.`repeated` 之外的字段不能重复,否则不同解析器可能取到不同的值.
    fn strict(bytes: &'a [u8], known: &[u64], repeated: &[u64]) -> Result<Self, Forbid> {
        let fields = Self::parse(bytes)?;
        for (i, (number, _)) in fields.0.iter().enumerate() {
            let duplicate = fields.0[..i].iter().any(|(other, _)| other == number);
            if !known.contains(number) || (duplicate && !repeated.contains(number)) {
                return Err(Forbid::ParseFailed);
            }
        }
        Ok(fields)
    }

    fn find(&self, number: u64) -> Option<Value<'a>> {
        self.0.iter().find(|(n, _)| *n == number).map(|(_, value)| *value)
    }

    /// 协议里这些字段都是有符号的 int32 或 int64,负数不合法.
    fn int(&self, number: u64) -> Result<u64, Forbid> {
        match self.find(number) {
            None => Ok(0),
            Some(Value::Varint(value)) if value <= i64::MAX as u64 => Ok(value),
            _ => Err(Forbid::ParseFailed),
        }
    }

    fn bytes(&self, number: u64) -> Result<&'a [u8], Forbid> {
        match self.find(number) {
            None => Ok(&[]),
            Some(Value::Bytes(value)) => Ok(value),
            Some(Value::Varint(_)) => Err(Forbid::ParseFailed),
        }
    }

    fn list(&self, number: u64) -> Result<Vec<&'a [u8]>, Forbid> {
        self.0
            .iter()
            .filter(|(n, _)| *n == number)
            .map(|(_, value)| match value {
                Value::Bytes(bytes) => Ok(*bytes),
                Value::Varint(_) => Err(Forbid::ParseFailed),
            })
            .collect()
    }
}

fn address(bytes: &[u8]) -> Result<[u8; 20], Forbid> {
    match bytes.split_first() {
        Some((&ADDRESS_PREFIX, rest)) => rest.try_into().map_err(|_| Forbid::ParseFailed),
        _ => Err(Forbid::ParseFailed),
    }
}

fn put_address(out: &mut Vec<u8>, number: u64, address: &[u8; 20]) {
    let mut bytes = vec![ADDRESS_PREFIX];
    bytes.extend_from_slice(address);
    put_bytes(out, number, &bytes);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub address: [u8; 20],
    pub weight: u64,
}

impl Key {
    fn decode(bytes: &[u8]) -> Result<Self, Forbid> {
        let fields = Fields::strict(bytes, &[1, 2], &[])?;
        Ok(Self {
            address: address(fields.bytes(1)?)?,
            weight: fields.int(2)?,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_address(&mut out, 1, &self.address);
        put_int(&mut out, 2, self.weight);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permission {
    pub kind: u64,
    pub id: u64,
    pub name: Vec<u8>,
    pub threshold: u64,
    pub parent_id: u64,
    pub operations: Vec<u8>,
    pub keys: Vec<Key>,
}

impl Permission {
    fn decode(bytes: &[u8]) -> Result<Self, Forbid> {
        let fields = Fields::strict(bytes, &[1, 2, 3, 4, 5, 6, 7], &[7])?;
        Ok(Self {
            kind: fields.int(1)?,
            id: fields.int(2)?,
            name: fields.bytes(3)?.to_vec(),
            threshold: fields.int(4)?,
            parent_id: fields.int(5)?,
            operations: fields.bytes(6)?.to_vec(),
            keys: fields.list(7)?.into_iter().map(Key::decode).collect::<Result<_, _>>()?,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_int(&mut out, 1, self.kind);
        put_int(&mut out, 2, self.id);
        put_bytes(&mut out, 3, &self.name);
        put_int(&mut out, 4, self.threshold);
        put_int(&mut out, 5, self.parent_id);
        put_bytes(&mut out, 6, &self.operations);
        for key in &self.keys {
            put_bytes(&mut out, 7, &key.encode());
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Transfer {
        owner: [u8; 20],
        to: [u8; 20],
        amount: u64,
    },
    Trigger {
        owner: [u8; 20],
        contract: [u8; 20],
        call_value: u64,
        data: Vec<u8>,
    },
    PermissionUpdate {
        owner: [u8; 20],
        new_owner: Permission,
        witness: Option<Permission>,
        actives: Vec<Permission>,
    },
    /// 其他系统合约.只确认发起方,参数原样保留.
    Other {
        kind: u64,
        owner: [u8; 20],
        value: Vec<u8>,
    },
}

impl Call {
    fn decode(kind: u64, owner_field: u64, value: &[u8]) -> Result<Self, Forbid> {
        match kind {
            TRANSFER_CONTRACT => {
                let fields = Fields::strict(value, &[1, 2, 3], &[])?;
                Ok(Self::Transfer {
                    owner: address(fields.bytes(1)?)?,
                    to: address(fields.bytes(2)?)?,
                    amount: fields.int(3)?,
                })
            }
            // call_token_value 和 token_id 会随调用转走 TRC-10,这里不展示,所以当未知字段拒绝.
            TRIGGER_CONTRACT => {
                let fields = Fields::strict(value, &[1, 2, 3, 4], &[])?;
                Ok(Self::Trigger {
                    owner: address(fields.bytes(1)?)?,
                    contract: address(fields.bytes(2)?)?,
                    call_value: fields.int(3)?,
                    data: fields.bytes(4)?.to_vec(),
                })
            }
            PERMISSION_UPDATE_CONTRACT => {
                let fields = Fields::strict(value, &[1, 2, 3, 4], &[4])?;
                let new_owner = Permission::decode(fields.bytes(2)?)?;
                if new_owner.keys.is_empty() {
                    return Err(Forbid::ParseFailed);
                }
                let witness = match fields.bytes(3)? {
                    [] => None,
                    bytes => Some(Permission::decode(bytes)?),
                };
                Ok(Self::PermissionUpdate {
                    owner: address(fields.bytes(1)?)?,
                    new_owner,
                    witness,
                    actives: fields
                        .list(4)?
                        .into_iter()
                        .map(Permission::decode)
                        .collect::<Result<_, _>>()?,
                })
            }
            _ => {
                let fields = Fields::parse(value)?;
                let owners = fields.list(owner_field)?;
                let [owner] = owners.as_slice() else {
                    return Err(Forbid::ParseFailed);
                };
                Ok(Self::Other {
                    kind,
                    owner: address(owner)?,
                    value: value.to_vec(),
                })
            }
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Transfer { owner, to, amount } => {
                put_address(&mut out, 1, owner);
                put_address(&mut out, 2, to);
                put_int(&mut out, 3, *amount);
            }
            Self::Trigger {
                owner,
                contract,
                call_value,
                data,
            } => {
                put_address(&mut out, 1, owner);
                put_address(&mut out, 2, contract);
                put_int(&mut out, 3, *call_value);
                put_bytes(&mut out, 4, data);
            }
            Self::PermissionUpdate {
                owner,
                new_owner,
                witness,
                actives,
            } => {
                put_address(&mut out, 1, owner);
                put_bytes(&mut out, 2, &new_owner.encode());
                if let Some(witness) = witness {
                    put_bytes(&mut out, 3, &witness.encode());
                }
                for active in actives {
                    put_bytes(&mut out, 4, &active.encode());
                }
            }
            Self::Other { value, .. } => out.extend_from_slice(value),
        }
        out
    }

    pub fn kind(&self) -> u64 {
        match self {
            Self::Transfer { .. } => TRANSFER_CONTRACT,
            Self::Trigger { .. } => TRIGGER_CONTRACT,
            Self::PermissionUpdate { .. } => PERMISSION_UPDATE_CONTRACT,
            Self::Other { kind, .. } => *kind,
        }
    }

    pub fn owner(&self) -> &[u8; 20] {
        match self {
            Self::Transfer { owner, .. }
            | Self::Trigger { owner, .. }
            | Self::PermissionUpdate { owner, .. }
            | Self::Other { owner, .. } => owner,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawData {
    pub ref_block_bytes: [u8; 2],
    pub ref_block_hash: [u8; 8],
    pub expiration: u64,
    pub call: Call,
    pub timestamp: u64,
    pub fee_limit: u64,
}

impl RawData {
    /// `contract` 只有一个时才不重复,所以多个 contract 在这里就被拒绝.
    /// memo,auths,scripts 和 Permission_id 都不展示,当未知字段拒绝.
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        let fields = Fields::strict(payload, &[1, 4, 8, 11, 14, 18], &[])?;
        let contract = Fields::strict(fields.bytes(11)?, &[1, 2], &[])?;
        let kind = contract.int(1)?;
        let any = Fields::strict(contract.bytes(2)?, &[1, 2], &[])?;
        let (name, owner_field) = contract_info(kind).ok_or(Forbid::Unsupported)?;
        if any.bytes(1)? != format!("{TYPE_URL_PREFIX}{name}").as_bytes() {
            return Err(Forbid::ParseFailed);
        }
        let tx = Self {
            ref_block_bytes: fields.bytes(1)?.try_into().map_err(|_| Forbid::ParseFailed)?,
            ref_block_hash: fields.bytes(4)?.try_into().map_err(|_| Forbid::ParseFailed)?,
            expiration: fields.int(8)?,
            call: Call::decode(kind, owner_field, any.bytes(2)?)?,
            timestamp: fields.int(14)?,
            fee_limit: fields.int(18)?,
        };
        if tx.expiration <= tx.timestamp || tx.expiration - tx.timestamp > MAX_VALIDITY_MS {
            return Err(Forbid::ParseFailed);
        }
        if tx.encode() != payload {
            return Err(Forbid::ParseFailed);
        }
        Ok(tx)
    }

    pub fn encode(&self) -> Vec<u8> {
        let name = contract_info(self.call.kind()).map_or("", |(name, _)| name);
        let mut any = Vec::new();
        put_bytes(&mut any, 1, format!("{TYPE_URL_PREFIX}{name}").as_bytes());
        put_bytes(&mut any, 2, &self.call.encode());
        let mut contract = Vec::new();
        put_int(&mut contract, 1, self.call.kind());
        put_bytes(&mut contract, 2, &any);
        let mut out = Vec::new();
        put_bytes(&mut out, 1, &self.ref_block_bytes);
        put_bytes(&mut out, 4, &self.ref_block_hash);
        put_int(&mut out, 8, self.expiration);
        put_bytes(&mut out, 11, &contract);
        put_int(&mut out, 14, self.timestamp);
        put_int(&mut out, 18, self.fee_limit);
        out
    }
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
    rows.extend(permission.keys.iter().map(|key| {
        Row::mono(
            &format!("{label}地址"),
            format!("{} 权重 {}", tron_text(&key.address), key.weight),
        )
    }));
    rows
}

/// A6d 页面.除了第一个地址,其他地址同样能动用资金,所以全部列出.
fn permission_change(
    owner: &[u8; 20],
    new_owner: &Permission,
    witness: Option<&Permission>,
    actives: &[Permission],
) -> (abi::Classified, bool) {
    let controller = &new_owner.keys[0].address;
    let mut rows = vec![
        Row::mono("当前控制方", tron_text(owner)),
        Row::mono("新控制方", tron_text(controller)),
    ];
    rows.extend(permission_rows("owner 权限", new_owner));
    if let Some(witness) = witness {
        rows.extend(permission_rows("witness 权限", witness));
    }
    for active in actives {
        rows.extend(permission_rows("active 权限", active));
    }
    let shared = std::iter::once(new_owner)
        .chain(actives)
        .flat_map(|p| &p.keys)
        .any(|key| key.address != *controller);
    let classified = abi::Classified {
        risk: Risk::PermissionChange,
        title: "账户权限更新".to_string(),
        amount: None,
        counterparty_label: Some("新控制方".into()),
        counterparty: Some(tron_text(controller)),
        rows,
        alerts: Vec::new(),
        inner: Vec::new(),
    };
    (classified, shared)
}

fn other_contract(kind: u64, name: &str, value: &[u8]) -> Result<abi::Classified, Forbid> {
    let mut rows = vec![Row::mono("合约类型编号", kind.to_string())];
    for (number, field) in Fields::parse(value)?.0 {
        let text = match field {
            Value::Varint(value) => value.to_string(),
            Value::Bytes(bytes) => bytes.to_lower_hex_string(),
        };
        rows.push(Row::mono(&format!("字段 {number}"), text));
    }
    Ok(abi::Classified {
        risk: Risk::Unknown,
        title: name.to_string(),
        amount: None,
        counterparty_label: None,
        counterparty: None,
        rows,
        alerts: Vec::new(),
        inner: Vec::new(),
    })
}

/// `raw_data` 不是 ABI 编码,方法选择器和疑似地址这些提示不适用.
fn plain_lines(bytes: &[u8]) -> Vec<RawLine> {
    let mut lines = raw_lines(bytes);
    lines.iter_mut().for_each(|line| line.note = None);
    lines
}

pub fn review(network: Network, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = signer_path(request, network)?;
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::MessageSigning);
    }
    let key = ctx.keys.secp256k1(&path)?;
    let payer = eth_address_bytes(&key).map_err(|_| Forbid::AddressMismatch)?;
    let tx = RawData::decode(&request.payload)?;
    if *tx.call.owner() != payer {
        return Err(Forbid::AddressMismatch);
    }

    let native = ctx.manifest.native(network.id()).ok_or(Forbid::UnknownNetwork)?;
    let trx = |sun: u64| format!("{} {}", format_units(&sun.to_string(), native.decimals), native.symbol);
    let style = Style {
        network: network.id(),
        manifest: ctx.manifest,
        format: &tron_text,
    };
    let contract_name = contract_info(tx.call.kind()).map_or("", |(name, _)| name);
    let mut builder = ReviewBuilder::default();

    let classified = match &tx.call {
        Call::Transfer { to, amount, .. } => abi::Classified {
            risk: Risk::Transfer,
            title: Risk::Transfer.label().to_string(),
            amount: Some(AmountView {
                text: format_units(&amount.to_string(), native.decimals),
                symbol: Some(native.symbol.clone()),
                asset: Some(native.key()),
                unlimited: false,
            }),
            counterparty_label: Some("收款地址".into()),
            counterparty: Some(tron_text(to)),
            alerts: Vec::new(),
            inner: Vec::new(),
            rows: Vec::new(),
        },
        Call::Trigger {
            contract,
            call_value,
            data,
            ..
        } => abi::classify(&style, contract, data, *call_value != 0),
        Call::PermissionUpdate {
            owner,
            new_owner,
            witness,
            actives,
        } => {
            let (classified, shared) = permission_change(owner, new_owner, witness.as_ref(), actives);
            if shared {
                builder.alerts.push("新权限里还有其他地址,请逐个核对".to_string());
            }
            classified
        }
        Call::Other { kind, value, .. } => other_contract(*kind, contract_name, value)?,
    };

    builder.amount = classified.amount.clone();
    builder.counterparty_label = classified.counterparty_label.clone();
    builder.counterparty = classified.counterparty.clone();
    builder.actions.extend(classified.actions());
    builder.alerts.extend(classified.alerts.iter().cloned());

    if tx.fee_limit as u128 > policy::fee_ceiling(network) {
        builder.alerts.push("手续费上限明显偏高".to_string());
    }

    builder.rows.push(Row::text("网络", network.label()));
    builder.rows.push(Row::mono("付款账户", tron_text(&payer)));
    builder.rows.push(Row::mono("派生路径", path.to_string()));
    builder.rows.push(Row::text("过期时间", utc_time(tx.expiration)));
    builder.rows.push(Row::text("创建时间", utc_time(tx.timestamp)));
    builder.rows.push(Row::text("交易类型", contract_name));
    if let Call::Trigger { call_value, .. } = &tx.call
        && *call_value != 0
    {
        builder.rows.push(Row::text("附带金额", trx(*call_value)));
        // 顶部的金额是合约方法的参数,附带的 TRX 不在里面.
        builder.alerts.push(format!("这笔调用附带 {}", trx(*call_value)));
    }
    builder.rows.push(Row::text("fee_limit", trx(tx.fee_limit)));
    builder.raw = match &tx.call {
        Call::Trigger { data, .. } => raw_lines(data),
        _ => plain_lines(&request.payload),
    };
    Ok(builder.finish(network.id()))
}

pub fn signing_hash(request: &SignRequest) -> Result<[u8; 32], Forbid> {
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::MessageSigning);
    }
    Ok(sha256(&RawData::decode(&request.payload)?.encode()))
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

fn native_amount(text: &str) -> Result<u64, Error> {
    let bytes = be_from_decimal(text)?;
    if bytes.len() > 8 {
        return Err(Error::InvalidParams);
    }
    Ok(bytes.iter().fold(0u64, |acc, b| acc << 8 | *b as u64))
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

/// 构建转账,返回 `raw_data` 的字节.
pub fn build_transfer(params: &TransferParams) -> Result<Vec<u8>, Error> {
    if params.network.chain() != Chain::Tron {
        return Err(Error::InvalidParams);
    }
    let owner = parse_tron_address(params.from)?;
    let to = parse_tron_address(params.to)?;
    let (call, fee_limit) = match params.contract {
        None => (
            Call::Transfer {
                owner,
                to,
                amount: native_amount(params.amount)?,
            },
            0,
        ),
        Some(contract) => (
            Call::Trigger {
                owner,
                contract: parse_tron_address(contract)?,
                call_value: 0,
                data: transfer_data(&to, params.amount)?,
            },
            params.fee_limit,
        ),
    };
    let raw = RawData {
        ref_block_bytes: params.ref_block_bytes,
        ref_block_hash: params.ref_block_hash,
        expiration: params.expiration_ms,
        call,
        timestamp: params.timestamp_ms,
        fee_limit,
    }
    .encode();
    // 冷钱包不接受的交易这里就不构建.
    RawData::decode(&raw).map_err(|_| Error::InvalidParams)?;
    Ok(raw)
}

/// `signer` 是付款账户的压缩公钥,必须和 `raw_data` 里的发起方一致.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 65] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    let tx = RawData::decode(&request.payload).map_err(|_| Error::InvalidParams)?;
    if eth_address_bytes(signer).map_err(|_| Error::InvalidKey)? != *tx.call.owner() {
        return Err(Error::BadSignature);
    }
    let raw_data = tx.encode();
    let hash = sha256(&raw_data);
    check_signature(&hash, signature, signer)?;
    let mut raw = Vec::new();
    put_bytes(&mut raw, 1, &raw_data);
    put_bytes(&mut raw, 2, signature);
    Ok(Verified {
        raw,
        hash: hash.to_lower_hex_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: [u8; 20] = [0x11; 20];
    const PEER: [u8; 20] = [0x22; 20];
    const USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const NOW: u64 = 1_800_000_000_000;

    fn raw(call: Call) -> RawData {
        RawData {
            ref_block_bytes: [0xab, 0xcd],
            ref_block_hash: [1, 2, 3, 4, 5, 6, 7, 8],
            expiration: NOW + 1_800_000,
            call,
            timestamp: NOW,
            fee_limit: 0,
        }
    }

    fn transfer() -> RawData {
        raw(Call::Transfer {
            owner: OWNER,
            to: PEER,
            amount: 5,
        })
    }

    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, number, value);
        out
    }

    fn int_field(number: u64, value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put_int(&mut out, number, value);
        out
    }

    fn contract_bytes(kind: u64, name: &str, value: &[u8]) -> Vec<u8> {
        let url = format!("{TYPE_URL_PREFIX}{name}");
        let any = [field(1, url.as_bytes()), field(2, value)].concat();
        [int_field(1, kind), field(2, &any)].concat()
    }

    fn with_contract(contract: &[u8]) -> Vec<u8> {
        let base = transfer();
        [
            field(1, &base.ref_block_bytes),
            field(4, &base.ref_block_hash),
            int_field(8, base.expiration),
            field(11, contract),
            int_field(14, base.timestamp),
        ]
        .concat()
    }

    fn prefixed(address: &[u8; 20]) -> Vec<u8> {
        [&[ADDRESS_PREFIX], address.as_slice()].concat()
    }

    #[test]
    fn varint_is_strict() {
        for bad in [
            vec![0x80],                           // 截断
            vec![0x80, 0x00],                     // 非最短
            vec![0xff; 11],                       // 过长
            [vec![0xff; 9], vec![0x02]].concat(), // 超过 64 位
        ] {
            assert_eq!(read_varint(&mut bad.as_slice()), Err(Forbid::ParseFailed), "{bad:?}");
        }
        let mut out = Vec::new();
        put_varint(&mut out, u64::MAX);
        assert_eq!(read_varint(&mut out.as_slice()), Ok(u64::MAX));
    }

    #[test]
    fn round_trip() {
        let tx = transfer();
        assert_eq!(RawData::decode(&tx.encode()).unwrap(), tx);
    }

    #[test]
    fn length_past_the_end_is_refused() {
        let mut bytes = transfer().encode();
        bytes.truncate(bytes.len() - 1);
        assert_eq!(RawData::decode(&bytes), Err(Forbid::ParseFailed));
        let huge = [vec![0x0a], vec![0xff; 9], vec![0x01]].concat();
        assert_eq!(RawData::decode(&huge), Err(Forbid::ParseFailed));
    }

    #[test]
    fn unknown_field_is_refused_at_every_level() {
        let top = [transfer().encode(), int_field(99, 1)].concat();
        assert_eq!(RawData::decode(&top), Err(Forbid::ParseFailed));

        // memo,auths,ref_block_num,scripts 都是协议字段,但冷钱包不展示,同样拒绝.
        for number in [3, 9, 10, 12] {
            let tx = [transfer().encode(), field(number, b"x")].concat();
            assert_eq!(RawData::decode(&tx), Err(Forbid::ParseFailed), "{number}");
        }

        let call = Call::Transfer {
            owner: OWNER,
            to: PEER,
            amount: 5,
        }
        .encode();
        let inner = [call, int_field(9, 1)].concat();
        let contract = contract_bytes(1, "TransferContract", &inner);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn permission_id_is_refused() {
        let call = Call::Transfer {
            owner: OWNER,
            to: PEER,
            amount: 5,
        }
        .encode();
        let contract = [contract_bytes(1, "TransferContract", &call), int_field(5, 2)].concat();
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn call_token_value_is_refused() {
        let call = Call::Trigger {
            owner: OWNER,
            contract: PEER,
            call_value: 0,
            data: vec![1],
        }
        .encode();
        let inner = [call, int_field(5, 1)].concat();
        let contract = contract_bytes(31, "TriggerSmartContract", &inner);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn duplicate_field_is_refused() {
        let mut bytes = field(1, &[0xab, 0xcd]);
        bytes.extend_from_slice(&transfer().encode());
        assert_eq!(RawData::decode(&bytes), Err(Forbid::ParseFailed));

        // 字段 3 写两次:有的解析器取第一个,有的取最后一个.
        let call = Call::Transfer {
            owner: OWNER,
            to: PEER,
            amount: 5,
        }
        .encode();
        let inner = [call, int_field(3, 9)].concat();
        let contract = contract_bytes(1, "TransferContract", &inner);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn field_order_and_explicit_defaults_are_refused() {
        let tx = transfer();
        let contract = {
            let bytes = tx.encode();
            let fields = Fields::parse(&bytes).unwrap();
            fields.bytes(11).unwrap().to_vec()
        };
        // 时间戳排到 contract 前面.
        let swapped = [
            field(1, &tx.ref_block_bytes),
            field(4, &tx.ref_block_hash),
            int_field(8, tx.expiration),
            int_field(14, tx.timestamp),
            field(11, &contract),
        ]
        .concat();
        assert_eq!(RawData::decode(&swapped), Err(Forbid::ParseFailed));

        // fee_limit 显式写 0,解析结果一样但字节不同.
        let explicit = [tx.encode(), vec![18 << 3, 0]].concat();
        assert_eq!(RawData::decode(&explicit), Err(Forbid::ParseFailed));
    }

    #[test]
    fn negative_integers_are_refused() {
        let mut negative = field(1, &prefixed(&OWNER));
        negative.extend_from_slice(&field(2, &prefixed(&PEER)));
        put_varint(&mut negative, 3 << 3);
        put_varint(&mut negative, -1i64 as u64);
        let contract = contract_bytes(1, "TransferContract", &negative);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn exactly_one_contract() {
        let bytes = transfer().encode();
        let contract = Fields::parse(&bytes).unwrap().bytes(11).unwrap().to_vec();
        let base = transfer();
        let two = [
            field(1, &base.ref_block_bytes),
            field(4, &base.ref_block_hash),
            int_field(8, base.expiration),
            field(11, &contract),
            field(11, &contract),
            int_field(14, base.timestamp),
        ]
        .concat();
        assert_eq!(RawData::decode(&two), Err(Forbid::ParseFailed));

        let none = [
            field(1, &base.ref_block_bytes),
            field(4, &base.ref_block_hash),
            int_field(8, base.expiration),
            int_field(14, base.timestamp),
        ]
        .concat();
        assert_eq!(RawData::decode(&none), Err(Forbid::ParseFailed));
    }

    #[test]
    fn type_url_must_match_the_contract_type() {
        let call = Call::Transfer {
            owner: OWNER,
            to: PEER,
            amount: 5,
        }
        .encode();
        let contract = contract_bytes(1, "TriggerSmartContract", &call);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn contract_type_without_a_known_owner_is_refused() {
        let value = field(1, &prefixed(&OWNER));
        for (kind, name) in [
            (99, "Whatever"),
            (51, "ShieldedTransferContract"),
            (20, "CustomContract"),
        ] {
            let contract = contract_bytes(kind, name, &value);
            assert_eq!(
                RawData::decode(&with_contract(&contract)),
                Err(Forbid::Unsupported),
                "{kind}"
            );
        }
    }

    #[test]
    fn owner_field_depends_on_the_contract_type() {
        // TransferAssetContract 的字段 1 是资产名,发起方在字段 2.
        let value = [
            field(1, b"1002000"),
            field(2, &prefixed(&OWNER)),
            field(3, &prefixed(&PEER)),
        ]
        .concat();
        let contract = contract_bytes(2, "TransferAssetContract", &value);
        let tx = RawData::decode(&with_contract(&contract)).unwrap();
        assert_eq!(*tx.call.owner(), OWNER);

        // 同名字段出现两次时不知道节点取哪一个.
        let twice = [value.clone(), field(2, &prefixed(&PEER))].concat();
        let contract = contract_bytes(2, "TransferAssetContract", &twice);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));

        // AccountCreateContract 的类型是 0,proto3 不写.
        let contract = contract_bytes(0, "AccountCreateContract", &field(1, &prefixed(&OWNER)));
        let tx = RawData::decode(&with_contract(&contract)).unwrap();
        assert_eq!(tx.call.kind(), 0);
    }

    #[test]
    fn oversized_tags_are_refused() {
        // Java 的 protobuf 只读低 32 位,这个标签在节点上会变成字段 2,也就是第二个发起方.
        let value = [field(1, b"1002000"), field(2, &prefixed(&OWNER))].concat();
        let mut hidden = value.clone();
        put_varint(&mut hidden, 1 << 32 | 2 << 3 | 2);
        put_varint(&mut hidden, 21);
        hidden.extend_from_slice(&prefixed(&PEER));
        let contract = contract_bytes(2, "TransferAssetContract", &hidden);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn address_needs_the_network_prefix() {
        let mut value = vec![0xa0];
        value.extend_from_slice(&PEER);
        let bad = [field(1, &prefixed(&OWNER)), field(2, &value), int_field(3, 1)].concat();
        let contract = contract_bytes(1, "TransferContract", &bad);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));

        let short = [field(1, &prefixed(&OWNER)), field(2, &[0x41, 1, 2]), int_field(3, 1)].concat();
        let contract = contract_bytes(1, "TransferContract", &short);
        assert_eq!(RawData::decode(&with_contract(&contract)), Err(Forbid::ParseFailed));
    }

    #[test]
    fn ref_block_lengths_are_fixed() {
        let tx = transfer();
        let bytes = tx.encode();
        let contract = Fields::parse(&bytes).unwrap().bytes(11).unwrap().to_vec();
        let bad = [
            field(1, &[1, 2, 3]),
            field(4, &tx.ref_block_hash),
            int_field(8, tx.expiration),
            field(11, &contract),
            int_field(14, tx.timestamp),
        ]
        .concat();
        assert_eq!(RawData::decode(&bad), Err(Forbid::ParseFailed));
    }

    #[test]
    fn validity_window() {
        let at = |expiration: u64| {
            let mut tx = transfer();
            tx.expiration = expiration;
            RawData::decode(&tx.encode())
        };
        assert_eq!(at(NOW), Err(Forbid::ParseFailed));
        assert_eq!(at(NOW - 1), Err(Forbid::ParseFailed));
        assert_eq!(at(NOW + MAX_VALIDITY_MS + 1), Err(Forbid::ParseFailed));
        assert!(at(NOW + 1).is_ok());
        assert!(at(NOW + MAX_VALIDITY_MS).is_ok());
    }

    #[test]
    fn build_refuses_what_review_would_refuse() {
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

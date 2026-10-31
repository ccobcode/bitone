//! EVM 交易：严格解析、分类、构建和核验。
//!
//! 解析后必须能重新编码回完全相同的字节，这样冷钱包展示的内容和实际签名的内容不可能不一致。

use crate::address::{eth_address_bytes, evm_text, parse_evm_address};
use crate::path::DerivationPath;
use crate::policy;
use crate::request::{KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
use crate::review::{ActionView, AmountView, Forbid, Review, ReviewBuilder, Risk, Row};
use crate::tx::abi::{self, Style};
use crate::tx::{ReviewContext, Verified, check_signature, raw_lines, signer_path};
use crate::units::{decimal_from_be, format_units};
use crate::{Error, Network};
use alloy_rlp::{Encodable, Header};
use serde_json::Value;
use sha3::{Digest, Keccak256};

mod typed;

pub use typed::typed_data_hash;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorization {
    pub chain_id: u64,
    pub address: [u8; 20],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fees {
    /// 类型 0 和 1。
    GasPrice(u128),
    /// 类型 2 和 4。
    Dynamic { max_priority: u128, max_fee: u128 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsignedTx {
    pub tx_type: u8,
    pub chain_id: u64,
    pub nonce: u64,
    pub fees: Fees,
    pub gas_limit: u64,
    pub to: Option<[u8; 20]>,
    pub value: Vec<u8>,
    pub data: Vec<u8>,
    /// 访问列表的原始编码，含列表头。
    pub access_list: Vec<u8>,
    pub access_list_len: usize,
    /// 授权列表的原始编码，含列表头。只有类型 4 有。
    pub authorization_list: Vec<u8>,
    pub authorizations: Vec<Authorization>,
}

fn item<'a>(buf: &mut &'a [u8]) -> Result<&'a [u8], Forbid> {
    let header = Header::decode(buf).map_err(|_| Forbid::ParseFailed)?;
    if header.list || header.payload_length > buf.len() {
        return Err(Forbid::ParseFailed);
    }
    let (value, rest) = buf.split_at(header.payload_length);
    *buf = rest;
    Ok(value)
}

fn raw_list<'a>(buf: &mut &'a [u8]) -> Result<(&'a [u8], &'a [u8]), Forbid> {
    let start = *buf;
    let header = Header::decode(buf).map_err(|_| Forbid::ParseFailed)?;
    if !header.list || header.payload_length > buf.len() {
        return Err(Forbid::ParseFailed);
    }
    let (body, rest) = buf.split_at(header.payload_length);
    *buf = rest;
    Ok((&start[..start.len() - rest.len()], body))
}

fn uint(buf: &mut &[u8]) -> Result<u128, Forbid> {
    let value = item(buf)?;
    if value.len() > 16 || value.first() == Some(&0) {
        return Err(Forbid::ParseFailed);
    }
    Ok(value.iter().fold(0u128, |acc, b| acc << 8 | *b as u128))
}

fn uint64(buf: &mut &[u8]) -> Result<u64, Forbid> {
    u64::try_from(uint(buf)?).map_err(|_| Forbid::ParseFailed)
}

fn big(buf: &mut &[u8]) -> Result<Vec<u8>, Forbid> {
    let value = item(buf)?;
    if value.len() > 32 || value.first() == Some(&0) {
        return Err(Forbid::ParseFailed);
    }
    Ok(value.to_vec())
}

fn destination(buf: &mut &[u8]) -> Result<Option<[u8; 20]>, Forbid> {
    let value = item(buf)?;
    match value.len() {
        0 => Ok(None),
        20 => Ok(Some(value.try_into().expect("20 字节"))),
        _ => Err(Forbid::ParseFailed),
    }
}

fn count_items(mut body: &[u8]) -> Result<usize, Forbid> {
    let mut count = 0;
    while !body.is_empty() {
        raw_list(&mut body)?;
        count += 1;
    }
    Ok(count)
}

fn authorizations(mut body: &[u8]) -> Result<Vec<Authorization>, Forbid> {
    let mut list = Vec::new();
    while !body.is_empty() {
        let (_, mut tuple) = raw_list(&mut body)?;
        let chain_id = uint64(&mut tuple)?;
        let address = destination(&mut tuple)?.ok_or(Forbid::ParseFailed)?;
        list.push(Authorization { chain_id, address });
    }
    Ok(list)
}

impl UnsignedTx {
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        let first = *payload.first().ok_or(Forbid::ParseFailed)?;
        let tx = match first {
            0x01 | 0x02 | 0x04 => Self::decode_typed(first, &payload[1..])?,
            0xc0..=0xff => Self::decode_legacy(payload)?,
            _ => return Err(Forbid::Unsupported),
        };
        if tx.encode() != payload {
            return Err(Forbid::ParseFailed);
        }
        Ok(tx)
    }

    fn decode_typed(tx_type: u8, mut bytes: &[u8]) -> Result<Self, Forbid> {
        let (_, mut body) = raw_list(&mut bytes)?;
        if !bytes.is_empty() {
            return Err(Forbid::ParseFailed);
        }
        let chain_id = uint64(&mut body)?;
        let nonce = uint64(&mut body)?;
        let fees = if tx_type == 0x01 {
            Fees::GasPrice(uint(&mut body)?)
        } else {
            let max_priority = uint(&mut body)?;
            Fees::Dynamic {
                max_priority,
                max_fee: uint(&mut body)?,
            }
        };
        let gas_limit = uint64(&mut body)?;
        let to = destination(&mut body)?;
        let value = big(&mut body)?;
        let data = item(&mut body)?.to_vec();
        let (access_list, access_body) = raw_list(&mut body)?;
        let (authorization_list, authorizations) = if tx_type == 0x04 {
            let (raw, inner) = raw_list(&mut body)?;
            (raw.to_vec(), authorizations(inner)?)
        } else {
            (Vec::new(), Vec::new())
        };
        if !body.is_empty() {
            return Err(Forbid::ParseFailed);
        }
        Ok(Self {
            tx_type,
            chain_id,
            nonce,
            fees,
            gas_limit,
            to,
            value,
            data,
            access_list: access_list.to_vec(),
            access_list_len: count_items(access_body)?,
            authorization_list,
            authorizations,
        })
    }

    fn decode_legacy(mut bytes: &[u8]) -> Result<Self, Forbid> {
        let (_, mut body) = raw_list(&mut bytes)?;
        if !bytes.is_empty() {
            return Err(Forbid::ParseFailed);
        }
        let nonce = uint64(&mut body)?;
        let gas_price = uint(&mut body)?;
        let gas_limit = uint64(&mut body)?;
        let to = destination(&mut body)?;
        let value = big(&mut body)?;
        let data = item(&mut body)?.to_vec();
        if body.is_empty() {
            return Err(Forbid::LegacyWithoutChainId);
        }
        let chain_id = uint64(&mut body)?;
        if uint(&mut body)? != 0 || uint(&mut body)? != 0 || !body.is_empty() {
            return Err(Forbid::ParseFailed);
        }
        Ok(Self {
            tx_type: 0,
            chain_id,
            nonce,
            fees: Fees::GasPrice(gas_price),
            gas_limit,
            to,
            value,
            data,
            access_list: Vec::new(),
            access_list_len: 0,
            authorization_list: Vec::new(),
            authorizations: Vec::new(),
        })
    }

    fn fields(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.tx_type != 0 {
            self.chain_id.encode(&mut out);
        }
        self.nonce.encode(&mut out);
        match self.fees {
            Fees::GasPrice(price) => price.encode(&mut out),
            Fees::Dynamic { max_priority, max_fee } => {
                max_priority.encode(&mut out);
                max_fee.encode(&mut out);
            }
        }
        self.gas_limit.encode(&mut out);
        match &self.to {
            Some(address) => address.as_slice().encode(&mut out),
            None => (&[][..] as &[u8]).encode(&mut out),
        }
        self.value.as_slice().encode(&mut out);
        self.data.as_slice().encode(&mut out);
        if self.tx_type != 0 {
            out.extend_from_slice(&self.access_list);
            out.extend_from_slice(&self.authorization_list);
        }
        out
    }

    fn wrap(&self, fields: Vec<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        if self.tx_type != 0 {
            out.push(self.tx_type);
        }
        Header {
            list: true,
            payload_length: fields.len(),
        }
        .encode(&mut out);
        out.extend_from_slice(&fields);
        out
    }

    /// 未签名编码，也就是签名哈希的原文。
    pub fn encode(&self) -> Vec<u8> {
        let mut fields = self.fields();
        if self.tx_type == 0 {
            self.chain_id.encode(&mut fields);
            0u8.encode(&mut fields);
            0u8.encode(&mut fields);
        }
        self.wrap(fields)
    }

    /// 装上签名，得到可以广播的原始交易。
    pub fn assemble(&self, signature: &[u8; 65]) -> Vec<u8> {
        let mut fields = self.fields();
        let parity = signature[64] as u64;
        let v = if self.tx_type == 0 {
            self.chain_id * 2 + 35 + parity
        } else {
            parity
        };
        v.encode(&mut fields);
        trimmed(&signature[..32]).encode(&mut fields);
        trimmed(&signature[32..64]).encode(&mut fields);
        self.wrap(fields)
    }

    /// 最高手续费，单位是最小单位。
    pub fn fee_cap(&self) -> u128 {
        let price = match self.fees {
            Fees::GasPrice(price) => price,
            Fees::Dynamic { max_fee, .. } => max_fee,
        };
        price.saturating_mul(self.gas_limit as u128)
    }
}

fn trimmed(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    &bytes[start..]
}

pub fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

fn signer_path_row(network: Network, chain_id: u64, payer: &str, path: &DerivationPath) -> [Row; 4] {
    [
        Row::text("网络", network.label()),
        Row::text("Chain ID", chain_id.to_string()),
        Row::mono("付款账户", payer),
        Row::mono("派生路径", path.to_string()),
    ]
}

fn tx_type_label(tx_type: u8) -> String {
    match tx_type {
        0 => "0x00 旧式".to_string(),
        1 => "0x01 访问列表".to_string(),
        2 => "0x02 EIP-1559".to_string(),
        other => format!("0x{other:02x} EIP-7702"),
    }
}

fn gwei(value: u128) -> String {
    format!("{} Gwei", format_units(&value.to_string(), 9))
}

pub fn review(network: Network, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = signer_path(request, network)?;
    let key = ctx.keys.secp256k1(&path)?;
    let payer = evm_text(&eth_address_bytes(&key).map_err(|_| Forbid::AddressMismatch)?);
    match request.kind {
        KIND_TRANSACTION => review_tx(network, request, ctx, &payer, &path),
        KIND_TYPED_DATA => typed::review(network, request, ctx, &payer, &path),
        _ => Err(Forbid::RawHashSigning),
    }
}

fn review_tx(
    network: Network,
    request: &SignRequest,
    ctx: &ReviewContext,
    payer: &str,
    path: &DerivationPath,
) -> Result<Review, Forbid> {
    let tx = UnsignedTx::decode(&request.payload)?;
    let chain_id = network.evm_chain_id().ok_or(Forbid::UnknownNetwork)?;
    if tx.chain_id != chain_id {
        return Err(Forbid::ChainIdMismatch);
    }
    for auth in &tx.authorizations {
        if auth.chain_id == 0 {
            return Err(Forbid::UniversalAuthorization);
        }
        if auth.chain_id != chain_id {
            return Err(Forbid::ChainIdMismatch);
        }
    }

    let native = ctx.manifest.native(network.id()).ok_or(Forbid::UnknownNetwork)?;
    let value_text = format_units(&decimal_from_be(&tx.value), native.decimals);
    let has_value = !tx.value.is_empty();
    let style = Style {
        network: network.id(),
        manifest: ctx.manifest,
        format: &evm_text,
    };
    let mut builder = ReviewBuilder::default();

    let native_amount = || AmountView {
        text: value_text.clone(),
        symbol: Some(native.symbol.clone()),
        asset: Some(native.key()),
        unlimited: false,
    };

    let classified = match (&tx.to, tx.data.is_empty()) {
        (Some(to), true) => abi::Classified {
            risk: Risk::Transfer,
            title: Risk::Transfer.label().to_string(),
            amount: Some(native_amount()),
            counterparty_label: Some("收款地址".into()),
            counterparty: Some(evm_text(to)),
            alerts: Vec::new(),
            inner: Vec::new(),
            rows: Vec::new(),
        },
        (Some(to), false) => abi::classify(&style, to, &tx.data, has_value),
        (None, _) => abi::Classified {
            risk: Risk::Unknown,
            title: "创建合约".to_string(),
            amount: None,
            counterparty_label: None,
            counterparty: None,
            alerts: Vec::new(),
            inner: Vec::new(),
            rows: vec![Row::text("数据长度", format!("{} 字节", tx.data.len()))],
        },
    };

    builder.amount = classified.amount.clone();
    builder.counterparty_label = classified.counterparty_label.clone();
    builder.counterparty = classified.counterparty.clone();
    builder.alerts.extend(classified.alerts.iter().cloned());
    let mut actions = classified.actions();
    // 有账户委托时页面顶部是新控制方，所以这笔交易本身的金额和收款方要放进列表，否则用户看不到。
    if !tx.authorizations.is_empty() {
        actions[0] = classified.detailed();
    }
    builder.actions.extend(actions);

    for auth in &tx.authorizations {
        builder.actions.push(ActionView {
            risk: Risk::PermissionChange,
            title: "EIP-7702 账户委托".to_string(),
            rows: vec![
                Row::mono("当前控制方", payer),
                Row::mono("新控制方", evm_text(&auth.address)),
            ],
        });
        builder.counterparty_label = Some("新控制方".into());
        builder.counterparty = Some(evm_text(&auth.address));
    }

    let cap = tx.fee_cap();
    if cap > policy::fee_ceiling(network) {
        builder.alerts.push("手续费上限明显偏高".to_string());
    }
    let cap_text = format!("{} {}", format_units(&cap.to_string(), native.decimals), native.symbol);

    builder.rows.extend(signer_path_row(network, chain_id, payer, path));
    builder.rows.push(Row::text("Nonce", tx.nonce.to_string()));
    builder.rows.push(Row::text("交易类型", tx_type_label(tx.tx_type)));
    if has_value && !tx.data.is_empty() {
        builder
            .rows
            .push(Row::text("附带金额", format!("{value_text} {}", native.symbol)));
    }
    builder.rows.push(Row::text("Gas Limit", tx.gas_limit.to_string()));
    match tx.fees {
        Fees::GasPrice(price) => builder.rows.push(Row::text("Gas 单价", gwei(price))),
        Fees::Dynamic { max_priority, max_fee } => {
            builder.rows.push(Row::text("最高单价", gwei(max_fee)));
            builder.rows.push(Row::text("优先费", gwei(max_priority)));
        }
    }
    builder.rows.push(Row::text("手续费上限", cap_text));
    if tx.access_list_len > 0 {
        builder
            .rows
            .push(Row::text("访问列表", format!("{} 项", tx.access_list_len)));
    }
    builder.raw = raw_lines(&tx.data);
    Ok(builder.finish(network.id()))
}

/// 这个请求的签名哈希。
pub fn signing_hash(request: &SignRequest) -> Result<[u8; 32], Forbid> {
    match request.kind {
        KIND_TRANSACTION => Ok(keccak(&UnsignedTx::decode(&request.payload)?.encode())),
        KIND_TYPED_DATA => {
            let data: Value = serde_json::from_slice(&request.payload).map_err(|_| Forbid::ParseFailed)?;
            typed_data_hash(&data)
        }
        _ => Err(Forbid::RawHashSigning),
    }
}

pub struct TransferParams<'a> {
    pub network: Network,
    pub to: &'a str,
    /// 最小单位的十进制字符串。
    pub amount: &'a str,
    /// 代币合约。留空是原生币转账。
    pub contract: Option<&'a str>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee: u128,
    pub max_priority: u128,
}

/// 十进制字符串转大端字节，去掉前导 0。
pub fn be_from_decimal(text: &str) -> Result<Vec<u8>, Error> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidParams);
    }
    let mut bytes: Vec<u8> = vec![];
    for digit in text.bytes().map(|b| (b - b'0') as u32) {
        let mut carry = digit;
        for byte in bytes.iter_mut().rev() {
            let value = *byte as u32 * 10 + carry;
            *byte = value as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    if bytes.len() > 32 {
        return Err(Error::InvalidParams);
    }
    Ok(bytes)
}

/// ERC-20 的 transfer 调用数据。估算 Gas 时也要用。
pub fn transfer_data(recipient: &[u8; 20], amount: &[u8]) -> Vec<u8> {
    let mut data = vec![0xa9, 0x05, 0x9c, 0xbb];
    data.extend_from_slice(&[0; 12]);
    data.extend_from_slice(recipient);
    data.resize(data.len() + 32 - amount.len(), 0);
    data.extend_from_slice(amount);
    data
}

/// 构建 EIP-1559 转账。
pub fn build_transfer(params: &TransferParams) -> Result<UnsignedTx, Error> {
    let chain_id = params.network.evm_chain_id().ok_or(Error::InvalidParams)?;
    let recipient = parse_evm_address(params.to)?;
    let amount = be_from_decimal(params.amount)?;
    let (to, value, data) = match params.contract {
        None => (recipient, amount, Vec::new()),
        Some(contract) => (
            parse_evm_address(contract)?,
            Vec::new(),
            transfer_data(&recipient, &amount),
        ),
    };
    Ok(UnsignedTx {
        tx_type: 2,
        chain_id,
        nonce: params.nonce,
        fees: Fees::Dynamic {
            max_priority: params.max_priority,
            max_fee: params.max_fee,
        },
        gas_limit: params.gas_limit,
        to: Some(to),
        value,
        data,
        access_list: vec![0xc0],
        access_list_len: 0,
        authorization_list: Vec::new(),
        authorizations: Vec::new(),
    })
}

pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 65] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    let tx = UnsignedTx::decode(&request.payload).map_err(|_| Error::InvalidParams)?;
    check_signature(&keccak(&tx.encode()), signature, signer)?;
    let raw = tx.assemble(signature);
    let hash = keccak(&raw)
        .iter()
        .fold(String::from("0x"), |s, b| s + &format!("{b:02x}"));
    Ok(Verified { raw, hash })
}

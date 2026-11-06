//! 合约调用,EVM 和 TRON 共用.ABI 和展示来自描述文件,类别由策略按解码出的动作判定.
//!
//! 标准函数内置,任何合约都解码,用户才看得到被授权方和额度.清单外的合约不一定遵守标准,类别是无法识别.
//! 清单的描述文件只用于标准函数之外的选择器,所以描述文件不能改变授权和转账的类别.
//! 描述文件的函数解不开或重新编码不等于 `data` 时按没有描述处理.标准函数的数据不规范时合约照样执行,
//! 所以按旧版编译器的读法取参数,类别是无法识别.

use crate::Chain;
use crate::decode::{AddressText, Amount, Node, Source, Value, abi_node};
use crate::manifest::{DisplayField, DisplayFormat, Manifest};
use crate::policy::{self, CallFact, CallShape};
use crate::review::{AmountView, Party, Risk, field_label};
use crate::units::format_units;
use alloy_dyn_abi::{DynSolType, DynSolValue, JsonAbiExt, Specifier};
use alloy_json_abi::Function;
use alloy_primitives::{Selector, U160, U256};
use bitcoin::hex::DisplayHex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub(crate) const TRANSFER: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
pub(crate) const APPROVE: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// 嵌入调用最多展开的层数和条数,超过时按没有描述处理.
const MAX_DEPTH: usize = 3;
const MAX_INNER_CALLS: usize = 32;

/// ERC-7730 的容器路径,调用的目标合约.
const TARGET_PATH: &str = "@.to";
const TOKEN_AMOUNT: &str = "tokenAmount";
const ADDRESS_NAME: &str = "addressName";
const EMBEDDED_CALL: &str = "calldata";
/// 布尔值为真时对方能动用这个合约的全部资产,`bitone-calldata.json` 用它.
const ALL_ASSETS: &str = "allAssets";
const CALLS_ROW: &str = "calls";
const TRAILING_ROW: &str = "trailing_data";
const WORD: usize = 32;

/// 注册表文件和本仓库的补充,见 `schemas/lock.toml`.
const STANDARD_FILES: [&str; 2] = [
    include_str!("../../../../schemas/erc7730/calldata-erc20-tokens.json"),
    include_str!("../../../../schemas/erc7730/bitone-calldata.json"),
];

#[derive(Deserialize)]
struct RegistryDisplay {
    formats: BTreeMap<String, DisplayFormat>,
}

#[derive(Deserialize)]
struct RegistryFile {
    display: RegistryDisplay,
}

/// 一个函数和它的展示方式.
#[derive(Clone)]
struct Described {
    function: Function,
    format: DisplayFormat,
}

static STANDARD: OnceLock<Vec<Described>> = OnceLock::new();

fn standard() -> &'static [Described] {
    STANDARD.get_or_init(|| {
        let files = STANDARD_FILES
            .iter()
            .map(|text| serde_json::from_str::<RegistryFile>(text).expect("内置描述文件有效"));
        files
            .flat_map(|file| file.display.formats)
            .map(|(signature, format)| Described {
                function: Function::parse(&signature).expect("内置函数签名有效"),
                format,
            })
            .collect()
    })
}

/// 全部标准函数的规范签名,规则表要覆盖它们.
pub fn standard_signatures() -> Vec<String> {
    standard().iter().map(|entry| entry.function.signature()).collect()
}

/// 清单的描述文件.解析不了的条目等于没有.
fn manifest_entry(style: &Style, contract: &str, selector: Selector) -> Option<Described> {
    style
        .manifest
        .descriptors(style.network, contract)
        .find_map(|descriptor| {
            let function = Function::parse(&descriptor.signature).ok()?;
            (function.selector() == selector).then(|| Described {
                function,
                format: descriptor.format.clone(),
            })
        })
}

/// 官方解码器不检查窄类型的高位,带着高位重新编码也回到原样,所以要比较值的范围.
fn fits_its_type(value: &DynSolValue) -> bool {
    match value {
        DynSolValue::Uint(number, bits) => number.bit_len() <= *bits,
        DynSolValue::Int(number, bits) => {
            let shift = 256usize.saturating_sub(*bits);
            number.wrapping_shl(shift).asr(shift) == *number
        }
        DynSolValue::FixedBytes(word, size) => word.get(*size..).is_some_and(|rest| rest.iter().all(|byte| *byte == 0)),
        DynSolValue::Array(items) | DynSolValue::FixedArray(items) | DynSolValue::Tuple(items) => {
            items.iter().all(fits_its_type)
        }
        DynSolValue::CustomStruct { tuple, .. } => tuple.iter().all(fits_its_type),
        DynSolValue::Bool(_)
        | DynSolValue::Address(_)
        | DynSolValue::Function(_)
        | DynSolValue::Bytes(_)
        | DynSolValue::String(_) => true,
    }
}

/// 重新编码必须等于 `data`,不相等按没有描述处理.
fn decoded_args(function: &Function, args: &[u8]) -> Option<Vec<DynSolValue>> {
    let values = function.abi_decode_input(args).ok()?;
    let regular = values.iter().all(fits_its_type) && function.abi_encode_input_raw(&values).ok()? == args;
    regular.then_some(values)
}

/// 一个参数字,读到数据末尾之后的字节是 0.
fn loose_word(args: &[u8], index: usize) -> [u8; WORD] {
    let tail = args.get(index * WORD..).unwrap_or_default();
    let len = tail.len().min(WORD);
    let mut word = [0; WORD];
    word[..len].copy_from_slice(&tail[..len]);
    word
}

/// 旧版编译器读到的值:地址和短整数取低位,定长字节取高位,布尔值只要不是 0 就是真.
fn cleaned(ty: &DynSolType, mut word: [u8; WORD]) -> [u8; WORD] {
    let keep = match ty {
        DynSolType::Address => 20,
        DynSolType::Uint(bits) => bits / 8,
        DynSolType::Bool => {
            let on = word != [0; WORD];
            word = [0; WORD];
            word[WORD - 1] = u8::from(on);
            1
        }
        DynSolType::FixedBytes(size) => {
            word[*size..].fill(0);
            WORD
        }
        _ => WORD,
    };
    word[..WORD - keep].fill(0);
    word
}

fn is_single_word(ty: &DynSolType) -> bool {
    matches!(
        ty,
        DynSolType::Address | DynSolType::Uint(_) | DynSolType::Bool | DynSolType::FixedBytes(_)
    )
}

/// 标准函数按旧版编译器的读法取参数,返回参数和多余的字节.
/// 参数都占一个字时按字取值,动态类型交给官方解码器,解不开时参数都算多余的字节,页面照样显示方法.
fn loose_args(function: &Function, args: &[u8]) -> (Vec<DynSolValue>, Vec<u8>) {
    let types: Vec<DynSolType> = function
        .inputs
        .iter()
        .filter_map(|param| param.resolve().ok())
        .collect();
    if types.len() == function.inputs.len() && types.iter().all(is_single_word) {
        let words = types
            .iter()
            .enumerate()
            .flat_map(|(index, ty)| cleaned(ty, loose_word(args, index)));
        let clean: Vec<u8> = words.collect();
        let values = function.abi_decode_input(&clean).unwrap_or_default();
        return (values, args.get(clean.len()..).unwrap_or_default().to_vec());
    }
    let mut padded = args.to_vec();
    padded.resize(args.len().next_multiple_of(WORD).max(function.inputs.len() * WORD), 0);
    let Ok(values) = function.abi_decode_input(&padded) else {
        return (Vec::new(), args.to_vec());
    };
    let used = function
        .abi_encode_input_raw(&values)
        .map_or(0, |rebuilt| rebuilt.len());
    (values, args.get(used..).unwrap_or_default().to_vec())
}

/// 地址的写法随链不同,比如 TRON 用 Base58.
pub struct Style<'a> {
    pub chain: Chain,
    pub network: &'a str,
    pub manifest: &'a Manifest,
    pub format: AddressText<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    pub risk: Risk,
    pub title: String,
    pub amount: Option<AmountView>,
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    /// 方法和合约,写在 `data` 这一行的备注里.
    pub details: Vec<(&'static str, String)>,
    /// 这次调用交给策略的事实.
    pub fact: CallFact,
    /// 解出的参数,挂在 `data` 下面,来源是 `descriptor`.
    pub fields: Vec<Node>,
    /// 嵌入调用.每一个都要让用户看到.
    pub inner: Vec<Classified>,
}

impl Classified {
    /// 全部调用的事实,外层在前.
    pub fn facts(&self) -> Vec<CallFact> {
        std::iter::once(self.fact.clone())
            .chain(self.inner.iter().flat_map(Self::facts))
            .collect()
    }

    /// 这笔调用和嵌入调用的对方.
    pub fn parties(&self) -> Vec<Party> {
        let own = self.counterparty_label.iter().zip(&self.counterparty);
        own.map(|(label, value)| Party {
            label: label.clone(),
            value: value.clone(),
        })
        .chain(self.inner.iter().flat_map(Self::parties))
        .collect()
    }

    fn line(&self, details: &[(String, String)]) -> String {
        let rows: Vec<String> = details
            .iter()
            .map(|(label, value)| [label.as_str(), value].join(" "))
            .collect();
        [self.title.as_str(), rows.join(",").as_str()].join(":")
    }

    fn own_details(&self) -> Vec<(String, String)> {
        self.details
            .iter()
            .map(|(label, value)| (label.to_string(), value.clone()))
            .collect()
    }

    /// 嵌入调用不在页面顶部,金额和对方也写进备注.
    fn inner_line(&self) -> String {
        let amount = self.amount.iter().map(|amount| {
            let symbol = amount
                .symbol
                .iter()
                .map(|symbol| [" ", symbol].concat())
                .collect::<String>();
            let text = if amount.unlimited {
                "无限"
            } else {
                amount.text.as_str()
            };
            ("金额".to_string(), [text, symbol.as_str()].concat())
        });
        let party = self.counterparty_label.iter().zip(&self.counterparty);
        let party = party.map(|(label, value)| (label.clone(), value.clone()));
        let details: Vec<(String, String)> = amount.chain(party).chain(self.own_details()).collect();
        self.line(&details)
    }

    fn lines(&self) -> Vec<String> {
        let inner = self.inner.iter().flat_map(|call| {
            if call.inner.is_empty() {
                vec![call.inner_line()]
            } else {
                call.lines()
            }
        });
        std::iter::once(self.line(&self.own_details())).chain(inner).collect()
    }

    /// `data` 这一行的备注.嵌入调用各占一行.
    pub fn note(&self) -> String {
        self.lines().join("\n")
    }
}

/// uint160 的最大值就是 Permit2 的"无限".更大的额度也一样对待.
fn is_unlimited(amount: U256) -> bool {
    amount >= U256::from(U160::MAX)
}

fn amount_view(style: &Style, contract: &str, amount: U256) -> AmountView {
    let raw = amount.to_string();
    match style.manifest.lookup(style.network, contract) {
        Some(asset) => AmountView {
            text: format_units(&raw, asset.decimals),
            symbol: Some(asset.symbol.clone()),
            asset: Some(asset.key()),
            unlimited: false,
        },
        None => AmountView {
            text: raw,
            symbol: None,
            asset: None,
            unlimited: false,
        },
    }
}

fn listed(style: &Style, contract: &str) -> bool {
    style.manifest.lookup(style.network, contract).is_some()
}

fn listed_text(style: &Style, contract: &str) -> &'static str {
    if listed(style, contract) {
        "在内置清单"
    } else {
        "不在内置清单"
    }
}

fn unknown(style: &Style, contract: &str, data: &[u8], shape: CallShape) -> Classified {
    let selector = data
        .get(..4)
        .map_or_else(|| "无".to_string(), |s| format!("0x{}", s.to_lower_hex_string()));
    let fact = CallFact {
        shape,
        contract: contract.to_string(),
        ..CallFact::default()
    };
    Classified {
        risk: policy::call_risk(crate::chains::rules(style.chain), &fact),
        title: Risk::Unknown.label().to_string(),
        amount: None,
        counterparty_label: Some("目标合约".into()),
        counterparty: Some(contract.to_string()),
        details: vec![("方法选择器", selector), ("数据长度", format!("{} 字节", data.len()))],
        fact,
        fields: Vec::new(),
        inner: Vec::new(),
    }
}

/// 这次调用的代币:`tokenPath` 指向参数时取那个地址,否则是目标合约.
fn token_of(style: &Style, field: &DisplayField, contract: &str, args: &[(&str, &DynSolValue)]) -> String {
    let path = field.params.token_path.as_deref().filter(|path| *path != TARGET_PATH);
    let named = path.and_then(|path| args.iter().find(|(name, _)| *name == path));
    match named {
        Some((_, DynSolValue::Address(address))) => (style.format)(&address.into_array()),
        Some(_) | None => contract.to_string(),
    }
}

fn amount_node(label: &str, view: &AmountView, value: U256, style: &Style, token: &str) -> Node {
    let asset = style.manifest.lookup(style.network, token);
    let amount = Amount {
        raw: value.to_be_bytes_trimmed_vec(),
        decimals: asset.map(|asset| asset.decimals),
        unit: view.symbol.clone(),
    };
    Node::new(label, Value::Amount(amount), Source::Descriptor)
}

/// 一次调用解出来的内容.
#[derive(Default)]
struct Reading {
    amount: Option<AmountView>,
    counterparty: Option<(String, String)>,
    unlimited: bool,
    fields: Vec<Node>,
    inner: Vec<Classified>,
}

/// 嵌入调用的字节.超过层数和条数时是 `None`.
fn embedded(value: &DynSolValue, depth: usize) -> Option<Vec<&[u8]>> {
    let calls: Vec<&[u8]> = match value {
        DynSolValue::Bytes(bytes) => vec![bytes.as_slice()],
        DynSolValue::Array(items) | DynSolValue::FixedArray(items) => {
            items.iter().map(DynSolValue::as_bytes).collect::<Option<_>>()?
        }
        _ => return None,
    };
    (depth < MAX_DEPTH && calls.len() <= MAX_INNER_CALLS).then_some(calls)
}

struct Call<'a> {
    contract: &'a [u8; 20],
    text: &'a str,
    depth: usize,
}

struct Argument<'a> {
    param: &'a str,
    value: &'a DynSolValue,
    field: Option<&'a DisplayField>,
    /// 放到页面顶部的那个地址参数.
    lead: Option<&'a str>,
}

fn read_field(
    style: &Style,
    call: &Call,
    argument: &Argument,
    args: &[(&str, &DynSolValue)],
    reading: &mut Reading,
) -> Option<()> {
    let Argument {
        param,
        value,
        field,
        lead,
    } = *argument;
    let label = field.map_or(param, |field| field.label.as_str());
    let format = field.and_then(|field| field.format.as_deref());
    let plain = || abi_node(label.to_string(), value, Source::Descriptor, style.format);
    let node = match (format, field, value) {
        (Some(TOKEN_AMOUNT), Some(field), DynSolValue::Uint(amount, _)) => {
            let token = token_of(style, field, call.text, args);
            let mut view = amount_view(style, &token, *amount);
            view.unlimited = field.params.threshold.is_some() && is_unlimited(*amount);
            reading.unlimited |= view.unlimited;
            let node = amount_node(label, &view, *amount, style, &token);
            reading.amount.get_or_insert(view);
            node
        }
        (Some(ADDRESS_NAME), _, DynSolValue::Address(address)) => {
            if lead == Some(param) {
                let text = (style.format)(&address.into_array());
                reading.counterparty = Some((field_label(style.chain, label), text));
            }
            plain()
        }
        (Some(ALL_ASSETS), _, DynSolValue::Bool(true)) => {
            let mut view = amount_view(style, call.text, U256::MAX);
            view.unlimited = true;
            reading.unlimited = true;
            reading.amount.get_or_insert(view);
            plain()
        }
        (Some(EMBEDDED_CALL), _, _) => {
            let calls = embedded(value, call.depth)?;
            let inner: Vec<Classified> = calls
                .iter()
                .map(|data| classify_at(style, call.contract, data, false, call.depth + 1))
                .collect();
            let rows = inner.iter().map(call_node).collect();
            reading.inner = inner;
            Node::new(CALLS_ROW, Value::List, Source::Descriptor).children(rows)
        }
        _ => plain(),
    };
    reading.fields.push(node);
    Some(())
}

/// 一个嵌入调用在列表里的一组字段.
fn call_node(call: &Classified) -> Node {
    Node::new(call.title.as_str(), Value::Struct, Source::Descriptor).children(call.fields.clone())
}

fn read(style: &Style, call: &Call, entry: &Described, values: &[DynSolValue]) -> Option<Reading> {
    let args: Vec<(&str, &DynSolValue)> = entry
        .function
        .inputs
        .iter()
        .map(|param| param.name.as_str())
        .zip(values)
        .collect();
    let fields = &entry.format.fields;
    // 描述文件的顺序就是展示的优先级,参数的顺序不是:`transferFrom` 的顶部是收款方.
    let lead = fields
        .iter()
        .find(|field| field.format.as_deref() == Some(ADDRESS_NAME));
    let lead = lead.map(|field| field.path.as_str());
    let mut reading = Reading::default();
    for (param, value) in &args {
        let argument = Argument {
            param,
            value,
            field: fields.iter().find(|field| field.path == *param),
            lead,
        };
        read_field(style, call, &argument, &args, &mut reading)?;
    }
    Some(reading)
}

/// 不规范的调用类别是无法识别,页面顶部仍按它读出来的方法排序,被授权方才不会被藏起来.
fn lead_risk(style: &Style, call: &Classified) -> Risk {
    match &call.fact.shape {
        CallShape::Irregular(signature) => {
            let regular = CallFact {
                shape: CallShape::Standard(signature.clone()),
                listed: true,
                ..call.fact.clone()
            };
            policy::call_risk(crate::chains::rules(style.chain), &regular)
        }
        CallShape::Undecoded | CallShape::Standard(_) | CallShape::Described => call.risk,
    }
}

/// 只有一个动作是最高风险时才放到页面顶部,否则金额和对方会误导用户.
fn lead_with(style: &Style, result: &mut Classified) {
    let ranked = |call: &Classified| lead_risk(style, call).headline_rank();
    let top = result.inner.iter().map(ranked).max();
    let mut leaders = result.inner.iter().filter(|call| Some(ranked(call)) == top);
    if let (Some(leader), None) = (leaders.next(), leaders.next())
        && ranked(leader) > Risk::Unknown.headline_rank()
    {
        result.amount = leader.amount.clone();
        result.counterparty_label = leader.counterparty_label.clone();
        result.counterparty = leader.counterparty.clone();
    }
}

fn described(style: &Style, call: &Call, entry: &Described, reading: Reading, fact: CallFact) -> Classified {
    let method = entry.function.name.as_str();
    let risk = reading
        .inner
        .iter()
        .map(|inner| inner.risk)
        .chain([policy::call_risk(crate::chains::rules(style.chain), &fact)])
        .max()
        .unwrap_or(Risk::Unknown);
    let title = if risk == Risk::Transfer {
        risk.label().to_string()
    } else {
        field_label(style.chain, &entry.format.intent)
    };
    let (counterparty_label, counterparty) = reading.counterparty.unzip();
    let mut result = Classified {
        risk,
        title,
        amount: reading.amount,
        counterparty_label,
        counterparty,
        details: vec![
            ("调用方法", method.to_string()),
            ("合约", call.text.to_string()),
            ("合约清单", listed_text(style, call.text).to_string()),
        ],
        fact,
        fields: reading.fields,
        inner: reading.inner,
    };
    if !result.inner.is_empty() {
        result.details.push(("内部调用", format!("{} 个", result.inner.len())));
        lead_with(style, &mut result);
    }
    result
}

/// 给一次合约调用定类别.`has_value` 表示这次调用同时转了原生币.
pub fn classify(style: &Style, contract: &[u8; 20], data: &[u8], has_value: bool) -> Classified {
    classify_at(style, contract, data, has_value, 0)
}

fn classify_at(style: &Style, contract: &[u8; 20], data: &[u8], has_value: bool, depth: usize) -> Classified {
    let text = (style.format)(contract);
    let Some((selector, args)) = data.split_first_chunk::<4>() else {
        return unknown(style, &text, data, CallShape::Undecoded);
    };
    let selector = Selector::from(*selector);
    let call = Call {
        contract,
        text: &text,
        depth,
    };
    let known = standard().iter().find(|entry| entry.function.selector() == selector);
    let foreign = known
        .is_none()
        .then(|| manifest_entry(style, &text, selector))
        .flatten();
    let Some(entry) = known.or(foreign.as_ref()) else {
        return unknown(style, &text, data, CallShape::Undecoded);
    };
    let signature = entry.function.signature();
    let strict = decoded_args(&entry.function, args);
    let regular = strict.is_some();
    let (values, extra) = match (strict, known) {
        (Some(values), _) => (values, Vec::new()),
        (None, Some(_)) => loose_args(&entry.function, args),
        (None, None) => return unknown(style, &text, data, CallShape::Undecoded),
    };
    let shape = match known {
        Some(_) if regular => CallShape::Standard(signature),
        Some(_) => CallShape::Irregular(signature),
        None => CallShape::Described,
    };
    let Some(mut reading) = read(style, &call, entry, &values) else {
        // 嵌入调用超过上限,这一层按没有描述处理.
        let shape = match shape {
            CallShape::Irregular(_) => shape,
            _ => CallShape::Undecoded,
        };
        return unknown(style, &text, data, shape);
    };
    if !extra.is_empty() {
        let row = Node::new(TRAILING_ROW, Value::Opaque(extra), Source::Descriptor);
        reading.fields.push(row);
    }
    let fact = CallFact {
        shape,
        contract: text.clone(),
        listed: listed(style, &text),
        has_value,
        unlimited: reading.unlimited,
    };
    described(style, &call, entry, reading, fact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::{evm_text, parse_tron_address, tron_text};
    use crate::decode::display;
    use crate::manifest::Descriptor;
    use sha3::{Digest, Keccak256};

    type Word = [u8; 32];

    const USDT: [u8; 20] = [
        0xda, 0xc1, 0x7f, 0x95, 0x8d, 0x2e, 0xe5, 0x23, 0xa2, 0x20, 0x62, 0x06, 0x99, 0x45, 0x97, 0xc1, 0x3d, 0x83,
        0x1e, 0xc7,
    ];
    const OTHER: [u8; 20] = [0x11; 20];
    const PEER: [u8; 20] = [0x22; 20];

    fn style() -> Style<'static> {
        Style {
            chain: Chain::Evm,
            network: "ethereum",
            manifest: Manifest::builtin(),
            format: &evm_text,
        }
    }

    fn selector(signature: &str) -> [u8; 4] {
        Keccak256::digest(signature.as_bytes())[..4].try_into().unwrap()
    }

    fn increase_allowance() -> [u8; 4] {
        selector("increaseAllowance(address,uint256)")
    }

    fn increase_approval() -> [u8; 4] {
        selector("increaseApproval(address,uint256)")
    }

    fn set_approval_for_all() -> [u8; 4] {
        selector("setApprovalForAll(address,bool)")
    }

    fn permit() -> [u8; 4] {
        selector("permit(address,address,uint256,uint256,uint8,bytes32,bytes32)")
    }

    fn permit2_approve() -> [u8; 4] {
        selector("approve(address,address,uint160,uint48)")
    }

    fn multicall_selector() -> [u8; 4] {
        selector("multicall(bytes[])")
    }

    fn multicall_deadline() -> [u8; 4] {
        selector("multicall(uint256,bytes[])")
    }

    fn call(sel: [u8; 4], words: &[Word]) -> Vec<u8> {
        let mut data = sel.to_vec();
        words.iter().for_each(|w| data.extend_from_slice(w));
        data
    }

    fn addr(a: [u8; 20]) -> Word {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(&a);
        w
    }

    fn uint(v: u64) -> Word {
        let mut w = [0u8; 32];
        w[24..].copy_from_slice(&v.to_be_bytes());
        w
    }

    fn field<'a>(result: &'a Classified, name: &str) -> &'a Node {
        result.fields.iter().find(|node| node.name == name).unwrap()
    }

    /// 期望值来自函数签名,不来自描述文件,防止内置函数缺了或抄错.
    #[test]
    fn selectors_match_their_signatures() {
        assert_eq!(TRANSFER, selector("transfer(address,uint256)"));
        assert_eq!(APPROVE, selector("approve(address,uint256)"));
        let signatures = standard_signatures();
        for wanted in [
            "transfer(address,uint256)",
            "approve(address,uint256)",
            "transferFrom(address,address,uint256)",
            "increaseAllowance(address,uint256)",
            "increaseApproval(address,uint256)",
            "decreaseAllowance(address,uint256)",
            "decreaseApproval(address,uint256)",
            "setApprovalForAll(address,bool)",
            "permit(address,address,uint256,uint256,uint8,bytes32,bytes32)",
            "approve(address,address,uint160,uint48)",
            "multicall(bytes[])",
            "multicall(uint256,bytes[])",
        ] {
            assert!(signatures.iter().any(|found| found == wanted), "{wanted}");
        }
        for entry in standard() {
            let wanted: [u8; 4] = selector(&entry.function.signature());
            assert_eq!(entry.function.selector().0, wanted);
        }
    }

    /// 描述文件写错参数名时这一行会退回 ABI 名字,这里当场发现.
    #[test]
    fn every_builtin_field_names_a_parameter() {
        for entry in standard() {
            for field in &entry.format.fields {
                let found = entry.function.inputs.iter().any(|param| param.name == field.path);
                assert!(found, "{} {}", entry.function.signature(), field.path);
            }
        }
    }

    #[test]
    fn listed_token_transfer_is_plain() {
        let result = classify(
            &style(),
            &USDT,
            &call(TRANSFER, &[addr(PEER), uint(1_250_000_000)]),
            false,
        );
        assert_eq!(result.risk, Risk::Transfer);
        let amount = result.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1250", Some("USDT")));
    }

    #[test]
    fn a_fake_token_cannot_get_the_green_label() {
        let result = classify(
            &style(),
            &OTHER,
            &call(TRANSFER, &[addr(PEER), uint(1_250_000_000)]),
            false,
        );
        assert_eq!(result.risk, Risk::Unknown);
        let amount = result.amount.clone().unwrap();
        assert_eq!(amount.text, "1250000000");
        assert_eq!(amount.symbol, None);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert_eq!(result.counterparty_label.as_deref(), Some("收款地址"));
    }

    #[test]
    fn transfer_with_native_value_is_not_plain() {
        let result = classify(&style(), &USDT, &call(TRANSFER, &[addr(PEER), uint(1)]), true);
        assert_eq!(result.risk, Risk::Unknown);
    }

    /// 合约照样按这些字节转账,所以金额和收款方都要显示,类别不能是普通转账.
    #[test]
    fn trailing_bytes_are_not_a_plain_transfer() {
        let mut data = call(TRANSFER, &[addr(PEER), uint(1)]);
        data.push(0xaa);
        let result = classify(&style(), &USDT, &data, false);
        assert_eq!(result.risk, Risk::Unknown);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert_eq!(result.amount.unwrap().symbol.as_deref(), Some("USDT"));
    }

    #[test]
    fn a_dirty_address_word_reads_its_low_bytes() {
        let mut word = addr(PEER);
        word[0] = 1;
        let result = classify(&style(), &USDT, &call(TRANSFER, &[word, uint(1)]), false);
        assert_eq!(result.risk, Risk::Unknown);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
    }

    /// 清单外的合约不一定遵守标准,类别是无法识别,但被授权方和额度不能丢.
    #[test]
    fn approvals_show_the_spender_on_any_contract() {
        for (contract, risk) in [(USDT, Risk::Approval), (OTHER, Risk::Unknown)] {
            let result = classify(&style(), &contract, &call(APPROVE, &[addr(PEER), [0xff; 32]]), false);
            assert_eq!(result.risk, risk);
            assert!(result.fact.unlimited);
            assert_eq!(result.counterparty, Some(evm_text(&PEER)));
            assert_eq!(result.counterparty_label.as_deref(), Some("被授权方"));
            assert!(result.amount.as_ref().unwrap().unlimited);
            assert_eq!(display(&field(&result, "Spender").value), evm_text(&PEER));
        }
    }

    #[test]
    fn descriptor_fields_never_count_as_evidence() {
        let result = classify(&style(), &USDT, &call(TRANSFER, &[addr(PEER), uint(1)]), false);
        assert!(result.fields.iter().all(|node| node.source == Source::Descriptor));
        assert_eq!(display(&field(&result, "Amount").value), "0.000001 USDT");
    }

    #[test]
    fn the_page_leads_with_the_field_the_descriptor_puts_first() {
        let moved = call(
            selector("transferFrom(address,address,uint256)"),
            &[addr(OTHER), addr(PEER), uint(1)],
        );
        let result = classify(&style(), &USDT, &moved, false);
        assert_eq!(result.risk, Risk::Recognized);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert_eq!(display(&field(&result, "From").value), evm_text(&OTHER));
        let permitted = call(
            permit(),
            &[addr(OTHER), addr(PEER), uint(1), uint(2), uint(27), [1; 32], [2; 32]],
        );
        let result = classify(&style(), &USDT, &permitted, false);
        assert_eq!(result.risk, Risk::Approval);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert_eq!(result.counterparty_label.as_deref(), Some("被授权方"));
    }

    #[test]
    fn permit2_reads_the_amount_of_the_token_it_names() {
        let token_amount = call(
            permit2_approve(),
            &[addr(USDT), addr(PEER), uint(2_000_000), uint(1_800_000_000)],
        );
        let result = classify(&style(), &OTHER, &token_amount, false);
        assert_eq!(result.risk, Risk::Unknown);
        let amount = result.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("2", Some("USDT")));
    }

    #[test]
    fn set_approval_for_all_is_unlimited_and_revoking_is_not_an_approval() {
        let grant = classify(
            &style(),
            &OTHER,
            &call(set_approval_for_all(), &[addr(PEER), uint(1)]),
            false,
        );
        assert_eq!(grant.risk, Risk::Unknown);
        assert!(grant.amount.unwrap().unlimited);
        let revoke = classify(
            &style(),
            &OTHER,
            &call(set_approval_for_all(), &[addr(PEER), uint(0)]),
            false,
        );
        assert_eq!(revoke.risk, Risk::Unknown);
        assert_eq!(revoke.amount, None);
        assert!(!revoke.fact.unlimited);
    }

    fn tron_style() -> Style<'static> {
        Style {
            chain: Chain::Tron,
            network: "tron",
            manifest: Manifest::builtin(),
            format: &tron_text,
        }
    }

    fn tron_usdt() -> [u8; 20] {
        parse_tron_address("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").unwrap()
    }

    /// Solidity 忽略尾部字节,旧版编译器屏蔽地址高位,缺的字节读成 0.
    /// 合约照样执行,所以方法,被授权方,额度都要显示,无限额度的事实也不能丢.
    #[test]
    fn an_irregular_approve_shows_what_the_contract_reads_on_both_chains() {
        let cases = [
            (style(), USDT),
            (style(), OTHER),
            (tron_style(), tron_usdt()),
            (tron_style(), OTHER),
        ];
        let mut dirty = addr(PEER);
        dirty[..12].fill(0xee);
        for (style, contract) in cases {
            let spender = (style.format)(&PEER);
            let mut trailing = call(APPROVE, &[addr(PEER), [0xff; 32]]);
            trailing.push(0xaa);
            let dirty = call(APPROVE, &[dirty, [0xff; 32]]);
            let full = call(APPROVE, &[addr(PEER), uint(0x0102)]);
            for (data, unlimited) in [(trailing, true), (dirty, true), (full[..67].to_vec(), false)] {
                let result = classify(&style, &contract, &data, false);
                assert_eq!(result.risk, Risk::Unknown);
                assert_eq!(result.title, "授权");
                assert_eq!(
                    result.fact.shape,
                    CallShape::Irregular("approve(address,uint256)".to_string())
                );
                assert_eq!(result.counterparty.as_deref(), Some(spender.as_str()));
                assert_eq!(result.counterparty_label.as_deref(), Some("被授权方"));
                assert_eq!(display(&field(&result, "Spender").value), spender);
                assert_eq!(result.fact.unlimited, unlimited);
                let amount = result.amount.clone().unwrap();
                assert_eq!(amount.unlimited, unlimited);
                assert_eq!(amount.symbol.is_some(), contract != OTHER);
                assert!(result.note().contains("调用方法 approve"));
            }
        }
    }

    #[test]
    fn extra_bytes_are_an_opaque_row_and_missing_bytes_read_as_zero() {
        let mut data = call(APPROVE, &[addr(PEER), uint(1)]);
        data.extend_from_slice(&[0xaa, 0xbb]);
        let result = classify(&style(), &OTHER, &data, false);
        let row = field(&result, TRAILING_ROW);
        assert_eq!(row.value, Value::Opaque(vec![0xaa, 0xbb]));
        assert_eq!(row.source, Source::Descriptor);
        let short = call(APPROVE, &[addr(PEER), uint(0x0102)]);
        let result = classify(&style(), &OTHER, &short[..short.len() - 1], false);
        assert_eq!(result.amount.unwrap().text, "256");
        assert!(result.fields.iter().all(|node| node.name != TRAILING_ROW));
        // 地址只剩 19 字节,合约读到的地址低位是 0.
        let result = classify(&style(), &OTHER, &short[..35], false);
        let mut cut = PEER;
        cut[19] = 0;
        assert_eq!(result.counterparty, Some(evm_text(&cut)));
        assert_eq!(result.amount.unwrap().text, "0");
    }

    /// 标准函数的参数都要读出来,尾部字节单独一行.
    #[test]
    fn every_builtin_function_survives_a_trailing_byte() {
        for entry in standard() {
            let function = &entry.function;
            let values: Vec<DynSolValue> = function
                .inputs
                .iter()
                .map(|param| match param.resolve().unwrap() {
                    DynSolType::Address => DynSolValue::Address(PEER.into()),
                    DynSolType::Bool => DynSolValue::Bool(true),
                    DynSolType::Uint(bits) => DynSolValue::Uint(U256::from(1), bits),
                    DynSolType::FixedBytes(size) => DynSolValue::FixedBytes([1; 32].into(), size),
                    DynSolType::Array(_) => DynSolValue::Array(vec![DynSolValue::Bytes(vec![1])]),
                    other => panic!("{other:?}"),
                })
                .collect();
            let mut data = function.abi_encode_input(&values).unwrap();
            assert_eq!(classify(&style(), &OTHER, &data, false).fields.len(), values.len());
            data.push(0xaa);
            let result = classify(&style(), &OTHER, &data, false);
            let signature = function.signature();
            assert_eq!(result.risk, Risk::Unknown, "{signature}");
            assert_eq!(result.fact.shape, CallShape::Irregular(signature.clone()));
            assert_eq!(result.fields.len(), values.len() + 1, "{signature}");
            assert_eq!(field(&result, TRAILING_ROW).value, Value::Opaque(vec![0xaa]));
        }
    }

    #[test]
    fn set_approval_for_all_with_a_dirty_flag_counts_as_granted() {
        let mut flag = [0; 32];
        flag[0] = 1;
        let result = classify(
            &style(),
            &OTHER,
            &call(set_approval_for_all(), &[addr(PEER), flag]),
            false,
        );
        assert!(result.fact.unlimited);
        assert!(result.amount.unwrap().unlimited);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
    }

    #[test]
    fn permit_and_permit2_read_their_narrow_integers_by_the_low_bits() {
        let mut amount = uint(7);
        amount[0] = 0xee;
        let result = classify(
            &style(),
            &OTHER,
            &call(permit2_approve(), &[addr(USDT), addr(PEER), amount, uint(9)]),
            false,
        );
        assert!(matches!(result.fact.shape, CallShape::Irregular(_)));
        assert_eq!(display(&field(&result, "Amount").value), "0.000007 USDT");
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
    }

    #[test]
    fn irregular_revoke_is_not_trusted() {
        let mut dirty = addr(PEER);
        dirty[0] = 1;
        let result = classify(
            &style(),
            &OTHER,
            &call(set_approval_for_all(), &[dirty, uint(0)]),
            false,
        );
        assert_eq!(result.risk, Risk::Unknown);
        assert!(matches!(result.fact.shape, CallShape::Irregular(_)));
    }

    #[test]
    fn short_approvals_are_unknown_and_named() {
        let calls = [
            call(APPROVE, &[addr(PEER), uint(1)]),
            call(increase_allowance(), &[addr(PEER), uint(1)]),
            call(increase_approval(), &[addr(PEER), uint(1)]),
            call(set_approval_for_all(), &[addr(PEER), uint(1)]),
            call(permit(), &[addr(OTHER), addr(PEER), uint(1)]),
            call(permit2_approve(), &[addr(OTHER), addr(PEER), uint(1)]),
        ];
        for data in &calls {
            for len in [4, 5, 36, 67] {
                let result = classify(&style(), &OTHER, &data[..len], false);
                assert_eq!(result.risk, Risk::Unknown, "{len}");
                assert!(matches!(result.fact.shape, CallShape::Irregular(_)), "{len}");
                assert!(result.counterparty.is_some(), "{len}");
            }
        }
    }

    fn multicall(calls: &[Vec<u8>]) -> Vec<u8> {
        let mut data = multicall_selector().to_vec();
        data.extend_from_slice(&uint(32));
        data.extend_from_slice(&uint(calls.len() as u64));
        let mut body = Vec::new();
        let mut heads = Vec::new();
        for item in calls {
            heads.extend_from_slice(&uint((calls.len() * 32 + body.len()) as u64));
            body.extend_from_slice(&uint(item.len() as u64));
            body.extend_from_slice(item);
            body.resize(body.len().div_ceil(32) * 32, 0);
        }
        data.extend_from_slice(&heads);
        data.extend_from_slice(&body);
        data
    }

    #[test]
    fn approval_hidden_inside_multicall_is_found() {
        let inner = vec![
            call(TRANSFER, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(PEER), [0xff; 32]]),
        ];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        // 一组调用的最低类别是无法识别,里面的授权要看得到.
        assert_eq!(result.risk, Risk::Unknown);
        assert!(result.inner.iter().any(|call| call.risk == Risk::Approval));
        assert!(result.facts().iter().any(|fact| fact.unlimited));
    }

    #[test]
    fn every_inner_call_is_shown() {
        let attacker = [0x33; 20];
        let inner = vec![
            call(TRANSFER, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(attacker), [0xff; 32]]),
        ];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.risk, Risk::Unknown);
        let note = result.note();
        assert_eq!(note.lines().count(), 4);
        assert!(note.contains(&evm_text(&attacker)));
        assert!(note.contains("金额 无限 USDT"));
        assert_eq!(result.parties().len(), 3);
        // 两个授权并列最高,顶部不能只显示其中一个.
        assert_eq!(result.counterparty, None);
        assert_eq!(result.amount, None);
        let calls = field(&result, CALLS_ROW);
        assert_eq!(calls.children.len(), 3);
        assert!(calls.children.iter().all(|call| !call.children.is_empty()));
    }

    #[test]
    fn a_single_approval_in_a_multicall_leads_the_page() {
        let inner = vec![call(APPROVE, &[addr(PEER), [0xff; 32]])];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert!(result.amount.unwrap().unlimited);
    }

    #[test]
    fn an_irregular_approval_in_a_multicall_leads_the_page() {
        let mut approve = call(APPROVE, &[addr(PEER), [0xff; 32]]);
        approve.push(0xaa);
        let result = classify(&style(), &USDT, &multicall(&[approve]), false);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert!(result.amount.as_ref().unwrap().unlimited);
        assert!(result.facts().iter().any(|fact| fact.unlimited));
    }

    #[test]
    fn transfers_in_a_multicall_keep_the_summary_risk() {
        let inner = vec![call(TRANSFER, &[addr(PEER), uint(1)])];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.risk, Risk::Unknown);
    }

    #[test]
    fn multicall_of_transfers_is_not_a_plain_transfer() {
        let inner = vec![call(TRANSFER, &[addr(PEER), uint(1)])];
        assert_eq!(classify(&style(), &USDT, &multicall(&inner), false).risk, Risk::Unknown);
    }

    #[test]
    fn nested_multicall_is_followed_until_the_limit() {
        let approve = call(APPROVE, &[addr(PEER), uint(1)]);
        let has_approval = |result: &Classified| {
            result
                .facts()
                .iter()
                .any(|fact| matches!(&fact.shape, CallShape::Standard(s) if s.starts_with("approve")))
        };
        let two = multicall(&[multicall(std::slice::from_ref(&approve))]);
        assert!(has_approval(&classify(&style(), &USDT, &two, false)));
        let deep = multicall(&[multicall(&[multicall(&[multicall(std::slice::from_ref(&approve))])])]);
        let result = classify(&style(), &USDT, &deep, false);
        assert_eq!(result.risk, Risk::Unknown);
        assert!(!has_approval(&result));
    }

    #[test]
    fn too_many_inner_calls_count_as_no_descriptor() {
        let inner = vec![call(TRANSFER, &[addr(PEER), uint(1)]); MAX_INNER_CALLS + 1];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.risk, Risk::Unknown);
        assert!(result.inner.is_empty());
        assert!(result.fields.is_empty());
    }

    #[test]
    fn a_multicall_with_trailing_bytes_still_shows_its_inner_calls() {
        let mut data = multicall(&[call(APPROVE, &[addr(PEER), [0xff; 32]])]);
        data.push(0xaa);
        let result = classify(&style(), &USDT, &data, false);
        assert_eq!(result.risk, Risk::Unknown);
        assert!(matches!(result.fact.shape, CallShape::Irregular(_)));
        assert_eq!(result.inner.len(), 1);
        assert!(result.facts().iter().any(|fact| fact.unlimited));
        assert_eq!(field(&result, TRAILING_ROW).value, Value::Opaque(vec![0xaa]));
    }

    /// 解不开时方法还在,参数整段作为 `opaque` 显示.
    #[test]
    fn an_unreadable_multicall_keeps_the_method_and_the_bytes() {
        let data = call(multicall_selector(), &[uint(32), uint(1), uint(u64::MAX)]);
        let result = classify(&style(), &USDT, &data, false);
        assert!(matches!(result.fact.shape, CallShape::Irregular(_)));
        assert_eq!(field(&result, TRAILING_ROW).value, Value::Opaque(data[4..].to_vec()));
    }

    #[test]
    fn malformed_multicall_does_not_panic() {
        for bad in [
            multicall_selector().to_vec(),
            call(multicall_selector(), &[uint(u64::MAX)]),
            call(multicall_selector(), &[uint(32), uint(u64::MAX)]),
            call(multicall_selector(), &[uint(32), uint(1), uint(u64::MAX)]),
            call(multicall_selector(), &[uint(32), uint(1), uint(32), uint(u64::MAX)]),
        ] {
            assert_eq!(classify(&style(), &USDT, &bad, false).risk, Risk::Unknown);
        }
    }

    fn descriptor(signature: &str) -> Descriptor {
        Descriptor {
            network: "ethereum".into(),
            contract: evm_text(&OTHER),
            signature: signature.into(),
            format: DisplayFormat {
                intent: "质押".into(),
                fields: vec![DisplayField {
                    path: "amount".into(),
                    label: "数量".into(),
                    format: None,
                    params: crate::manifest::DisplayParams::default(),
                }],
            },
        }
    }

    fn with_descriptors(signatures: &[&str]) -> Manifest {
        let mut manifest = Manifest::builtin().clone();
        manifest.descriptors = signatures.iter().map(|text| descriptor(text)).collect();
        manifest
    }

    #[test]
    fn a_descriptor_only_changes_the_display() {
        let manifest = with_descriptors(&["stake(uint256 amount,uint256 lock)"]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let data = call(selector("stake(uint256,uint256)"), &[uint(5), uint(6)]);
        let result = classify(&style, &OTHER, &data, false);
        assert_eq!((result.risk, result.title.as_str()), (Risk::Recognized, "质押"));
        let approve = classify(&style, &OTHER, &call(APPROVE, &[addr(PEER), uint(1)]), false);
        assert_eq!(approve.risk, Risk::Unknown);
        assert_eq!(classify(&style, &OTHER, &data, true).risk, Risk::Unknown);
        assert_eq!(classify(&style, &USDT, &data, false).risk, Risk::Unknown);
    }

    /// 描述文件的函数重新编码必须等于 `data`,否则按没有描述处理.
    #[test]
    fn a_descriptor_stays_strict() {
        let manifest = with_descriptors(&["stake(uint256 amount,uint256 lock)"]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let data = call(selector("stake(uint256,uint256)"), &[uint(5), uint(6)]);
        let mut trailing = data.clone();
        trailing.push(0xaa);
        for bad in [trailing, data[..data.len() - 1].to_vec()] {
            let result = classify(&style, &OTHER, &bad, false);
            assert_eq!(result.fact.shape, CallShape::Undecoded);
            assert!(result.fields.is_empty());
        }
    }

    /// 窄类型的高位不干净时重新编码也回到原样,这里同样算不规范.
    #[test]
    fn a_descriptor_rejects_values_wider_than_their_type() {
        let manifest = with_descriptors(&[
            "stake(uint8 amount,bytes4 tag)",
            "lock(int8 amount)",
            "batch(uint8[] amount)",
        ]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let mut tag = [0; 32];
        tag[31] = 1;
        let stake = selector("stake(uint8,bytes4)");
        assert_eq!(
            classify(&style, &OTHER, &call(stake, &[uint(0x100), [0; 32]]), false)
                .fact
                .shape,
            CallShape::Undecoded
        );
        assert_eq!(
            classify(&style, &OTHER, &call(stake, &[uint(1), tag]), false)
                .fact
                .shape,
            CallShape::Undecoded
        );
        assert_eq!(
            classify(&style, &OTHER, &call(stake, &[uint(1), [0; 32]]), false)
                .fact
                .shape,
            CallShape::Described
        );
        let lock = selector("lock(int8)");
        for (word, shape) in [
            (uint(0x7f), CallShape::Described),
            ([0xff; 32], CallShape::Described),
            (uint(0x80), CallShape::Undecoded),
        ] {
            assert_eq!(classify(&style, &OTHER, &call(lock, &[word]), false).fact.shape, shape);
        }
        let batch = selector("batch(uint8[])");
        assert_eq!(
            classify(&style, &OTHER, &call(batch, &[uint(32), uint(1), uint(0x100)]), false)
                .fact
                .shape,
            CallShape::Undecoded
        );
    }

    /// 描述文件没写的参数也要显示,描述文件不能藏字段.
    #[test]
    fn a_descriptor_cannot_hide_an_argument() {
        let manifest = with_descriptors(&["stake(uint256 amount,address beneficiary)"]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let data = call(selector("stake(uint256,address)"), &[uint(5), addr(PEER)]);
        let result = classify(&style, &OTHER, &data, false);
        assert_eq!(result.fields.len(), 2);
        assert_eq!(display(&field(&result, "数量").value), "5");
        assert_eq!(display(&field(&result, "beneficiary").value), evm_text(&PEER));
    }

    #[test]
    fn a_manifest_entry_cannot_lower_a_known_selector() {
        let manifest = with_descriptors(&[
            "transfer(address to,uint256 amount)",
            "approve(address spender,uint256 amount)",
            "multicall(bytes[] data)",
            "multicall(uint256 deadline,bytes[] data)",
        ]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let mut trailing = call(TRANSFER, &[addr(PEER), uint(1)]);
        trailing.push(0);
        for bad in [
            trailing,
            call(TRANSFER, &[addr(PEER)]),
            multicall_selector().to_vec(),
            call(multicall_selector(), &[uint(u64::MAX)]),
            call(multicall_deadline(), &[uint(0)]),
        ] {
            assert_eq!(classify(&style, &OTHER, &bad, false).risk, Risk::Unknown);
        }
        let too_deep = multicall(&[call(APPROVE, &[addr(PEER), uint(1)])]);
        assert_eq!(
            classify_at(&style, &OTHER, &too_deep, false, MAX_DEPTH).risk,
            Risk::Unknown
        );
        // 标准函数不看清单的描述文件.
        let listed = classify(&style, &USDT, &call(APPROVE, &[addr(PEER), uint(1)]), false);
        assert_eq!(listed.risk, Risk::Approval);
    }

    /// 选择器相同的描述文件也改变不了标准函数的类别.
    #[test]
    fn a_colliding_descriptor_cannot_replace_a_standard_function() {
        let manifest = with_descriptors(&["approve(address spender,uint256 amount)"]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let result = classify(&style, &OTHER, &call(APPROVE, &[addr(PEER), [0xff; 32]]), false);
        assert_eq!(result.title, "授权");
        assert!(matches!(result.fact.shape, CallShape::Standard(_)));
    }

    #[test]
    fn a_broken_descriptor_is_no_descriptor() {
        let manifest = with_descriptors(&["not a function"]);
        let style = Style {
            manifest: &manifest,
            ..style()
        };
        let result = classify(&style, &OTHER, &[0x12, 0x34, 0x56, 0x78], false);
        assert_eq!(result.fact.shape, CallShape::Undecoded);
    }
}

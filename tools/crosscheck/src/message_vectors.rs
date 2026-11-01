//! 合约调用和消息的对照向量,由 `viem/messages.mjs` 和 `tronweb/messages.mjs` 生成.
//!
//! 运行时用 alloy,所以字段的期望值来自另一套实现.调用的字段按顺序逐个比较,不合规的调用不能读出字段.
//! EIP-712,TIP-712 比较域分隔符,消息哈希和签名原文的哈希,树要通过 C1,C2,C4.

use crate::{locked, sha256};
use bitcoin::hex::FromHex;
use bitone_core::address::{evm_text, parse_tron_address, tron_text};
use bitone_core::decode::{
    AddressText, Decoder, Input, Node, TronPersonalWalker, TypedFlavor, TypedWalker, Value, decode, display,
    parse_typed,
};
use bitone_core::hex::DisplayHex;
use bitone_core::manifest::Manifest;
use bitone_core::policy::CallShape;
use bitone_core::tx::abi::{Style, classify};
use bitone_core::units::decimal_from_be;
use bitone_core::{Chain, Mode};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const EVM_FILE: &str = "evm-messages.json";
const TRON_FILE: &str = "tron-messages.json";
const LOCK: &str = "vectors.lock";
/// 清单内的合约,字段的类别和这里比较的内容无关.
const USDT: [u8; 20] = [
    0xda, 0xc1, 0x7f, 0x95, 0x8d, 0x2e, 0xe5, 0x23, 0xa2, 0x20, 0x62, 0x06, 0x99, 0x45, 0x97, 0xc1, 0x3d, 0x83, 0x1e,
    0xc7,
];
const TRON_USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";

#[derive(Deserialize)]
struct Call {
    case: String,
    data: String,
    fields: Vec<String>,
}

#[derive(Deserialize)]
struct Irregular {
    case: String,
    data: String,
    signature: String,
    fields: Vec<String>,
}

#[derive(Deserialize)]
struct Typed {
    case: String,
    payload: String,
    domain_separator: String,
    hash_struct: Option<String>,
    hash: String,
}

#[derive(Deserialize)]
struct Personal {
    case: String,
    payload: String,
    hash: String,
}

#[derive(Deserialize)]
struct EvmVectors {
    calls: Vec<Call>,
    irregular: Vec<Irregular>,
    typed: Vec<Typed>,
}

#[derive(Deserialize)]
struct TronVectors {
    calls: Vec<Call>,
    personal: Vec<Personal>,
    typed: Vec<Typed>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub calls: usize,
    pub irregular: usize,
    pub typed: usize,
    pub personal: usize,
}

fn read(dir: &Path, file: &str) -> Result<Vec<u8>, String> {
    let describe = |error: std::io::Error| format!("{}:{error}", dir.display());
    let text = fs::read(dir.join(file)).map_err(describe)?;
    let lock = fs::read_to_string(dir.join(LOCK)).map_err(describe)?;
    if locked(&lock)?.get(file).map(String::as_str) != Some(sha256(&text).as_str()) {
        return Err(format!("{file} 的哈希和 {LOCK} 不符"));
    }
    Ok(text)
}

fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    Vec::from_hex(hex).map_err(|error| error.to_string())
}

/// 整数比原始值,EVM 地址比小写,其余和页面显示的一样.
fn plain(value: &Value) -> String {
    match value {
        Value::Amount(amount) => decimal_from_be(&amount.raw),
        Value::Address(text) if text.starts_with("0x") => text.trim_start_matches("0x").to_lowercase(),
        other => display(other),
    }
}

/// 叶子,按参数顺序.
fn leaves(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        if node.value.is_group() {
            leaves(&node.children, out);
        } else {
            out.push(plain(&node.value));
        }
    }
}

fn style<'a>(chain: Chain, network: &'a str, format: AddressText<'a>) -> Style<'a> {
    Style {
        chain,
        network,
        manifest: Manifest::builtin(),
        format,
    }
}

fn check_call(style: &Style, contract: &[u8; 20], vector: &Call) -> Result<(), String> {
    let result = classify(style, contract, &bytes(&vector.data)?, false);
    let mut shown = Vec::new();
    leaves(&result.fields, &mut shown);
    if shown != vector.fields {
        return Err(format!(
            "{} 的字段:树里是 {shown:?},向量是 {:?}",
            vector.case, vector.fields
        ));
    }
    if result.fact.shape == CallShape::Undecoded {
        return Err(format!("{} 没有解开", vector.case));
    }
    Ok(())
}

fn check_irregular(style: &Style, vector: &Irregular) -> Result<(), String> {
    let result = classify(style, &USDT, &bytes(&vector.data)?, false);
    if result.fact.shape != CallShape::Irregular(vector.signature.clone()) {
        return Err(format!("{} 没有按不规范调用处理", vector.case));
    }
    let mut shown = Vec::new();
    leaves(&result.fields, &mut shown);
    if shown != vector.fields {
        return Err(format!(
            "{} 的字段:树里是 {shown:?},向量是 {:?}",
            vector.case, vector.fields
        ));
    }
    Ok(())
}

fn same(case: &str, name: &str, shown: &str, expected: &str) -> Result<(), String> {
    if shown == expected {
        Ok(())
    } else {
        Err(format!("{case} 的 {name}:算出 {shown},向量是 {expected}"))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.to_lower_hex_string()
}

fn run(decoder: &impl Decoder, chain: Chain, payload: &[u8]) -> Result<bitone_core::decode::Decoded, String> {
    let input = Input {
        chain,
        mode: Some(Mode::Mainnet),
        network: chain.id(),
        kind: 1,
        paths: &[],
        payload,
        signer: None,
    };
    decode(decoder, &input).map_err(|failure| failure.to_string())
}

fn signing_hash(decoded: &bitone_core::decode::Decoded) -> Result<String, String> {
    let visits = decoded.visits();
    let row = visits.iter().find(|visit| visit.node.name == "signing_hash");
    row.map(|visit| display(&visit.node.value))
        .ok_or("树里没有 signing_hash".to_string())
}

fn check_typed(vector: &Typed, chain: Chain, flavor: TypedFlavor, format: AddressText<'_>) -> Result<(), String> {
    let case = &vector.case;
    let parsed = parse_typed(vector.payload.as_bytes(), flavor).map_err(|error| format!("{case}:{error}"))?;
    let walker = TypedWalker {
        parsed: &parsed,
        chain_id: None,
        format,
        labels: None,
    };
    let decoded = run(&walker, chain, vector.payload.as_bytes()).map_err(|error| format!("{case}:{error}"))?;
    decoded
        .check_rendered(decoded.visits().len())
        .map_err(|error| error.to_string())?;
    same(
        case,
        "域分隔符",
        &hex(parsed.domain_separator.as_slice()),
        &vector.domain_separator,
    )?;
    let body = parsed.hash_struct.as_ref().map(|hash| hex(hash.as_slice()));
    if body != vector.hash_struct {
        return Err(format!(
            "{case} 的消息哈希:算出 {body:?},向量是 {:?}",
            vector.hash_struct
        ));
    }
    same(case, "签名原文", &hex(parsed.digest.as_slice()), &vector.hash)?;
    same(case, "树里的哈希", &signing_hash(&decoded)?, &vector.hash)
}

fn check_personal(vector: &Personal, chain: Chain, decoder: &impl Decoder) -> Result<(), String> {
    let decoded = run(decoder, chain, &bytes(&vector.payload)?)?;
    same(&vector.case, "签名原文", &signing_hash(&decoded)?, &vector.hash)
}

pub fn check(dir: &Path) -> Result<Report, String> {
    let evm: EvmVectors = serde_json::from_slice(&read(dir, EVM_FILE)?).map_err(|error| error.to_string())?;
    let tron: TronVectors = serde_json::from_slice(&read(dir, TRON_FILE)?).map_err(|error| error.to_string())?;
    let on_evm = style(Chain::Evm, "ethereum", &evm_text);
    let on_tron = style(Chain::Tron, "tron", &tron_text);
    let tron_usdt = parse_tron_address(TRON_USDT).map_err(|error| error.to_string())?;
    evm.calls.iter().try_for_each(|call| check_call(&on_evm, &USDT, call))?;
    tron.calls
        .iter()
        .try_for_each(|call| check_call(&on_tron, &tron_usdt, call))?;
    evm.irregular
        .iter()
        .try_for_each(|call| check_irregular(&on_evm, call))?;
    for vector in &evm.typed {
        check_typed(vector, Chain::Evm, TypedFlavor::Eip712, &evm_text)?;
    }
    for vector in &tron.typed {
        check_typed(vector, Chain::Tron, TypedFlavor::Tip712, &tron_text)?;
    }
    for vector in &tron.personal {
        check_personal(vector, Chain::Tron, &TronPersonalWalker)?;
    }
    Ok(Report {
        calls: evm.calls.len() + tron.calls.len(),
        irregular: evm.irregular.len(),
        typed: evm.typed.len() + tron.typed.len(),
        personal: tron.personal.len(),
    })
}

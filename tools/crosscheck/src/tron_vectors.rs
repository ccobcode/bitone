//! TRON 的对照向量,由 `tronweb/generate.mjs` 从 tronweb 生成.
//!
//! 运行时用 prost,所以对照必须是另一套实现.树的每个字段都要在向量里,向量的每个字段都要在树里,
//! C1,C2 在 `decode` 里检查,C4 比较渲染行数.41 个合约类型都有一笔.

use crate::{locked, sha256};
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::secp256k1::{Message, Secp256k1, ecdsa};
use bitone_core::address::{parse_tron_address, tron_address};
use bitone_core::decode::{
    Decoded, Input, Node, TronWalker, Value as Field, decode, display, tron_contracts, tron_owner, tron_signed,
};
use bitone_core::policy::{TronFacts, assess_tron};
use bitone_core::review::{Forbid, Review};
use bitone_core::tx::tron::UnsignedTx;
use bitone_core::units::decimal_from_be;
use bitone_core::{Chain, Mode};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const FILE: &str = "tron.json";
const LOCK: &str = "vectors.lock";
const LABELS: &str = include_str!("../../../crates/bitone-core/assets/field-labels.json");
const UNKNOWN_FIELD: &str = "field_";
/// 树里不来自载荷的行.
const NOT_IN_VECTORS: [&str; 1] = ["network"];

#[derive(Deserialize)]
struct Transaction {
    case: String,
    contract: Option<String>,
    raw: String,
    txid: String,
    signature: String,
    signed: String,
    owner: Option<String>,
    fields: Value,
}

#[derive(Deserialize)]
struct Refused {
    case: String,
    raw: String,
}

#[derive(Deserialize)]
struct Vectors {
    owner: String,
    transactions: Vec<Transaction>,
    refused: Vec<Refused>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub transactions: usize,
    pub contracts: usize,
    pub refused: usize,
}

fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    Vec::from_hex(hex).map_err(|error| error.to_string())
}

fn run(payload: &[u8]) -> Result<Decoded, String> {
    let input = Input {
        chain: Chain::Tron,
        mode: Some(Mode::Mainnet),
        network: "tron",
        kind: 0,
        paths: &[],
        payload,
        signer: None,
    };
    decode(&TronWalker { coin: None, call: &[] }, &input).map_err(|failure| failure.to_string())
}

/// 向量里的字节都是十六进制,整数是十进制.地址转回字节和 tronweb 的字节比较.
fn plain(value: &Field) -> Result<String, String> {
    Ok(match value {
        Field::Amount(amount) => decimal_from_be(&amount.raw),
        Field::Address(text) => {
            let address = parse_tron_address(text).map_err(|error| error.to_string())?;
            [&[0x41], address.as_slice()].concat().to_lower_hex_string()
        }
        Field::Text(text) => text.as_bytes().to_lower_hex_string(),
        Field::Time(millis) => millis.to_string(),
        other => display(other),
    })
}

/// jspb 的名字没有下划线,全部小写后才能比较.
fn normal(name: &str) -> String {
    name.replace('_', "").to_lowercase()
}

fn compare_fields(case: &str, node: &Node, expected: &Map<String, Value>) -> Result<(), String> {
    let rows = node
        .children
        .iter()
        .filter(|child| !NOT_IN_VECTORS.contains(&child.name.as_str()));
    let mut seen = BTreeSet::new();
    for child in rows {
        let name = normal(&child.name);
        let value = expected
            .get(&name)
            .ok_or_else(|| format!("{case} 的 {} 不在向量里", child.name))?;
        compare(case, child, value)?;
        seen.insert(name);
    }
    match expected.keys().find(|key| !seen.contains(*key)) {
        Some(key) => Err(format!("{case} 的 {key} 不在树里")),
        None => Ok(()),
    }
}

fn compare(case: &str, node: &Node, expected: &Value) -> Result<(), String> {
    match expected {
        Value::Object(fields) => compare_fields(case, node, fields),
        Value::Array(items) => {
            if node.children.len() != items.len() {
                return Err(format!("{case} 的 {} 项数不同", node.name));
            }
            let mut pairs = node.children.iter().zip(items);
            pairs.try_for_each(|(child, item)| compare(case, child, item))
        }
        Value::String(text) => {
            let shown = plain(&node.value)?;
            if shown == *text {
                Ok(())
            } else {
                Err(format!("{case} 的 {}:树里是 {shown},向量是 {text}", node.name))
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Err(format!("{case} 的 {} 类型不对", node.name)),
    }
}

fn check_labels(case: &str, decoded: &Decoded) -> Result<(), String> {
    let labels: BTreeMap<String, String> = serde_json::from_str(LABELS).map_err(|error| error.to_string())?;
    let visits = decoded.visits();
    let missing = visits.iter().find(|visit| {
        let name = &visit.node.name;
        let chained = [Chain::Tron.id(), ".", name].concat();
        !(labels.contains_key(name) || labels.contains_key(&chained) || name.starts_with(UNKNOWN_FIELD))
    });
    match missing {
        Some(visit) => Err(format!("{case} 的字段 {} 没有显示文字", visit.node.name)),
        None => Ok(()),
    }
}

/// C4:树的每个节点都进了渲染列表.
fn check_rendered(case: &str, decoded: &Decoded) -> Result<(), String> {
    let review = Review::assessed(decoded, assess_tron(decoded, &TronFacts::default()));
    if review.forbidden.is_some() || review.fields.len() as u32 != decoded.node_count() {
        return Err(format!("{case} 的渲染行数和节点数不同"));
    }
    Ok(())
}

fn recover(digest: [u8; 32], signature: &[u8; 65]) -> Result<String, String> {
    let describe = |error: bitcoin::secp256k1::Error| error.to_string();
    let (parity, compact) = signature.split_last().ok_or("签名是空的")?;
    // tronweb 的 v 是 27 或 28.
    let recovery = ecdsa::RecoveryId::from_i32(i32::from(*parity) - 27).map_err(describe)?;
    let recoverable = ecdsa::RecoverableSignature::from_compact(compact, recovery).map_err(describe)?;
    let key = Secp256k1::verification_only()
        .recover_ecdsa(&Message::from_digest(digest), &recoverable)
        .map_err(describe)?;
    tron_address(&key.serialize()).map_err(|error| error.to_string())
}

fn check_transaction(vector: &Transaction, owner: &str) -> Result<(), String> {
    let case = &vector.case;
    let payload = bytes(&vector.raw)?;
    let decoded = run(&payload).map_err(|error| format!("{case}:{error}"))?;
    compare(case, decoded.root(), &vector.fields)?;
    check_labels(case, &decoded)?;
    check_rendered(case, &decoded)?;
    UnsignedTx::decode(&payload).map_err(|error| format!("{case}:{error}"))?;
    if tron_owner(decoded.root()) != vector.owner.as_deref() {
        return Err(format!("{case} 的发起方不同"));
    }
    let hash = sha256(&payload);
    if hash != vector.txid {
        return Err(format!("{case} 的 txID 不同"));
    }
    let digest: [u8; 32] = bytes(&hash)?.try_into().map_err(|_| "哈希不是 32 字节")?;
    let signature: [u8; 65] = bytes(&vector.signature)?
        .try_into()
        .map_err(|_| format!("{case} 的签名不是 65 字节"))?;
    if tron_signed(&payload, &signature).to_lower_hex_string() != vector.signed {
        return Err(format!("{case} 的已签名编码不同"));
    }
    if recover(digest, &signature)? != owner {
        return Err(format!("{case} 恢复出的地址不同"));
    }
    Ok(())
}

fn check_refused(vector: &Refused) -> Result<(), String> {
    let payload = bytes(&vector.raw)?;
    if run(&payload).is_ok() || UnsignedTx::decode(&payload) != Err(Forbid::ParseFailed) {
        return Err(format!("{} 应该解码失败", vector.case));
    }
    Ok(())
}

/// 官方枚举的每一个值都要有样本.
fn check_coverage(vectors: &Vectors) -> Result<usize, String> {
    let covered: BTreeSet<&str> = vectors
        .transactions
        .iter()
        .filter_map(|tx| tx.contract.as_deref())
        .collect();
    let contracts = tron_contracts();
    match contracts.iter().find(|(_, name)| !covered.contains(name)) {
        Some((number, name)) => Err(format!("合约类型 {number} {name} 没有样本")),
        None => Ok(contracts.len()),
    }
}

pub fn check(dir: &Path) -> Result<Report, String> {
    let describe = |error: std::io::Error| format!("{}:{error}", dir.display());
    let text = fs::read(dir.join(FILE)).map_err(describe)?;
    let lock = fs::read_to_string(dir.join(LOCK)).map_err(describe)?;
    if locked(&lock)?.get(FILE).map(String::as_str) != Some(sha256(&text).as_str()) {
        return Err(format!("{FILE} 的哈希和 {LOCK} 不符"));
    }
    let vectors: Vectors = serde_json::from_slice(&text).map_err(|error| error.to_string())?;
    let owner = vectors.owner.as_str();
    vectors
        .transactions
        .iter()
        .try_for_each(|tx| check_transaction(tx, owner))?;
    vectors.refused.iter().try_for_each(check_refused)?;
    Ok(Report {
        transactions: vectors.transactions.len(),
        contracts: check_coverage(&vectors)?,
        refused: vectors.refused.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first() -> (Transaction, String) {
        let text = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors").join(FILE)).unwrap();
        let vectors: Vectors = serde_json::from_slice(&text).unwrap();
        let owner = vectors.owner.clone();
        (vectors.transactions.into_iter().nth(1).unwrap(), owner)
    }

    fn amount(fields: &mut Value) -> &mut Map<String, Value> {
        fields["contract"][0]["parameter"]["value"].as_object_mut().unwrap()
    }

    #[test]
    fn the_comparison_notices_a_changed_field() {
        let (vector, owner) = first();
        assert!(check_transaction(&vector, &owner).is_ok());
        let mut changed = first().0;
        amount(&mut changed.fields).insert("amount".to_string(), Value::from("1"));
        assert!(check_transaction(&changed, &owner).is_err());
    }

    #[test]
    fn the_comparison_notices_a_field_on_either_side() {
        let owner = first().1;
        let mut extra = first().0;
        amount(&mut extra.fields).insert("extra".to_string(), Value::from("1"));
        assert!(check_transaction(&extra, &owner).is_err());
        let mut missing = first().0;
        amount(&mut missing.fields).remove("amount");
        assert!(check_transaction(&missing, &owner).is_err());
    }
}

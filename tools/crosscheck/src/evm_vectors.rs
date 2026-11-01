//! EVM 的对照向量,由 `viem/generate.mjs` 从 viem 和 ethereum/tests 生成.
//!
//! 运行时用 alloy 解码,所以对照必须是另一套实现.树里的字段和向量里的字段逐个比较,
//! 签名原文的哈希,装回签名的字节,交易哈希和恢复出的发送方都要相同.

use crate::{locked, sha256};
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::secp256k1::{Message, Secp256k1, ecdsa};
use bitone_core::address::evm_address;
use bitone_core::decode::{Decoded, Decoder, EvmWalker, Input, Node, PersonalWalker, Value, decode, display};
use bitone_core::tx::evm::{UnsignedTx, keccak};
use bitone_core::units::decimal_from_be;
use bitone_core::{Chain, Mode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const FILE: &str = "evm.json";
const LOCK: &str = "vectors.lock";
const LABELS: &str = include_str!("../../../crates/bitone-core/assets/field-labels.json");
/// 树里不来自向量的行:类型字节,手续费上限,EIP-155 的两个占位项,网络,没有 Chain ID 时那一行.
const NOT_IN_VECTORS: [&str; 6] = ["tx_type", "fee_cap", "eip155_r", "eip155_s", "network", "chain_id"];
const LISTS: [&str; 3] = ["access_list", "authorization_list", "blob_versioned_hashes"];

#[derive(Deserialize)]
struct AccessEntry {
    address: String,
    storage_keys: Vec<String>,
}

#[derive(Deserialize)]
struct Authorization {
    chain_id: String,
    address: String,
    nonce: String,
    y_parity: String,
    r: String,
    s: String,
    authority: String,
}

#[derive(Deserialize)]
struct Transaction {
    case: String,
    unsigned: String,
    signature: String,
    signed: String,
    hash: String,
    sender: String,
    fields: BTreeMap<String, String>,
    access_list: Vec<AccessEntry>,
    authorization_list: Vec<Authorization>,
    blob_versioned_hashes: Vec<String>,
}

#[derive(Deserialize)]
struct Refused {
    case: String,
    unsigned: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum MessageKind {
    Transaction,
    Personal,
}

#[derive(Deserialize)]
struct SignedMessage {
    case: String,
    kind: MessageKind,
    payload: String,
    hash: String,
}

#[derive(Deserialize)]
struct Vectors {
    transactions: Vec<Transaction>,
    invalid: Vec<Refused>,
    too_wide: Vec<Refused>,
    messages: Vec<SignedMessage>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub transactions: usize,
    pub invalid: usize,
    pub messages: usize,
}

const WALKER: EvmWalker = EvmWalker {
    coin: None,
    chain_id: None,
};

fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    Vec::from_hex(hex).map_err(|error| error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.to_lower_hex_string()
}

fn run(decoder: &impl Decoder, payload: &[u8]) -> Result<Decoded, String> {
    let input = Input {
        chain: Chain::Evm,
        mode: Some(Mode::Mainnet),
        network: "ethereum",
        kind: 0,
        paths: &[],
        payload,
        signer: None,
    };
    decode(decoder, &input).map_err(|failure| failure.to_string())
}

/// 整数比原始值,地址比小写,其余和页面显示的一样.
fn plain(value: &Value) -> String {
    match value {
        Value::Amount(amount) => decimal_from_be(&amount.raw),
        Value::Address(text) => text.trim_start_matches("0x").to_lowercase(),
        other => display(other),
    }
}

fn child<'a>(node: &'a Node, name: &str) -> Result<&'a Node, String> {
    node.children
        .iter()
        .find(|child| child.name == name)
        .ok_or_else(|| format!("树里没有 {name}"))
}

fn same(case: &str, name: &str, shown: String, expected: &str) -> Result<(), String> {
    if shown == expected {
        Ok(())
    } else {
        Err(format!("{case} 的 {name}:树里是 {shown},向量是 {expected}"))
    }
}

fn check_authorization(case: &str, node: &Node, expected: &Authorization) -> Result<(), String> {
    let Authorization {
        chain_id,
        address,
        nonce,
        y_parity,
        r,
        s,
        authority,
    } = expected;
    let pairs = [
        ("chain_id", chain_id),
        ("address", address),
        ("nonce", nonce),
        ("y_parity", y_parity),
        ("r", r),
        ("s", s),
        ("authority", authority),
    ];
    pairs
        .into_iter()
        .try_for_each(|(name, value)| same(case, name, plain(&child(node, name)?.value), value))
}

fn check_lists(case: &str, root: &Node, vector: &Transaction) -> Result<(), String> {
    if let Ok(list) = child(root, "access_list") {
        same(
            case,
            "访问项数",
            list.children.len().to_string(),
            &vector.access_list.len().to_string(),
        )?;
        for (entry, expected) in list.children.iter().zip(&vector.access_list) {
            same(
                case,
                "address",
                plain(&child(entry, "address")?.value),
                &expected.address,
            )?;
            let keys: Vec<String> = child(entry, "storage_keys")?
                .children
                .iter()
                .map(|key| plain(&key.value))
                .collect();
            same(case, "storage_keys", keys.join(","), &expected.storage_keys.join(","))?;
        }
    }
    if let Ok(list) = child(root, "authorization_list") {
        same(
            case,
            "授权数",
            list.children.len().to_string(),
            &vector.authorization_list.len().to_string(),
        )?;
        for (entry, expected) in list.children.iter().zip(&vector.authorization_list) {
            check_authorization(case, entry, expected)?;
        }
    }
    if let Ok(list) = child(root, "blob_versioned_hashes") {
        let hashes: Vec<String> = list.children.iter().map(|hash| plain(&hash.value)).collect();
        same(case, "blob", hashes.join(","), &vector.blob_versioned_hashes.join(","))?;
    }
    Ok(())
}

/// 树的每个标量字段都要在向量里,向量的每个字段都要在树里.
fn check_fields(vector: &Transaction, decoded: &Decoded) -> Result<(), String> {
    let root = decoded.root();
    for (name, expected) in &vector.fields {
        same(&vector.case, name, plain(&child(root, name)?.value), expected)?;
    }
    for node in &root.children {
        let name = node.name.as_str();
        if !(vector.fields.contains_key(name) || LISTS.contains(&name) || NOT_IN_VECTORS.contains(&name)) {
            return Err(format!("{} 的 {name} 不在向量里", vector.case));
        }
    }
    check_lists(&vector.case, root, vector)
}

fn check_labels(decoded: &Decoded) -> Result<(), String> {
    let labels: BTreeMap<String, String> = serde_json::from_str(LABELS).map_err(|error| error.to_string())?;
    let visits = decoded.visits();
    match visits.iter().find(|visit| !labels.contains_key(&visit.node.name)) {
        Some(visit) => Err(format!("字段 {} 没有显示文字", visit.node.name)),
        None => Ok(()),
    }
}

fn recover(digest: [u8; 32], signature: &[u8; 65]) -> Result<String, String> {
    let describe = |error: bitcoin::secp256k1::Error| error.to_string();
    let (parity, compact) = signature.split_last().ok_or("签名是空的")?;
    let recovery = ecdsa::RecoveryId::from_i32(i32::from(*parity)).map_err(describe)?;
    let recoverable = ecdsa::RecoverableSignature::from_compact(compact, recovery).map_err(describe)?;
    let key = Secp256k1::verification_only()
        .recover_ecdsa(&Message::from_digest(digest), &recoverable)
        .map_err(describe)?;
    let address = evm_address(&key.serialize()).map_err(|error| error.to_string())?;
    Ok(address.trim_start_matches("0x").to_lowercase())
}

fn check_transaction(vector: &Transaction) -> Result<(), String> {
    let case = &vector.case;
    let payload = bytes(&vector.unsigned)?;
    let decoded = run(&WALKER, &payload).map_err(|error| format!("{case}:{error}"))?;
    check_fields(vector, &decoded)?;
    check_labels(&decoded)?;
    let tx = UnsignedTx::decode(&payload).map_err(|error| format!("{case}:{error}"))?;
    same(case, "未签名编码", hex(&tx.encode()), &vector.unsigned)?;
    let signature: [u8; 65] = bytes(&vector.signature)?
        .try_into()
        .map_err(|_| format!("{case} 的签名不是 65 字节"))?;
    let signed = tx.assemble(&signature).map_err(|error| format!("{case}:{error}"))?;
    same(case, "已签名编码", hex(&signed), &vector.signed)?;
    same(case, "交易哈希", hex(&keccak(&signed)), &vector.hash)?;
    same(case, "发送方", recover(keccak(&payload), &signature)?, &vector.sender)
}

fn check_refused(vector: &Refused) -> Result<(), String> {
    let payload = bytes(&vector.unsigned)?;
    if run(&WALKER, &payload).is_ok() || UnsignedTx::decode(&payload).is_ok() {
        return Err(format!("{} 应该解码失败", vector.case));
    }
    Ok(())
}

fn check_message(vector: &SignedMessage) -> Result<(), String> {
    let payload = bytes(&vector.payload)?;
    let decoded = match vector.kind {
        MessageKind::Transaction => run(&WALKER, &payload)?,
        MessageKind::Personal => run(&PersonalWalker, &payload)?,
    };
    check_labels(&decoded)?;
    let digest = child(decoded.root(), "signing_hash")?;
    same(&vector.case, "signing_hash", plain(&digest.value), &vector.hash)
}

pub fn check(dir: &Path) -> Result<Report, String> {
    let describe = |error: std::io::Error| format!("{}:{error}", dir.display());
    let text = fs::read(dir.join(FILE)).map_err(describe)?;
    let lock = fs::read_to_string(dir.join(LOCK)).map_err(describe)?;
    if locked(&lock)?.get(FILE).map(String::as_str) != Some(sha256(&text).as_str()) {
        return Err(format!("{FILE} 的哈希和 {LOCK} 不符"));
    }
    let vectors: Vectors = serde_json::from_slice(&text).map_err(|error| error.to_string())?;
    vectors.transactions.iter().try_for_each(check_transaction)?;
    let refused = vectors.invalid.iter().chain(&vectors.too_wide);
    refused.clone().try_for_each(check_refused)?;
    vectors.messages.iter().try_for_each(check_message)?;
    Ok(Report {
        transactions: vectors.transactions.len(),
        invalid: refused.count(),
        messages: vectors.messages.len(),
    })
}

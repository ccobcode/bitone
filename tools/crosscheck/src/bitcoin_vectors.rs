//! Bitcoin 的官方向量.PSBT 来自 BIP-174,370,371,sighash 来自 BIP-143,341.
//!
//! 树里的字段和 `bitcoin`,`psbt-v2` 直接解出的字段逐个比较.这里不经过 `bitone-core` 的规整,
//! 所以规整和遍历器出的错都会显出来.向量的来源和 commit 写在数据文件里.

use crate::{locked, sha256};
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::{Parity, PublicKey, XOnlyPublicKey};
use bitcoin::sighash::{EcdsaSighashType, TapSighashType};
use bitcoin::{Amount, Script, ScriptBuf, Sequence, Transaction, TxOut};
use bitone_core::decode::{Decoded, Input, PsbtWalker, Value, decode, display};
use bitone_core::tx::bitcoin::{Sighash, script_for, signature_hash};
use bitone_core::units::decimal_from_be;
use bitone_core::{AccountKind, Chain, Mode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const FILE: &str = "bitcoin.json";
const ROOT: &str = "psbt";
const LABELS: &str = include_str!("../../../crates/bitone-core/assets/field-labels.json");
const LOCK: &str = "vectors.lock";

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Expect {
    Valid,
    Invalid,
}

#[derive(Deserialize)]
struct PsbtVector {
    bip: u32,
    case: String,
    hex: String,
    expect: Expect,
    lock_time: Option<u32>,
}

#[derive(Deserialize)]
struct SegwitVector {
    case: String,
    transaction: String,
    index: usize,
    kind: String,
    sats: u64,
    script: String,
    sighash: String,
}

#[derive(Deserialize)]
struct Spent {
    sats: u64,
    script: String,
}

#[derive(Deserialize)]
struct TaprootCase {
    index: usize,
    hash_type: u8,
    sighash: String,
}

#[derive(Deserialize)]
struct TaprootKey {
    internal_key: String,
    script: String,
}

#[derive(Deserialize)]
struct Bip341 {
    transaction: String,
    spent: Vec<Spent>,
    cases: Vec<TaprootCase>,
    script_pubkeys: Vec<TaprootKey>,
}

#[derive(Deserialize)]
struct Vectors {
    psbt: Vec<PsbtVector>,
    bip143: Vec<SegwitVector>,
    bip341: Bip341,
}

#[derive(Debug, Default)]
pub struct Report {
    pub valid: usize,
    pub invalid: usize,
    pub sighashes: usize,
}

fn bytes(hex: &str) -> Result<Vec<u8>, String> {
    Vec::from_hex(hex).map_err(|error| error.to_string())
}

fn script(hex: &str) -> Result<ScriptBuf, String> {
    bytes(hex).map(ScriptBuf::from_bytes)
}

fn decoded(payload: &[u8]) -> Result<Decoded, String> {
    let input = Input {
        chain: Chain::Bitcoin,
        mode: Some(Mode::Mainnet),
        network: "bitcoin",
        kind: 0,
        paths: &[],
        payload,
    };
    decode(&PsbtWalker, &input).map_err(|failure| failure.to_string())
}

/// 金额比较最小单位,其他字段比较显示值.
fn plain(value: &Value) -> String {
    match value {
        Value::Amount(amount) => decimal_from_be(&amount.raw),
        other => display(other),
    }
}

/// 进入签名原文或由它算出的,父节点叫 `parent`,名字叫 `name` 的全部行.前序交易在 `context` 里,不算.
fn rows(tree: &Decoded, parent: &str, name: &str) -> Vec<String> {
    let visits = tree.visits();
    visits
        .iter()
        .filter(|visit| visit.node.name == name)
        .filter(|visit| visit.parent.is_some_and(|id| visits[id as usize].node.name == parent))
        .filter(|visit| !matches!(visit.node.source.kind(), bitone_core::decode::SourceKind::Context))
        .map(|visit| plain(&visit.node.value))
        .collect()
}

/// 对照工具解出的一笔交易.
struct Fields {
    version: String,
    lock_time: String,
    txids: Vec<String>,
    vouts: Vec<String>,
    sequences: Vec<String>,
    amounts: Vec<String>,
    scripts: Vec<String>,
}

fn sats(amount: Amount) -> String {
    amount.to_sat().to_string()
}

fn asm(script: &Script) -> String {
    script.to_asm_string()
}

fn v0_fields(tx: &Transaction) -> Fields {
    Fields {
        version: tx.version.to_string(),
        lock_time: tx.lock_time.to_consensus_u32().to_string(),
        txids: tx
            .input
            .iter()
            .map(|txin| txin.previous_output.txid.to_string())
            .collect(),
        vouts: tx
            .input
            .iter()
            .map(|txin| txin.previous_output.vout.to_string())
            .collect(),
        sequences: tx
            .input
            .iter()
            .map(|txin| txin.sequence.to_consensus_u32().to_string())
            .collect(),
        amounts: tx.output.iter().map(|out| sats(out.value)).collect(),
        scripts: tx.output.iter().map(|out| asm(&out.script_pubkey)).collect(),
    }
}

fn v2_fields(psbt: &psbt_v2::v2::Psbt) -> Result<Fields, String> {
    let lock_time = psbt.determine_lock_time().map_err(|error| error.to_string())?;
    Ok(Fields {
        version: psbt.global.tx_version.to_string(),
        lock_time: lock_time.to_consensus_u32().to_string(),
        txids: psbt
            .inputs
            .iter()
            .map(|input| input.previous_txid.to_string())
            .collect(),
        vouts: psbt
            .inputs
            .iter()
            .map(|input| input.spent_output_index.to_string())
            .collect(),
        sequences: psbt
            .inputs
            .iter()
            .map(|input| input.sequence.unwrap_or(Sequence::MAX).to_consensus_u32().to_string())
            .collect(),
        amounts: psbt.outputs.iter().map(|output| sats(output.amount)).collect(),
        scripts: psbt.outputs.iter().map(|output| asm(&output.script_pubkey)).collect(),
    })
}

fn same(case: &str, field: &str, tree: Vec<String>, official: &[String]) -> Result<(), String> {
    if tree == official {
        return Ok(());
    }
    Err(format!("{case}:{field} 不一致,树 {tree:?},对照 {official:?}"))
}

fn compare_v0(case: &str, tree: &Decoded, tx: &Transaction) -> Result<(), String> {
    let fields = v0_fields(tx);
    same(case, "version", rows(tree, "unsigned_tx", "version"), &[fields.version])?;
    same(
        case,
        "lock_time",
        rows(tree, "unsigned_tx", "lock_time"),
        &[fields.lock_time],
    )?;
    same(case, "txid", rows(tree, "input", "txid"), &fields.txids)?;
    same(case, "vout", rows(tree, "input", "vout"), &fields.vouts)?;
    same(case, "sequence", rows(tree, "input", "sequence"), &fields.sequences)?;
    same(case, "amount", rows(tree, "output", "amount"), &fields.amounts)?;
    same(case, "script", rows(tree, "output", "script"), &fields.scripts)
}

fn compare_v2(case: &str, tree: &Decoded, psbt: &psbt_v2::v2::Psbt) -> Result<(), String> {
    let fields = v2_fields(psbt)?;
    same(case, "version", rows(tree, "global", "version"), &[fields.version])?;
    same(
        case,
        "lock_time",
        rows(tree, "global", "lock_time"),
        &[fields.lock_time],
    )?;
    same(case, "txid", rows(tree, "psbt_input", "previous_txid"), &fields.txids)?;
    same(case, "vout", rows(tree, "psbt_input", "output_index"), &fields.vouts)?;
    same(
        case,
        "sequence",
        rows(tree, "psbt_input", "sequence"),
        &fields.sequences,
    )?;
    same(case, "amount", rows(tree, "psbt_output", "amount"), &fields.amounts)?;
    same(case, "script", rows(tree, "psbt_output", "script"), &fields.scripts)
}

fn check_valid(vector: &PsbtVector, payload: &[u8]) -> Result<(), String> {
    let case = format!("BIP-{} {}", vector.bip, vector.case);
    let tree = decoded(payload).map_err(|reason| format!("{case}:{reason}"))?;
    let unknown = if vector.bip == 370 {
        let official = psbt_v2::v2::Psbt::deserialize(payload).map_err(|error| format!("{case}:{error}"))?;
        compare_v2(&case, &tree, &official)?;
        let inputs = official.inputs.iter().map(|input| input.unknowns.len());
        let outputs = official.outputs.iter().map(|output| output.unknowns.len());
        official.global.unknowns.len() + inputs.chain(outputs).sum::<usize>()
    } else {
        let official = Psbt::deserialize(payload).map_err(|error| format!("{case}:{error}"))?;
        compare_v0(&case, &tree, &official.unsigned_tx)?;
        let inputs = official.inputs.iter().map(|input| input.unknown.len());
        let outputs = official.outputs.iter().map(|output| output.unknown.len());
        official.unknown.len() + inputs.chain(outputs).sum::<usize>()
    };
    if let Some(lock_time) = vector.lock_time {
        same(
            &case,
            "lock_time",
            rows(&tree, "global", "lock_time"),
            &[lock_time.to_string()],
        )?;
    }
    let found = tree
        .visits()
        .iter()
        .filter(|visit| visit.node.name == "unknown_key")
        .count();
    if found != unknown {
        return Err(format!("{case}:树里有 {found} 个未知键,对照有 {unknown} 个"));
    }
    let labels: BTreeMap<String, String> = serde_json::from_str(LABELS).map_err(|error| error.to_string())?;
    match tree
        .visits()
        .iter()
        .find(|visit| !labels.contains_key(&visit.node.name))
    {
        Some(visit) => Err(format!("{case}:{} 没有显示文字", visit.node.name)),
        None => Ok(()),
    }
}

fn check_psbt(vector: &PsbtVector) -> Result<(), String> {
    let payload = bytes(&vector.hex)?;
    match vector.expect {
        Expect::Valid => check_valid(vector, &payload),
        // 不是 PSBT 的载荷解码成 `opaque` 根节点,核对时拒绝.
        Expect::Invalid => match decoded(&payload) {
            Ok(tree) if tree.root().name == ROOT => Err(format!("BIP-{} {}:应该不合法", vector.bip, vector.case)),
            Ok(_) | Err(_) => Ok(()),
        },
    }
}

fn check_segwit(vector: &SegwitVector) -> Result<(), String> {
    let tx: Transaction = deserialize_hex(&vector.transaction).map_err(|error| error.to_string())?;
    let kind = AccountKind::from_id(&vector.kind).ok_or("未知的地址类型")?;
    let spent = TxOut {
        value: Amount::from_sat(vector.sats),
        script_pubkey: script(&vector.script)?,
    };
    let mut prevouts = vec![TxOut::NULL; tx.input.len()];
    prevouts[vector.index] = spent.clone();
    let digest = signature_hash(
        &tx,
        vector.index,
        &prevouts,
        kind,
        &spent.script_pubkey,
        Sighash::Ecdsa(EcdsaSighashType::All),
    )
    .map_err(|reason| format!("{}:{reason}", vector.case))?;
    same(
        &vector.case,
        "sighash",
        vec![digest.to_lower_hex_string()],
        std::slice::from_ref(&vector.sighash),
    )
}

fn check_taproot(vectors: &Bip341) -> Result<usize, String> {
    let tx: Transaction = deserialize_hex(&vectors.transaction).map_err(|error| error.to_string())?;
    let prevouts = vectors
        .spent
        .iter()
        .map(|spent| {
            Ok(TxOut {
                value: Amount::from_sat(spent.sats),
                script_pubkey: script(&spent.script)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    for case in &vectors.cases {
        let ty = TapSighashType::from_consensus_u8(case.hash_type).map_err(|error| error.to_string())?;
        let code = &prevouts[case.index].script_pubkey;
        let digest = signature_hash(
            &tx,
            case.index,
            &prevouts,
            AccountKind::Taproot,
            code,
            Sighash::Taproot(ty),
        )
        .map_err(|reason| reason.to_string())?;
        same(
            "BIP-341",
            "sighash",
            vec![digest.to_lower_hex_string()],
            std::slice::from_ref(&case.sighash),
        )?;
    }
    for key in &vectors.script_pubkeys {
        let internal = XOnlyPublicKey::from_slice(&bytes(&key.internal_key)?).map_err(|error| error.to_string())?;
        let public = PublicKey::from_x_only_public_key(internal, Parity::Even);
        let derived = script_for(AccountKind::Taproot, &public).map_err(|reason| reason.to_string())?;
        same(
            "BIP-341",
            "scriptPubKey",
            vec![derived.to_hex_string()],
            std::slice::from_ref(&key.script),
        )?;
    }
    Ok(vectors.cases.len())
}

/// 校验数据文件的哈希,再逐个向量对照.
pub fn check(dir: &Path) -> Result<Report, String> {
    let describe = |error: std::io::Error| format!("{}:{error}", dir.display());
    let entries = locked(&fs::read_to_string(dir.join(LOCK)).map_err(describe)?)?;
    let data = fs::read(dir.join(FILE)).map_err(describe)?;
    if entries.get(FILE).map(String::as_str) != Some(sha256(&data).as_str()) {
        return Err(format!("{FILE} 的哈希和 {LOCK} 不符"));
    }
    let vectors: Vectors = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    let mut report = Report::default();
    for vector in &vectors.psbt {
        check_psbt(vector)?;
        match vector.expect {
            Expect::Valid => report.valid += 1,
            Expect::Invalid => report.invalid += 1,
        }
    }
    for vector in &vectors.bip143 {
        check_segwit(vector)?;
        report.sighashes += 1;
    }
    report.sighashes += check_taproot(&vectors.bip341)?;
    Ok(report)
}

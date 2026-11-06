//! Solana 的对照向量,由 `codama/generate.mjs` 从 schemas/solana/ 生成.
//!
//! 运行时用 bitone-core 的遍历器解码,所以字段的期望值来自 @codama/dynamic-parsers.
//! 每条指令都放进 legacy,v0,v1 三种消息,检查 C1,C2,C4,字段逐个比较.C3 由 `tests/architecture.rs` 检查.

use crate::{locked, sha256};
use bitcoin::hex::FromHex;
use bitone_core::chains::solana::decode::{
    Instruction, Message, SolanaWalker, V1Config, Version, idl_actions, idl_sources,
};
use bitone_core::chains::solana::policy::{SolanaFacts, assess_solana};
use bitone_core::decode::{Coin, Decoded, Failure, Input, Node, Value, decode};
use bitone_core::review::Review;
use bitone_core::units::decimal_from_be;
use bitone_core::{Chain, Mode};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const FILE: &str = "solana.json";
const LABELS: &str = include_str!("../../../crates/bitone-core/assets/field-labels.json");
const IDL: &str = include_str!("../../../crates/bitone-core/assets/solana/idl.json");
const LOCK: &str = "vectors.lock";
const SCHEMA_LOCK: &str = "lock.toml";
const SCHEMAS: &str = "../../../schemas";
const NONE: &str = "无";
/// 列表和元组的元素,枚举的分支:名字跟随所在的字段.
const NAMELESS: [&str; 2] = ["item", "variant"];
const INSTRUCTION: &str = "instruction";
const ACCOUNTS: &str = "accounts";
const DATA: &str = "data";
const INSTRUCTION_NAME: &str = "instruction_name";
const ROWS_ONLY_IN_THE_TREE: [&str; 2] = ["role", "extra_account"];
const COIN: Coin = Coin {
    unit: "SOL",
    decimals: 9,
};
const WALKER: SolanaWalker = SolanaWalker {
    coin: Some(COIN),
    hint: None,
};
const PAYER: [u8; 32] = [0x77; 32];

#[derive(Deserialize)]
struct Vector {
    program: String,
    instruction: String,
    data: String,
    accounts: Vec<String>,
    #[serde(default)]
    inner: Vec<String>,
    fields: Vec<(String, Option<String>)>,
}

#[derive(Deserialize)]
struct Vectors {
    instructions: Vec<Vector>,
}

#[derive(Deserialize)]
struct IdlProgram {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct Idl {
    programs: Vec<IdlProgram>,
}

#[derive(Debug)]
pub(crate) struct Report {
    pub programs: usize,
    pub instructions: usize,
    pub messages: usize,
    pub fields: usize,
    pub refused: usize,
}

fn program_key(text: &str) -> Result<[u8; 32], String> {
    let bytes = bitcoin::base58::decode(text).map_err(|error| error.to_string())?;
    bytes.try_into().map_err(|_| format!("{text} 不是 32 字节"))
}

fn message(vector: &Vector, version: Version, accounts: usize) -> Result<Message, String> {
    let data = Vec::from_hex(&vector.data).map_err(|error| error.to_string())?;
    let mut keys = vec![PAYER];
    keys.extend((0..accounts).map(|index| [index as u8 + 1; 32]));
    keys.push(program_key(&vector.program)?);
    Ok(Message {
        version,
        required_signatures: 1,
        readonly_signed: 0,
        readonly_unsigned: 0,
        instructions: vec![Instruction {
            program: keys.len() as u8 - 1,
            accounts: (1..=accounts as u8).collect(),
            data,
        }],
        keys,
        blockhash: [9; 32],
        lookups: Vec::new(),
    })
}

fn decoded(payload: &[u8]) -> Result<Decoded, Failure> {
    decode(
        &WALKER,
        &Input {
            chain: Chain::Solana,
            mode: Some(Mode::Mainnet),
            network: "solana",
            kind: 0,
            paths: &[],
            payload,
            signer: None,
        },
    )
}

/// 向量里的值是对照工具的原始值,金额不带小数位和单位.
fn raw(value: &Value) -> Option<String> {
    match value {
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Int(number) => Some(number.to_string()),
        Value::Uint(bytes) => Some(decimal_from_be(bytes)),
        Value::Amount(amount) => Some(decimal_from_be(&amount.raw)),
        Value::Address(text) | Value::Text(text) | Value::Enum(text) => Some(text.clone()),
        Value::Time(millis) => Some((millis / 1000).to_string()),
        Value::Hash(bytes) | Value::Bytes(bytes) | Value::Script(bytes) | Value::Opaque(bytes) => {
            Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        }
        Value::Struct | Value::List => None,
    }
}

fn child<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    node.children.iter().find(|child| child.name == name)
}

/// 指令数据的叶子,前序.`batch` 的内部指令接着展开.
fn leaves(node: &Node, inherited: &str, fields: &mut Vec<(String, String)>) {
    if node.name == INSTRUCTION {
        if let Some(data) = child(node, DATA) {
            data.children.iter().for_each(|item| leaves(item, inherited, fields));
        }
        return;
    }
    let name = if NAMELESS.contains(&node.name.as_str()) {
        inherited
    } else {
        node.name.as_str()
    };
    if node.children.is_empty() {
        fields.extend(raw(&node.value).map(|value| (name.to_string(), value)));
    } else {
        node.children.iter().for_each(|item| leaves(item, name, fields));
    }
}

/// 全部账户位的名字,`batch` 的内部指令接着展开.
fn account_names(instruction: &Node, names: &mut Vec<String>) {
    if let Some(accounts) = child(instruction, ACCOUNTS) {
        let rows = accounts.children.iter();
        names.extend(
            rows.filter(|row| !ROWS_ONLY_IN_THE_TREE.contains(&row.name.as_str()))
                .map(|row| row.name.clone()),
        );
    }
    if let Some(data) = child(instruction, DATA) {
        for row in &data.children {
            for item in &row.children {
                if let Some(inner) = child(item, INSTRUCTION) {
                    account_names(inner, names);
                }
            }
        }
    }
}

fn inner_names(instruction: &Node) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(data) = child(instruction, DATA) {
        for row in &data.children {
            for item in &row.children {
                let inner = child(item, INSTRUCTION).and_then(|inner| child(inner, INSTRUCTION_NAME));
                names.extend(inner.and_then(|node| raw(&node.value)));
            }
        }
    }
    names
}

fn top_instruction(decoded: &Decoded) -> Option<&Node> {
    child(child(decoded.root(), "instructions")?, INSTRUCTION)
}

/// 字段名都有显示文字,运行时缺了界面只能显示原名.
fn check_labels(decoded: &Decoded) -> Result<(), String> {
    let labels: BTreeMap<String, String> = serde_json::from_str(LABELS).map_err(|error| error.to_string())?;
    let chain = [Chain::Solana.id(), "."].concat();
    let visits = decoded.visits();
    let missing = visits.iter().find(|visit| {
        let name = &visit.node.name;
        !labels.contains_key(name) && !labels.contains_key(&[chain.as_str(), name].concat())
    });
    match missing {
        Some(visit) => Err(format!("字段 {} 没有显示文字", visit.node.name)),
        None => Ok(()),
    }
}

fn check_tree(vector: &Vector, decoded: &Decoded) -> Result<usize, String> {
    let case = &vector.instruction;
    let instruction = top_instruction(decoded).ok_or_else(|| format!("{case} 没有指令节点"))?;
    let name = child(instruction, INSTRUCTION_NAME).and_then(|node| raw(&node.value));
    if name.as_deref() != Some(case) {
        return Err(format!("{case} 解码成了 {name:?}"));
    }
    let visits = decoded.visits();
    if let Some(visit) = visits.iter().find(|visit| matches!(visit.node.value, Value::Opaque(_))) {
        return Err(format!("{case} 有 opaque 节点 {}", visit.node.name));
    }
    let mut fields = Vec::new();
    if let Some(data) = child(instruction, DATA) {
        data.children.iter().for_each(|row| leaves(row, "", &mut fields));
    }
    let expected: Vec<(String, String)> = vector
        .fields
        .iter()
        .map(|(name, value)| (name.clone(), value.clone().unwrap_or_else(|| NONE.to_string())))
        .collect();
    if fields != expected {
        return Err(format!("{case} 的字段不同:\n树  {fields:?}\n向量 {expected:?}"));
    }
    let mut names = Vec::new();
    account_names(instruction, &mut names);
    if names != vector.accounts {
        return Err(format!("{case} 的账户不同:{names:?} {:?}", vector.accounts));
    }
    if inner_names(instruction) != vector.inner {
        return Err(format!("{case} 的内部指令不同"));
    }
    check_labels(decoded)?;
    // C4:每个节点都进入渲染列表.
    let review = Review::assessed(decoded, assess_solana(decoded, &SolanaFacts::default()));
    if review.forbidden.is_some() || review.field_count != decoded.node_count() {
        return Err(format!("{case} 的渲染列表和解码树不一致"));
    }
    Ok(fields.len())
}

/// 少一个账户时程序不会执行,解码失败.少一个字节时不能崩溃,能解码就要过 C1,C2.
fn check_broken(vector: &Vector) -> Result<usize, String> {
    let mut refused = 0;
    if let Some(fewer) = vector.accounts.len().checked_sub(1) {
        let payload = message(vector, Version::Legacy, fewer)?.encode();
        if decoded(&payload).is_ok() {
            return Err(format!("{} 少一个账户还能解码", vector.instruction));
        }
        refused += 1;
    }
    let mut short = message(vector, Version::Legacy, vector.accounts.len())?;
    if short.instructions[0].data.pop().is_some() {
        // 解码成功或失败都可以,`decoded` 里已经过了 C1,C2.
        let _ = decoded(&short.encode());
    }
    Ok(refused)
}

fn versions() -> [Version; 3] {
    let config = V1Config {
        priority_fee: Some(5000),
        compute_unit_limit: Some(200_000),
        loaded_accounts_data_size_limit: Some(65_536),
        heap_size: Some(32_768),
    };
    [Version::Legacy, Version::V0, Version::V1(config)]
}

/// IDL 表的来源和 `schemas/lock.toml` 一致,文件也没有改过.
fn check_schemas(root: &Path) -> Result<(), String> {
    let text = fs::read_to_string(root.join(SCHEMA_LOCK)).map_err(|error| error.to_string())?;
    let mut file = None;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let quoted = |key: &str| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(" = \""))
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_string)
        };
        file = quoted("file").or(file);
        let Some(digest) = quoted("sha256") else {
            continue;
        };
        let Some(name) = file.take().filter(|name| idl_sources().contains_key(name)) else {
            continue;
        };
        let bytes = fs::read(root.join(&name)).map_err(|error| error.to_string())?;
        if sha256(&bytes) != digest {
            return Err(format!("{name} 的哈希和 {SCHEMA_LOCK} 不符"));
        }
        if idl_sources().get(&name) != Some(&digest) {
            return Err(format!("{name} 改过以后没有重新生成 IDL 表"));
        }
        seen.insert(name);
    }
    if seen.len() != idl_sources().len() {
        return Err(format!("{SCHEMA_LOCK} 没有列出全部 IDL"));
    }
    Ok(())
}

pub(crate) fn check(dir: &Path) -> Result<Report, String> {
    let describe = |error: std::io::Error| error.to_string();
    let bytes = fs::read(dir.join(FILE)).map_err(describe)?;
    let lock = locked(&fs::read_to_string(dir.join(LOCK)).map_err(describe)?)?;
    if lock.get(FILE) != Some(&sha256(&bytes)) {
        return Err(format!("{FILE} 的哈希和 {LOCK} 不符"));
    }
    check_schemas(&dir.join(SCHEMAS))?;
    let vectors: Vectors = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut report = Report {
        programs: 0,
        instructions: vectors.instructions.len(),
        messages: 0,
        fields: 0,
        refused: 0,
    };
    let mut programs = BTreeSet::new();
    for vector in &vectors.instructions {
        programs.insert(vector.program.as_str());
        for version in versions() {
            let payload = message(vector, version, vector.accounts.len())?.encode();
            let tree = decoded(&payload).map_err(|failure| format!("{}:{failure}", vector.instruction))?;
            report.fields += check_tree(vector, &tree)?;
            report.messages += 1;
        }
        report.refused += check_broken(vector)?;
    }
    report.programs = programs.len();
    // IDL 的每条指令都有向量.
    if report.instructions != idl_actions().len() {
        return Err(format!(
            "向量有 {} 条指令,IDL 有 {} 条",
            report.instructions,
            idl_actions().len()
        ));
    }
    Ok(report)
}

/// 模糊测试的种子.`walk_idl` 的格式见它的目标文件.
#[derive(Default)]
pub(crate) struct Seeds {
    pub messages: Vec<Vec<u8>>,
    pub instructions: Vec<Vec<u8>>,
}

pub(crate) fn seeds(dir: &Path) -> Result<Seeds, String> {
    let bytes = fs::read(dir.join(FILE)).map_err(|error| error.to_string())?;
    let vectors: Vectors = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let idl: Idl = serde_json::from_str(IDL).map_err(|error| error.to_string())?;
    let ids: Vec<&String> = idl.programs.iter().flat_map(|program| &program.ids).collect();
    let mut seeds = Seeds::default();
    for vector in &vectors.instructions {
        for version in versions() {
            seeds
                .messages
                .push(message(vector, version, vector.accounts.len())?.encode());
        }
        let program = ids
            .iter()
            .position(|id| **id == vector.program)
            .ok_or_else(|| format!("IDL 没有 {}", vector.program))?;
        let mut seed = vec![program as u8, vector.accounts.len() as u8];
        seed.extend(Vec::from_hex(&vector.data).map_err(|error| error.to_string())?);
        seeds.instructions.push(seed);
    }
    Ok(seeds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer() -> Vector {
        Vector {
            program: "11111111111111111111111111111111".to_string(),
            instruction: "transferSol".to_string(),
            data: "020000000500000000000000".to_string(),
            accounts: vec!["source".to_string(), "destination".to_string()],
            inner: Vec::new(),
            fields: vec![
                ("discriminator".to_string(), Some("2".to_string())),
                ("amount".to_string(), Some("5".to_string())),
            ],
        }
    }

    fn checked(vector: &Vector) -> Result<usize, String> {
        let payload = message(vector, Version::Legacy, vector.accounts.len())?.encode();
        check_tree(vector, &decoded(&payload).map_err(|failure| failure.to_string())?)
    }

    #[test]
    fn a_matching_vector_passes() {
        assert_eq!(checked(&transfer()), Ok(2));
    }

    #[test]
    fn a_wrong_field_is_caught() {
        let mut vector = transfer();
        vector.fields[1].1 = Some("6".to_string());
        assert!(checked(&vector).unwrap_err().contains("字段不同"));
    }

    #[test]
    fn a_wrong_field_name_is_caught() {
        let mut vector = transfer();
        vector.fields[1].0 = "lamports".to_string();
        assert!(checked(&vector).is_err());
    }

    #[test]
    fn a_wrong_account_name_is_caught() {
        let mut vector = transfer();
        vector.accounts[1] = "recipient".to_string();
        assert!(checked(&vector).unwrap_err().contains("账户不同"));
    }

    #[test]
    fn a_wrong_instruction_is_caught() {
        let mut vector = transfer();
        vector.instruction = "assign".to_string();
        assert!(checked(&vector).is_err());
    }

    #[test]
    fn trailing_bytes_are_not_a_clean_decode() {
        let mut vector = transfer();
        vector.data.push_str("aa");
        assert!(checked(&vector).unwrap_err().contains("opaque"));
    }
}

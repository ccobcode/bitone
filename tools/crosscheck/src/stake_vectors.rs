//! Stake 的两种账户布局.向量里的指令由官方 `solana-stake-interface` 的构建函数生成,
//! `stake_interface.rs` 检查这份文件和构建函数一致,这里检查遍历器读出的账户名和地址.
//! 见 docs/10-tx-decoding.md 第 5.3 节.

use crate::{locked, sha256};
use bitcoin::hex::FromHex;
use bitone_core::decode::solana::SolanaWalker;
use bitone_core::decode::{Coin, Decoded, Input, Node, Value, decode, display};
use bitone_core::{Chain, Mode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub(crate) const FILE: &str = "stake.json";
const LOCK: &str = "vectors.lock";
const INSTRUCTION: &str = "instruction";
const INSTRUCTION_NAME: &str = "instruction_name";
const ACCOUNTS: &str = "accounts";
const WALKER: SolanaWalker = SolanaWalker {
    coin: Some(Coin {
        unit: "SOL",
        decimals: 9,
    }),
    hint: None,
};

/// `accounts` 是构建函数给出的账户,名字来自调用时传入的角色,不来自 schema.
/// `counterparty` 是这条指令在第一屏的对方.
#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct Case {
    pub instruction: String,
    pub layout: String,
    pub builder: String,
    pub message: String,
    pub accounts: Vec<(String, String)>,
    pub counterparty: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct Vectors {
    pub generator: String,
    pub cases: Vec<Case>,
}

#[derive(Debug)]
pub(crate) struct Report {
    pub cases: usize,
    pub layouts: usize,
}

fn run(payload: &[u8]) -> Result<Decoded, String> {
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
    .map_err(|failure| failure.to_string())
}

fn child<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    node.children.iter().find(|child| child.name == name)
}

fn check_case(case: &Case) -> Result<(), String> {
    let label = [case.instruction.as_str(), case.layout.as_str()].join(" ");
    let payload = Vec::from_hex(&case.message).map_err(|error| error.to_string())?;
    let decoded = run(&payload).map_err(|reason| format!("{label}:{reason}"))?;
    let visits = decoded.visits();
    if visits.iter().any(|visit| matches!(visit.node.value, Value::Opaque(_))) {
        return Err(format!("{label} 有 opaque 节点"));
    }
    let instruction = visits
        .iter()
        .find(|visit| visit.node.name == INSTRUCTION)
        .ok_or_else(|| format!("{label} 没有指令节点"))?
        .node;
    let name = child(instruction, INSTRUCTION_NAME).map(|node| display(&node.value));
    if name.as_deref() != Some(case.instruction.as_str()) {
        return Err(format!("{label} 解码成了 {name:?}"));
    }
    let rows = child(instruction, ACCOUNTS).ok_or_else(|| format!("{label} 没有账户"))?;
    let found: Vec<(String, String)> = rows
        .children
        .iter()
        .map(|row| (row.name.clone(), display(&row.value)))
        .collect();
    if found != case.accounts {
        return Err(format!("{label} 的账户不同:\n树  {found:?}\n向量 {:?}", case.accounts));
    }
    Ok(())
}

pub(crate) fn load(dir: &Path) -> Result<Vectors, String> {
    let describe = |error: std::io::Error| error.to_string();
    let bytes = fs::read(dir.join(FILE)).map_err(describe)?;
    let lock = locked(&fs::read_to_string(dir.join(LOCK)).map_err(describe)?)?;
    if lock.get(FILE) != Some(&sha256(&bytes)) {
        return Err(format!("{FILE} 的哈希和 {LOCK} 不符"));
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

pub(crate) fn check(dir: &Path) -> Result<Report, String> {
    let vectors = load(dir)?;
    vectors.cases.iter().try_for_each(check_case)?;
    let layouts: BTreeSet<&str> = vectors.cases.iter().map(|case| case.layout.as_str()).collect();
    Ok(Report {
        cases: vectors.cases.len(),
        layouts: layouts.len(),
    })
}

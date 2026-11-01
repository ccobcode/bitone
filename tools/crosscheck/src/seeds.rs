//! 把入库的向量写成模糊测试的种子,每个目标一个目录,见 docs/10-tx-decoding.md 第 8.3 节.
//! 不合法的载荷也写进去,它们离解码器的边界最近.

use crate::{sha256, solana_vectors};
use bitcoin::hex::FromHex;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// 向量文件里 `list` 这一列的 `field` 是 `target` 的种子,都是十六进制.
const HEX_SOURCES: [(&str, &str, &str, &str); 11] = [
    ("decode_evm", "evm.json", "transactions", "unsigned"),
    ("decode_evm", "evm.json", "invalid", "unsigned"),
    ("decode_evm", "evm.json", "too_wide", "unsigned"),
    ("decode_evm", "evm.json", "messages", "payload"),
    ("decode_tron", "tron.json", "transactions", "raw"),
    ("decode_tron", "tron.json", "refused", "raw"),
    ("decode_psbt", "bitcoin.json", "psbt", "hex"),
    ("decode_solana", "stake.json", "cases", "message"),
    ("decode_abi", "evm-messages.json", "calls", "data"),
    ("decode_abi", "evm-messages.json", "irregular", "data"),
    ("decode_abi", "tron-messages.json", "calls", "data"),
];
const TYPED_FILE: &str = "evm-messages.json";
const TYPED_LIST: &str = "typed";
const TYPED_FIELD: &str = "payload";

fn write(dir: &Path, target: &str, seed: &[u8]) -> Result<(), String> {
    let folder = dir.join(target);
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    fs::write(folder.join(sha256(seed)), seed).map_err(|error| error.to_string())
}

/// 文件里一列的文字.列表或字段不存在是错误,否则向量改了格式之后目标会悄悄少了种子.
fn column(vectors: &Path, file: &str, list: &str, field: &str) -> Result<Vec<String>, String> {
    let bytes = fs::read(vectors.join(file)).map_err(|error| format!("{file}:{error}"))?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|error| format!("{file}:{error}"))?;
    let rows = root[list].as_array().ok_or_else(|| format!("{file} 没有 {list}"))?;
    let cell = |row: &Value| row[field].as_str().map(str::to_string);
    rows.iter()
        .map(|row| cell(row).ok_or_else(|| format!("{file} 的 {list} 没有 {field}")))
        .collect()
}

/// `crosscheck seeds <目录>`.
pub(crate) fn run(vectors: &Path, dir: &Path) -> Result<(), String> {
    for (target, file, list, field) in HEX_SOURCES {
        for text in column(vectors, file, list, field)? {
            let seed = Vec::from_hex(&text).map_err(|error| format!("{file} 的 {list}:{error}"))?;
            write(dir, target, &seed)?;
        }
    }
    for text in column(vectors, TYPED_FILE, TYPED_LIST, TYPED_FIELD)? {
        write(dir, "decode_typed_data", text.as_bytes())?;
    }
    let solana = solana_vectors::seeds(vectors)?;
    for seed in &solana.messages {
        write(dir, "decode_solana", seed)?;
    }
    for seed in &solana.instructions {
        write(dir, "walk_idl", seed)?;
    }
    Ok(())
}

//! 语料检查和官方向量对照.只读入库的快照,不访问网络,见 docs/10-tx-decoding.md 第 8 节.
//!
//! `corpus/corpus.lock`,`vectors/vectors.lock` 每行是 `SHA-256 文件名`.语料是空的时候只输出空报告.

mod bitcoin_vectors;
mod evm_vectors;

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;
use std::{fs, io};

const CORPUS: &str = "corpus";
const LOCK: &str = "corpus.lock";
const VECTORS: &str = "vectors";
const SAMPLES: &str = "jsonl";

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::new(), |mut hex, byte| {
        write!(hex, "{byte:02x}").expect("写入内存不会失败");
        hex
    })
}

pub(crate) fn locked(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut entries = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (digest, file) = line.split_once(' ').ok_or_else(|| format!("{LOCK} 的行无效:{line}"))?;
        if entries.insert(file.trim().to_string(), digest.to_string()).is_some() {
            return Err(format!("{LOCK} 重复列出 {file}"));
        }
    }
    Ok(entries)
}

/// 校验哈希,返回每个文件的样本数.没有列在锁文件里的样本也是错误.
fn check(dir: &Path) -> Result<BTreeMap<String, usize>, String> {
    let describe = |error: io::Error| format!("{}:{error}", dir.display());
    let entries = locked(&fs::read_to_string(dir.join(LOCK)).map_err(describe)?)?;
    for item in fs::read_dir(dir).map_err(describe)? {
        let path = item.map_err(describe)?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.extension().is_some_and(|ext| ext == SAMPLES) && !entries.contains_key(&name) {
            return Err(format!("{name} 没有列在 {LOCK} 里"));
        }
    }
    let mut counts = BTreeMap::new();
    for (file, digest) in entries {
        let bytes = fs::read(dir.join(&file)).map_err(describe)?;
        if sha256(&bytes) != digest {
            return Err(format!("{file} 的哈希和 {LOCK} 不符"));
        }
        let lines = bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty());
        counts.insert(file, lines.count());
    }
    Ok(counts)
}

fn run() -> Result<(), String> {
    let counts = check(Path::new(CORPUS))?;
    println!("语料文件 {} 个", counts.len());
    counts
        .iter()
        .for_each(|(file, samples)| println!("{file}:{samples} 个样本"));
    let report = bitcoin_vectors::check(Path::new(VECTORS))?;
    println!(
        "Bitcoin 向量:{} 个合法 PSBT,{} 个不合法 PSBT,{} 个 sighash",
        report.valid, report.invalid, report.sighashes
    );
    let report = evm_vectors::check(Path::new(VECTORS))?;
    println!(
        "EVM 向量:{} 笔交易,{} 个不合法载荷,{} 条消息",
        report.transactions, report.invalid, report.messages
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn corpus(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crosscheck-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    #[test]
    fn digest_is_lower_hex() {
        assert_eq!(sha256(b""), EMPTY_SHA256);
    }

    #[test]
    fn the_checked_in_corpus_is_consistent() {
        check(&Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS)).unwrap();
    }

    #[test]
    fn the_official_bitcoin_vectors_hold() {
        let report = bitcoin_vectors::check(&Path::new(env!("CARGO_MANIFEST_DIR")).join(VECTORS)).unwrap();
        assert!(report.valid > 0 && report.invalid > 0 && report.sighashes > 0);
    }

    #[test]
    fn the_evm_vectors_hold() {
        let report = evm_vectors::check(&Path::new(env!("CARGO_MANIFEST_DIR")).join(VECTORS)).unwrap();
        assert!(report.transactions > 0 && report.invalid > 0 && report.messages > 0);
    }

    #[test]
    fn an_empty_corpus_passes() {
        assert!(check(&corpus("empty", &[(LOCK, "")])).unwrap().is_empty());
    }

    #[test]
    fn samples_are_counted_by_line() {
        let body = "{}\n{}\n";
        let lock = format!("{} evm.jsonl\n", sha256(body.as_bytes()));
        let counts = check(&corpus("count", &[(LOCK, &lock), ("evm.jsonl", body)])).unwrap();
        assert_eq!(counts["evm.jsonl"], 2);
    }

    #[test]
    fn a_changed_file_fails() {
        let lock = format!("{EMPTY_SHA256} evm.jsonl\n");
        assert!(check(&corpus("changed", &[(LOCK, &lock), ("evm.jsonl", "{}\n")])).is_err());
    }

    #[test]
    fn an_unlisted_sample_fails() {
        assert!(check(&corpus("unlisted", &[(LOCK, ""), ("evm.jsonl", "{}\n")])).is_err());
    }

    #[test]
    fn a_listed_file_must_exist() {
        let lock = format!("{EMPTY_SHA256} evm.jsonl\n");
        assert!(check(&corpus("missing", &[(LOCK, &lock)])).is_err());
    }

    #[test]
    fn a_file_listed_twice_fails() {
        assert!(locked("a x.jsonl\nb x.jsonl\n").is_err());
        assert!(locked("no-separator").is_err());
    }
}

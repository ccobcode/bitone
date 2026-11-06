//! 签名的分层约定.`sign.rs` 只管公共流程和按链分发,每条链一个文件,文件之间互不引用.
//! 加一条链只加一个文件,再在 `sign.rs` 接一行.

use std::fs;
use std::path::{Path, PathBuf};

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 去掉测试模块.测试可以写具体的链.
fn code(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    text.split("#[cfg(test)]").next().unwrap().to_string()
}

fn chains() -> Vec<(String, String)> {
    let found: Vec<(String, String)> = fs::read_dir(source().join("sign"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .map(|path| (path.file_stem().unwrap().to_string_lossy().into_owned(), code(&path)))
        .collect();
    assert!(found.len() >= 4, "{found:?}");
    found
}

#[test]
fn every_chain_signs_in_its_own_file() {
    let shared = code(&source().join("sign.rs"));
    for (chain, _) in chains() {
        assert!(
            !shared.contains(&format!("fn sign_{chain}")),
            "sign.rs 不能实现 {chain}"
        );
        assert!(
            !shared.contains(&format!("chains::{chain}")),
            "sign.rs 不能引用 chains::{chain}"
        );
    }
}

#[test]
fn only_the_dispatcher_matches_on_the_chain() {
    assert!(code(&source().join("sign.rs")).contains("match network.chain()"));
    for (chain, text) in chains() {
        assert!(!text.contains("match network.chain()"), "{chain}");
    }
}

#[test]
fn chains_do_not_reach_into_each_other() {
    let all = chains();
    for (chain, text) in &all {
        for (other, _) in all.iter().filter(|(other, _)| other != chain) {
            for pattern in [format!("super::{other}"), format!("chains::{other}")] {
                assert!(!text.contains(&pattern), "{chain} 不能引用 {pattern}");
            }
        }
    }
}

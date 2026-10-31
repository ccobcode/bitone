//! 网关的分层约定.依赖只能向下:`api`,`follow` 用 `chains`,`chains` 用 `upstream`.
//! 每条链一个目录,目录之间互不引用,共享层不出现具体的链,加一条链只加一个目录.

use std::fs;
use std::path::{Path, PathBuf};

const CHAIN_NAMES: [&str; 4] = ["Chain::Bitcoin", "Chain::Evm", "Chain::Tron", "Chain::Solana"];

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 目录下的全部 Rust 文件,去掉测试模块.测试可以写具体的链.
fn files(path: &Path) -> Vec<(PathBuf, String)> {
    if path.is_file() {
        let text = fs::read_to_string(path).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap().to_string();
        return vec![(path.to_path_buf(), code)];
    }
    let entries = fs::read_dir(path).unwrap();
    entries.flat_map(|entry| files(&entry.unwrap().path())).collect()
}

fn forbid(paths: &[&str], patterns: &[&str]) {
    for path in paths {
        for (file, code) in files(&source().join(path)) {
            for pattern in patterns {
                assert!(!code.contains(pattern), "{} 不能出现 {pattern}", file.display());
            }
        }
    }
}

#[test]
fn shared_layers_do_not_name_a_chain() {
    let shared = ["api", "upstream", "follow.rs", "config.rs", "prices.rs", "lib.rs"];
    forbid(&shared, &CHAIN_NAMES);
}

#[test]
fn only_the_registry_matches_on_the_chain() {
    let root = source().join("chains");
    for (file, code) in files(&root) {
        if file != root.join("mod.rs") {
            assert!(!code.contains("match network.chain()"), "{}", file.display());
        }
    }
}

#[test]
fn dependencies_only_point_down() {
    forbid(&["upstream"], &["crate::chains", "crate::api", "crate::follow"]);
    forbid(&["chains"], &["crate::api", "crate::follow", "crate::config"]);
    forbid(&["follow.rs"], &["crate::api"]);
}

#[test]
fn chains_do_not_reach_into_each_other() {
    let root = source().join("chains");
    let chains: Vec<String> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(chains.len() >= 4, "{chains:?}");
    for chain in &chains {
        for other in chains.iter().filter(|other| *other != chain) {
            let patterns = [
                format!("chains::{other}"),
                format!("super::{other}"),
                format!("super::super::{other}"),
            ];
            let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
            forbid(&[&format!("chains/{chain}")], &patterns);
        }
    }
}

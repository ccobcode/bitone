//! 核心库的分层约定.
//!
//! 每条链一个目录 `chains/<chain>/`,解码,审核,构建,策略和规则表都在里面,目录之间互不引用.
//! 共享层不出现具体的链,按 `Chain` 分发只在 `chains/mod.rs`.加一条链加一个目录,再在那里接一行.
//! 解码不依赖策略,清单和审核.策略只依赖模型.C3 的写法在这里检查.

use std::fs;
use std::path::{Path, PathBuf};

const CHAIN_NAMES: [&str; 4] = ["Chain::Bitcoin", "Chain::Evm", "Chain::Tron", "Chain::Solana"];

/// 共享层里允许出现具体链的注册表.它们记录的是跨链的官方编号:网络和 Chain ID,二维码协议冻结的编号,
/// SLIP-44 的 coin type 和各链的地址格式.交易的解码,审核和策略都不在这里.
const REGISTRIES: [&str; 5] = ["network.rs", "protocol.rs", "path.rs", "address.rs", "keys.rs"];

/// 只在测试里编译的模块.测试可以写具体的链.
const TEST_ONLY: [&str; 2] = ["decode/fixture.rs", "decode/tests.rs"];

/// 一条链里不属于解码的文件.其余文件都是解码器.
const CHAIN_ROLES: [&str; 4] = ["mod.rs", "tx.rs", "policy.rs", "generated"];

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 去掉整行注释和测试模块.测试可以写具体的链.
fn code(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    let body = text.split("#[cfg(test)]\nmod tests").next().unwrap();
    let lines = body.lines().filter(|line| !line.trim_start().starts_with("//"));
    lines.collect::<Vec<_>>().join("\n")
}

/// 目录下全部 Rust 文件,`(相对 src 的路径, 代码)`.
fn files(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let name = path.strip_prefix(source()).unwrap().to_string_lossy().into_owned();
            found.push((name, code(&path)));
        }
    }
    found
}

fn chains() -> Vec<String> {
    let found: Vec<String> = fs::read_dir(source().join("chains"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(found.len() >= 4, "{found:?}");
    found
}

fn chain_decoders(chain: &str) -> Vec<(String, String)> {
    let dir = source().join("chains").join(chain);
    let found: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| !CHAIN_ROLES.contains(&path.file_name().unwrap().to_str().unwrap()))
        .map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), code(&path)))
        .collect();
    assert!(!found.is_empty(), "{chain} 没有解码器");
    found
}

/// 共享的解码模型和全部链的解码器.
fn decode_files() -> Vec<(String, String)> {
    let mut found = files(&source().join("decode"));
    for chain in chains() {
        found.extend(
            chain_decoders(&chain)
                .into_iter()
                .map(|(file, text)| (format!("{chain}/{file}"), text)),
        );
    }
    found
}

/// `crate::` 后面的第一段,`crate::{a, b::c}` 展开成 `a`,`b`.
fn crate_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (index, _) in text.match_indices("crate::") {
        let rest = &text[index + "crate::".len()..];
        match rest.strip_prefix('{') {
            Some(group) => {
                let group = group.split('}').next().unwrap();
                found.extend(group.split(',').map(|part| first_segment(part.trim())));
            }
            None => found.push(first_segment(rest)),
        }
    }
    found.retain(|segment| !segment.is_empty());
    found
}

fn first_segment(path: &str) -> String {
    path.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
}

fn outside(text: &str, allowed: &[&str]) -> Vec<String> {
    let mut stray = crate_paths(text);
    stray.retain(|segment| !allowed.contains(&segment.as_str()));
    stray
}

#[test]
fn crate_paths_are_found_in_every_form() {
    let text = "use crate::policy::Risk;\nuse crate::{Chain, review::Row};\nlet x = crate::manifest::load();";
    assert_eq!(crate_paths(text), ["policy", "Chain", "review", "manifest"]);
}

#[test]
fn decode_uses_only_the_foundation() {
    let allowed = ["decode", "Chain", "Mode", "hex", "units"];
    for (file, text) in decode_files() {
        let stray = outside(&text, &allowed);
        assert!(stray.is_empty(), "{file} 不能依赖 {stray:?}");
        assert!(!text.contains("super::super"), "{file} 不能绕过模块边界");
    }
}

#[test]
fn shared_layers_do_not_name_a_chain() {
    for (file, text) in files(&source()) {
        let exempt = REGISTRIES.contains(&file.as_str()) || TEST_ONLY.contains(&file.as_str());
        if file.starts_with("chains/") || exempt {
            continue;
        }
        for name in CHAIN_NAMES {
            assert!(
                !text.contains(name),
                "{file} 不能出现 {name},按链分发只在 chains/mod.rs"
            );
        }
    }
}

#[test]
fn only_the_registry_matches_on_the_chain() {
    let registry = code(&source().join("chains/mod.rs"));
    assert!(registry.contains("match chain {"));
    for chain in chains() {
        for (file, text) in files(&source().join("chains").join(&chain)) {
            assert!(!text.contains("match chain {"), "{file}");
        }
    }
}

#[test]
fn chains_do_not_reach_into_each_other() {
    let all = chains();
    for chain in &all {
        for other in all.iter().filter(|other| *other != chain) {
            let patterns = [format!("chains::{other}"), format!("super::{other}")];
            for (file, text) in files(&source().join("chains").join(chain)) {
                for pattern in &patterns {
                    assert!(!text.contains(pattern.as_str()), "{file} 不能引用 {pattern}");
                }
            }
        }
    }
}

#[test]
fn every_chain_keeps_its_own_rule_table() {
    for chain in chains() {
        let policy = code(&source().join("chains").join(&chain).join("policy.rs"));
        assert_eq!(policy.matches("include_str!").count(), 1, "{chain}");
        assert!(policy.contains(&format!("assets/{chain}/rules.json")), "{chain}");
        for line in policy.lines().filter(|line| line.contains("pub fn assess")) {
            assert!(!line.contains("RuleTable"), "{chain}: 规则表不是调用方能传入的参数");
        }
    }
    let engine = code(&source().join("policy.rs"));
    assert!(!engine.contains("rules.json"), "共享的策略没有自己的规则表");
}

/// 共享层不能依赖某一条链的模块.两条链共用的标准,比如 ABI 和 EIP-712,放在共享层.
#[test]
fn shared_layers_do_not_reach_into_a_chain() {
    for (file, text) in files(&source()) {
        if file.starts_with("chains/") || TEST_ONLY.contains(&file.as_str()) {
            continue;
        }
        for chain in chains() {
            let pattern = format!("chains::{chain}");
            assert!(!text.contains(&pattern), "{file} 不能引用 {pattern}");
        }
    }
}

#[test]
fn policy_depends_only_on_the_model() {
    let engine = code(&source().join("policy.rs"));
    let stray = outside(&engine, &["decode", "Network"]);
    assert!(stray.is_empty(), "policy.rs 不能依赖 {stray:?}");
    for chain in chains() {
        let text = code(&source().join("chains").join(&chain).join("policy.rs"));
        let stray = outside(&text, &["decode", "policy", "Chain", "Mode", "Network"]);
        assert!(stray.is_empty(), "{chain}/policy.rs 不能依赖 {stray:?}");
    }
}

#[test]
fn policy_never_sees_the_payload() {
    let mut texts = vec![("policy.rs".to_string(), code(&source().join("policy.rs")))];
    for chain in chains() {
        let path = source().join("chains").join(&chain).join("policy.rs");
        texts.push((format!("{chain}/policy.rs"), code(&path)));
    }
    for (file, text) in texts {
        for word in ["payload", "SignRequest", "Input"] {
            assert!(!text.contains(word), "{file} 不能接触 {word}");
        }
    }
}

#[test]
fn the_model_exposes_no_payload_or_setter() {
    let model = code(&source().join("decode/mod.rs"));
    let decoded = model.split("impl Decoded {").nth(1).unwrap();
    let decoded = decoded.split("\n}\n").next().unwrap();
    assert!(!decoded.contains("&mut self"));
    assert!(!decoded.contains("-> &[u8]"));
    let fields = model.split("pub struct Decoded {").nth(1).unwrap();
    let fields = fields.split('}').next().unwrap();
    assert!(!fields.contains("payload"));
    assert!(!fields.contains("pub "));
}

/// C3 禁止的写法,返回命中的片段.字符串里的内容也算,解码器不需要这样写字符串.
fn exhaustiveness_holes(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let previous = |mut index: usize| {
        while index > 0 {
            index -= 1;
            if !chars[index].is_whitespace() {
                return chars[index];
            }
        }
        '\0'
    };
    let next = |mut index: usize| {
        while index < chars.len() {
            if !chars[index].is_whitespace() {
                return chars[index];
            }
            index += 1;
        }
        '\0'
    };
    let mut holes = Vec::new();
    for index in 0..chars.len() {
        let wildcard = chars[index] == '_'
            && (index == 0 || !(chars[index - 1].is_alphanumeric() || chars[index - 1] == '_'))
            && chars.get(index + 1).is_none_or(|c| !(c.is_alphanumeric() || *c == '_'));
        if wildcard {
            let after = chars[index + 1..].iter().collect::<String>();
            let after = after.trim_start();
            let arm = after.starts_with("=>");
            let field = previous(index) == ':' && matches!(next(index + 1), ',' | '}');
            if arm {
                holes.push("_ =>".to_string());
            } else if field {
                holes.push(": _".to_string());
            }
        }
        if chars[index] == '.' && chars.get(index + 1) == Some(&'.') && chars.get(index + 2) != Some(&'=') {
            let rest =
                matches!(previous(index), '{' | ',' | '(' | '[') && matches!(next(index + 2), '}' | ')' | ']' | ',');
            if rest {
                holes.push("..".to_string());
            }
        }
    }
    holes
}

#[test]
fn the_exhaustiveness_scan_finds_every_hole() {
    for hole in [
        "match x { A => 1, _ => 2 }",
        "let Tx { nonce, .. } = tx;",
        "let Tx { nonce, value: _ } = tx;",
        "let Tx { .. } = tx;",
        "let (a, ..) = pair;",
        "let [first, ..] = list;",
        "match x { A { .. } => 1 }",
    ] {
        assert!(!exhaustiveness_holes(hole).is_empty(), "{hole}");
    }
}

#[test]
fn the_exhaustiveness_scan_passes_plain_code() {
    for fine in [
        "let Tx { nonce, value } = tx;",
        "match x { A => 1, B => 2 }",
        "let part = &bytes[start..end];",
        "let tail = &bytes[4..];",
        "let head = &bytes[..4];",
        "for index in 0..len {}",
        "while let [tag, tail @ ..] = rest {}",
        "let next = Self { offset, ..self };",
        "let _unused_name_is_not_a_wildcard = 1;",
        "let f = |_: u8| 1;",
    ] {
        assert!(exhaustiveness_holes(fine).is_empty(), "{fine}");
    }
}

#[test]
fn the_generated_dispatch_is_exhaustive() {
    let text = fs::read_to_string(source().join("chains/tron/generated/recode.rs")).unwrap();
    assert!(exhaustiveness_holes(&text).is_empty());
}

#[test]
fn decoders_are_exhaustive() {
    for (file, text) in decode_files() {
        let holes = exhaustiveness_holes(&text);
        assert!(holes.is_empty(), "{file} 不能写 {holes:?},官方类型的字段和分支都要进树");
    }
}

#[test]
fn clippy_keeps_enum_matches_exhaustive() {
    let module = fs::read_to_string(source().join("decode/mod.rs")).unwrap();
    assert!(module.contains("deny(clippy::wildcard_enum_match_arm"));
    for chain in chains() {
        for (file, _) in chain_decoders(&chain) {
            let text = fs::read_to_string(source().join("chains").join(&chain).join(&file)).unwrap();
            assert!(text.contains("deny(clippy::wildcard_enum_match_arm"), "{chain}/{file}");
        }
    }
}

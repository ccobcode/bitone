//! 解码和策略的分层约定.
//! `decode` 不依赖 `policy`,`manifest`,`review`.`policy` 只依赖模型.C3 的写法在这里检查.

use std::fs;
use std::path::{Path, PathBuf};

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// 去掉整行注释和测试模块.
fn code(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    let body = text.split("#[cfg(test)]\nmod tests").next().unwrap();
    let lines = body.lines().filter(|line| !line.trim_start().starts_with("//"));
    lines.collect::<Vec<_>>().join("\n")
}

fn decode_files() -> Vec<(String, String)> {
    let found: Vec<(String, String)> = fs::read_dir(source().join("decode"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), code(&path)))
        .collect();
    assert!(found.len() >= 4, "{found:?}");
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
    let allowed = ["Chain", "Mode", "hex", "units"];
    for (file, text) in decode_files() {
        let stray = outside(&text, &allowed);
        assert!(stray.is_empty(), "{file} 不能依赖 {stray:?}");
        assert!(!text.contains("super::super"), "{file} 不能绕过模块边界");
    }
}

#[test]
fn policy_depends_only_on_the_model() {
    let text = code(&source().join("policy.rs"));
    let stray = outside(&text, &["decode", "Chain", "Mode", "Network"]);
    assert!(stray.is_empty(), "policy.rs 不能依赖 {stray:?}");
}

#[test]
fn policy_never_sees_the_payload() {
    let text = code(&source().join("policy.rs"));
    for word in ["payload", "SignRequest", "Input"] {
        assert!(!text.contains(word), "policy.rs 不能接触 {word}");
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

#[test]
fn the_rule_table_has_one_source() {
    let policy = code(&source().join("policy.rs"));
    assert_eq!(policy.matches("include_str!").count(), 1);
    assert!(policy.contains("assets/rules.json"));
    let assess = policy.split("pub fn assess").nth(1).unwrap();
    let signature = assess.split('{').next().unwrap();
    assert!(!signature.contains("Rules"), "规则表不是调用方能传入的参数");
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
    let text = fs::read_to_string(source().join("decode/generated/recode.rs")).unwrap();
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
}

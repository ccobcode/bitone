//! 全覆盖检查 C1,C2,C4,见 docs/10-tx-decoding.md 第 2.2 节.C3 在编译时成立,见 `tests/architecture.rs`.

use super::model::{Extent, Node, Span};
use crate::hex::DisplayHex;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use thiserror::Error;

/// 树的嵌套上限.发布配置是 `panic = "abort"`,解码器的递归不能靠栈溢出结束.
pub const MAX_DEPTH: u32 = 32;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum Reject {
    #[error("载荷无法解码")]
    Malformed,
    #[error("载荷有未归属的字节,偏移 {offset}")]
    Gap { offset: u64 },
    #[error("载荷的字节归属重复,偏移 {offset}")]
    Overlap { offset: u64 },
    #[error("字段的字节范围超出载荷")]
    OutOfBounds,
    #[error("字段的字节范围不成立")]
    BadExtent,
    #[error("解码树嵌套过深")]
    TooDeep,
    #[error("重新编码的结果和载荷不同")]
    RoundTrip,
    #[error("解码器没有提供重新编码的结果")]
    NoRoundTrip,
    #[error("渲染列表和解码树不一致")]
    Rendered,
}

/// 解码失败.页面显示原因和载荷的 SHA-256.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{reason},载荷 SHA-256 {digest}")]
pub struct Failure {
    pub reason: Reject,
    pub digest: String,
}

pub fn payload_digest(payload: &[u8]) -> String {
    Sha256::digest(payload).to_lower_hex_string()
}

/// C1:`signed`,`context` 的字节范围加结构字节恰好覆盖载荷,不重不漏.
pub fn partition(root: &Node, payload_len: usize) -> Result<(), Reject> {
    let mut spans = Vec::new();
    collect(root, 0, false, &mut spans)?;
    let payload_len = payload_len as u64;
    if spans.iter().any(|span| span.end() > payload_len) {
        return Err(Reject::OutOfBounds);
    }
    spans.retain(|span| span.len > 0);
    spans.sort_unstable_by_key(|span| span.offset);
    let mut covered = 0u64;
    for span in spans {
        match u64::from(span.offset).cmp(&covered) {
            Ordering::Greater => return Err(Reject::Gap { offset: covered }),
            Ordering::Less => {
                return Err(Reject::Overlap {
                    offset: u64::from(span.offset),
                });
            }
            Ordering::Equal => covered = span.end(),
        }
    }
    if covered < payload_len {
        return Err(Reject::Gap { offset: covered });
    }
    Ok(())
}

/// `inherited` 表示祖先已经拥有这段字节,这里不再累加,子节点由 C2 证明.
fn collect(node: &Node, depth: u32, inherited: bool, spans: &mut Vec<Span>) -> Result<(), Reject> {
    if depth > MAX_DEPTH {
        return Err(Reject::TooDeep);
    }
    let extent = node.source.extent();
    let valid = match extent {
        Some(Extent::Own(_)) | Some(Extent::Container) => !inherited,
        Some(Extent::Inherited) => inherited,
        None => true,
    };
    let stray_framing = !node.framing.is_empty() && (inherited || extent.is_none());
    let empty_container = extent == Some(Extent::Container) && !node.value.is_group();
    if !valid || stray_framing || empty_container {
        return Err(Reject::BadExtent);
    }
    if let Some(Extent::Own(span)) = extent {
        spans.push(span);
    }
    spans.extend_from_slice(&node.framing);
    let inherited = inherited || matches!(extent, Some(Extent::Own(_)));
    node.children
        .iter()
        .try_for_each(|child| collect(child, depth + 1, inherited, spans))
}

/// C2 的一次比较.`original` 是载荷或它的一段,EIP-712 的载荷不进签名原文,比较的是 `hashStruct`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundTrip {
    pub original: Vec<u8>,
    pub rebuilt: Vec<u8>,
}

/// C2:重新编码的结果逐字节相同.空列表也是失败,解码器不能跳过这一步.
pub fn round_trip(checks: &[RoundTrip]) -> Result<(), Reject> {
    if checks.is_empty() {
        return Err(Reject::NoRoundTrip);
    }
    if checks.iter().any(|check| check.original != check.rebuilt) {
        return Err(Reject::RoundTrip);
    }
    Ok(())
}

/// C4:渲染列表的行数等于节点数.
pub fn rendered(rows: usize, nodes: u32) -> Result<(), Reject> {
    if rows != nodes as usize {
        return Err(Reject::Rendered);
    }
    Ok(())
}

//! 解码:把载荷变成一棵类型化的树,并证明树覆盖了全部字节.标准见 docs/10-tx-decoding.md 第 2 节.
//!
//! `decode` 是纯函数,不读清单和密钥.`Decoded` 只能经过 C1,C2 得到,里面没有原始载荷,
//! 所以策略拿到的都是检查过的树.依赖方向和 C3 由 `tests/architecture.rs` 检查.

#![deny(clippy::wildcard_enum_match_arm, clippy::wildcard_in_or_patterns)]

mod check;
mod evm;
#[cfg(test)]
pub(crate) mod fixture;
mod format;
mod model;
mod psbt;
mod psbt_official;
mod psbt_values;
mod rlp;
mod script;

pub use check::{Failure, MAX_DEPTH, Reject, RoundTrip};
pub use evm::{
    Coin, EvmPayload, EvmWalker, PersonalWalker, RawHashWalker, Tx, authority, fee_cap, parse_evm, transaction_actions,
};
pub use format::{display, escape_text};
pub use model::{Amount, Extent, Kind, Node, Source, SourceKind, Span, Value};
pub use psbt::{Parsed, PsbtWalker, is_psbt, parse};
pub use script::{ScriptClass, address_network, estimated_vsize, output_action, spend_weight};

use crate::{Chain, Mode};

/// 节点在前序遍历里的序号.策略按它追加发现,渲染时它就是行号.
pub type NodeId = u32;

/// 一次解码的输入.网络和派生路径不在载荷里,作为 `derived` 节点进树.
/// 本机不认识的网络也要解码,所以 `network` 是请求里的原文,不是 `Network`,
/// 链和私钥的类型来自派生路径.`mode` 为空表示主网和测试网共用这把私钥,比如 EVM.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub chain: Chain,
    pub mode: Option<Mode>,
    pub network: &'a str,
    pub kind: u8,
    pub paths: &'a [String],
    pub payload: &'a [u8],
    /// 付款账户的地址.派生路径算不出来时是空的,这一行不进树.
    pub signer: Option<&'a str>,
}

/// 解码器的结果,通过 C1,C2 之后才是 `Decoded`.外层认识,里面不认识时,不认识的部分是 `opaque` 节点.
#[derive(Clone, Debug)]
pub struct Walked {
    pub root: Node,
    pub round_trips: Vec<RoundTrip>,
}

/// 一种载荷的解码器.返回 `None` 表示不认识这个格式,这不是失败,树是一个 `opaque` 根节点.
/// 返回错误表示格式已识别而字节不合规,这笔交易被拒绝.
pub trait Decoder {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject>;
}

/// 前序遍历到的一个节点.`span` 是这一行的字节范围:继承的取祖先的,容器取子节点的外包.
#[derive(Clone, Copy, Debug)]
pub struct Visit<'a> {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: u32,
    pub node: &'a Node,
    pub span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    chain: Chain,
    mode: Option<Mode>,
    network: String,
    kind: u8,
    root: Node,
    nodes: u32,
    digest: String,
}

pub fn decode(decoder: &impl Decoder, input: &Input<'_>) -> Result<Decoded, Failure> {
    let digest = check::payload_digest(input.payload);
    match checked(decoder, input) {
        Ok(root) => Ok(seal(root, input, digest)),
        Err(reason) => Err(Failure { reason, digest }),
    }
}

/// 格式不认识时不走 C1,C2:整个载荷就是一个节点,没有可以比较的结构.
fn checked(decoder: &impl Decoder, input: &Input<'_>) -> Result<Node, Reject> {
    let Some(Walked { root, round_trips }) = decoder.walk(input)? else {
        return Ok(opaque_root(input.payload));
    };
    check::partition(&root, input.payload.len())?;
    check::round_trip(&round_trips)?;
    Ok(root)
}

fn opaque_root(payload: &[u8]) -> Node {
    let span = Span::new(0, payload.len() as u32);
    Node::new(
        "payload",
        Value::Opaque(payload.to_vec()),
        Source::Signed(Extent::Own(span)),
    )
}

fn seal(mut root: Node, input: &Input<'_>, digest: String) -> Decoded {
    root.children.splice(0..0, envelope(input));
    let mut decoded = Decoded {
        chain: input.chain,
        mode: input.mode,
        network: input.network.to_string(),
        kind: input.kind,
        root,
        nodes: 0,
        digest,
    };
    decoded.nodes = decoded.visits().len() as u32;
    decoded
}

fn envelope(input: &Input<'_>) -> Vec<Node> {
    let network = Node::new("network", Value::Text(input.network.to_string()), Source::Derived);
    let paths = input
        .paths
        .iter()
        .map(|path| Node::new("derivation_path", Value::Text(path.clone()), Source::Derived));
    let signer = input
        .signer
        .map(|address| Node::new("signer", Value::Address(address.to_string()), Source::Derived));
    std::iter::once(network).chain(paths).chain(signer).collect()
}

impl Decoded {
    pub fn chain(&self) -> Chain {
        self.chain
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// 请求里的网络原文,本机不一定认识.
    pub fn network(&self) -> &str {
        &self.network
    }

    /// `SignRequest.kind`.消息签名也走这棵树,这样拒绝页能显示解码后的内容.
    pub fn kind(&self) -> u8 {
        self.kind
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn node_count(&self) -> u32 {
        self.nodes
    }

    /// 载荷的 SHA-256,不是载荷本身.
    pub fn payload_digest(&self) -> &str {
        &self.digest
    }

    /// 全部节点,前序.C1 已经限制了嵌套,这里的递归有上限.
    pub fn visits(&self) -> Vec<Visit<'_>> {
        let mut visits = Vec::new();
        visit(&self.root, None, 0, None, &mut visits);
        visits
    }

    /// 全部动作:节点序号,节点,规则表里的种类名.
    pub fn actions(&self) -> Vec<(NodeId, &Node, &str)> {
        self.visits()
            .into_iter()
            .filter_map(|visit| Some((visit.id, visit.node, visit.node.action.as_deref()?)))
            .collect()
    }

    /// C4.
    pub fn check_rendered(&self, rows: usize) -> Result<(), Failure> {
        check::rendered(rows, self.nodes).map_err(|reason| Failure {
            reason,
            digest: self.digest.clone(),
        })
    }
}

/// 返回这棵子树的字节外包.
fn visit<'a>(
    node: &'a Node,
    parent: Option<NodeId>,
    depth: u32,
    inherited: Option<Span>,
    visits: &mut Vec<Visit<'a>>,
) -> Option<Span> {
    let id = visits.len() as NodeId;
    let extent = node.source.extent();
    let span = match extent {
        Some(Extent::Own(span)) => Some(span),
        Some(Extent::Inherited) => inherited,
        Some(Extent::Container) | None => None,
    };
    visits.push(Visit {
        id,
        parent,
        depth,
        node,
        span,
    });
    let mut hull = node
        .framing
        .iter()
        .copied()
        .fold(span, |hull, part| union(hull, Some(part)));
    for child in &node.children {
        hull = union(hull, visit(child, Some(id), depth + 1, span.or(inherited), visits));
    }
    if extent == Some(Extent::Container) {
        visits[id as usize].span = hull;
    }
    hull
}

fn union(a: Option<Span>, b: Option<Span>) -> Option<Span> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let offset = a.offset.min(b.offset);
            let end = a.end().max(b.end());
            Some(Span::new(offset, (end - u64::from(offset)) as u32))
        }
        (one, None) | (None, one) => one,
    }
}

#[cfg(test)]
mod tests;

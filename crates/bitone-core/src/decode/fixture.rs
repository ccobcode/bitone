//! 测试用的小格式:若干 `标签 长度 内容` 记录.只有测试编译,不是任何一条链的解码器.
//!
//! 标签 1 是备注,标签 2 是金额,标签 3 的内容交给 `Sealed`,模拟没有字节范围的官方编解码器.
//! 其他标签是以后才有的记录,内容是 `opaque`.首字节 `UNKNOWN_FORMAT` 表示不认识整个载荷.

use super::{Decoder, Extent, Input, Node, Reject, RoundTrip, Source, Span, Value, Walked};
use crate::{Chain, Mode};

pub const UNKNOWN_FORMAT: u8 = 0xff;

/// 官方类型.解构不写 `..`,加字段时 `sealed_nodes` 编译失败.
pub struct Sealed {
    pub first: u8,
    pub second: u8,
}

/// 官方枚举.`match` 不写 `_`,加分支时 `record_node` 编译失败.
pub enum Tag {
    Memo,
    Amount,
    Sealed,
    Future(u8),
}

impl Tag {
    fn parse(byte: u8) -> Self {
        match byte {
            1 => Self::Memo,
            2 => Self::Amount,
            3 => Self::Sealed,
            other => Self::Future(other),
        }
    }
}

fn sealed_nodes(sealed: Sealed) -> Vec<Node> {
    let Sealed { first, second } = sealed;
    let inherited = Source::Signed(Extent::Inherited);
    vec![
        Node::new("first", Value::Uint(vec![first]), inherited),
        Node::new("second", Value::Uint(vec![second]), inherited),
    ]
}

fn record_node(tag: Tag, content: &[u8], offset: u32) -> Result<Node, Reject> {
    let span = Span::new(offset + 2, content.len() as u32);
    let source = Source::Signed(Extent::Own(span));
    let node = match tag {
        Tag::Memo => {
            let text = std::str::from_utf8(content).map_err(|_| Reject::Malformed)?;
            Node::new("memo", Value::Text(text.to_string()), source)
        }
        Tag::Amount => Node::new("amount", Value::Uint(content.to_vec()), source).action("transfer"),
        Tag::Sealed => {
            let [first, second] = content else {
                return Err(Reject::Malformed);
            };
            let sealed = Sealed {
                first: *first,
                second: *second,
            };
            Node::new("sealed", Value::Struct, source).children(sealed_nodes(sealed))
        }
        Tag::Future(tag) => Node::new("record", Value::Opaque(content.to_vec()), source).action(format!("tag-{tag}")),
    };
    Ok(node.framed(vec![Span::new(offset, 2)]))
}

/// 老实的解码器.
pub struct Records;

impl Decoder for Records {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        if input.payload.first() == Some(&UNKNOWN_FORMAT) {
            return Ok(None);
        }
        let mut children = Vec::new();
        let mut rest = input.payload;
        let mut offset = 0u32;
        while let [tag, length, tail @ ..] = rest {
            let content = tail.get(..usize::from(*length)).ok_or(Reject::Malformed)?;
            children.push(record_node(Tag::parse(*tag), content, offset)?);
            offset += 2 + u32::from(*length);
            rest = &tail[usize::from(*length)..];
        }
        if !rest.is_empty() {
            return Err(Reject::Malformed);
        }
        Ok(Some(Walked {
            root: Node::new("records", Value::Struct, Source::Signed(Extent::Container)).children(children),
            round_trips: vec![RoundTrip {
                original: input.payload.to_vec(),
                rebuilt: input.payload.to_vec(),
            }],
        }))
    }
}

/// 用任意函数改写老实解码器的结果,测试各种不诚实的解码器.
pub struct Tampered<F>(pub F);

impl<F: Fn(&mut Walked)> Decoder for Tampered<F> {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let mut walked = Records.walk(input)?;
        walked.iter_mut().for_each(&self.0);
        Ok(walked)
    }
}

pub fn input(payload: &[u8]) -> Input<'_> {
    Input {
        chain: Chain::Evm,
        mode: Some(Mode::Mainnet),
        network: "ethereum",
        kind: 0,
        paths: &[],
        payload,
    }
}

/// 一个金额加一条备注.
pub const PAYLOAD: [u8; 9] = [2, 2, 0x01, 0x00, 1, 3, b'o', b'k', b'!'];

/// 一个金额,加一条这个版本不认识的记录.
pub const PARTIAL: [u8; 8] = [2, 2, 0x01, 0x00, 9, 2, 0xaa, 0xbb];

//! protobuf 线格式.这里只认容器,字段的含义来自 schema.

#![deny(clippy::wildcard_enum_match_arm, clippy::wildcard_in_or_patterns)]

use crate::decode::{Cursor, Field, MAX_DEPTH, Reject, Span};

const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const LENGTH_DELIMITED: u64 = 2;
const GROUP_START: u64 = 3;
const GROUP_END: u64 = 4;
const FIXED32: u64 = 5;
const WIRE_BITS: u32 = 3;
const WIRE_MASK: u64 = 7;

/// protobuf 的变长整数.多余的编码一律拒绝,同一笔交易不能有两种写法.
pub fn read_varint(buf: &mut &[u8]) -> Result<u64, Reject> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = buf.split_first().ok_or(Reject::Malformed)?;
        *buf = rest;
        let bits = u64::from(byte & 0x7f);
        // 第 10 个字节只能放最高位,多出来的位会被其他解析器丢掉.
        if shift == 63 && bits > 1 {
            return Err(Reject::Malformed);
        }
        value |= bits << shift;
        if byte & 0x80 == 0 {
            if byte == 0 && shift > 0 {
                return Err(Reject::NonCanonical);
            }
            return Ok(value);
        }
    }
    Err(Reject::Malformed)
}

pub(super) fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub(super) fn put_bytes(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    put_varint(out, u64::from(number) << WIRE_BITS | LENGTH_DELIMITED);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// 一个字段的值.定长和分组只出现在未知字段上.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Body {
    Varint(u64),
    Fixed,
    Bytes,
    Group,
}

/// 线格式上的一个字段.
pub(super) struct Wire<'a> {
    pub(super) number: u32,
    pub(super) body: Body,
    /// 值的字节.长度前缀和分组的结束标签都在 `framing` 里.
    pub(super) value: Field<'a>,
    pub(super) framing: Vec<Span>,
    /// 标签的原始字节.
    pub(super) tag: &'a [u8],
    /// 这个字段的全部字节.
    pub(super) raw: &'a [u8],
}

fn varint<'a>(cursor: &mut Cursor<'a>) -> Result<(u64, Field<'a>), Reject> {
    let mut rest = cursor.remaining();
    let value = read_varint(&mut rest)?;
    let used = cursor.remaining().len() - rest.len();
    Ok((value, cursor.take(used as u64)?))
}

/// 节点只读标签的低 32 位,更长的标签在节点上是另一个字段.编号 0 不合法.
fn number_of(tag: u64) -> Result<u32, Reject> {
    let number = u32::try_from(tag).map_err(|_| Reject::Malformed)? >> WIRE_BITS;
    if number == 0 {
        return Err(Reject::Malformed);
    }
    Ok(number)
}

fn group<'a>(cursor: &mut Cursor<'a>, number: u32, depth: u32) -> Result<(Field<'a>, Span), Reject> {
    if depth >= MAX_DEPTH {
        return Err(Reject::TooDeep);
    }
    let inner = cursor.remaining();
    let offset = cursor.take(0)?.span.offset;
    loop {
        let before = cursor.remaining();
        let (tag, head) = varint(cursor)?;
        if tag & WIRE_MASK == GROUP_END {
            if number_of(tag)? != number {
                return Err(Reject::Malformed);
            }
            let bytes = inner.get(..inner.len() - before.len()).ok_or(Reject::Malformed)?;
            let span = Span::new(offset, bytes.len() as u32);
            return Ok((Field { bytes, span }, head.span));
        }
        body(cursor, tag, depth + 1)?;
    }
}

/// 值的类型,字节和结构字节.
fn body<'a>(cursor: &mut Cursor<'a>, tag: u64, depth: u32) -> Result<(Body, Field<'a>, Vec<Span>), Reject> {
    let number = number_of(tag)?;
    match tag & WIRE_MASK {
        VARINT => {
            let (value, field) = varint(cursor)?;
            Ok((Body::Varint(value), field, Vec::new()))
        }
        FIXED64 => Ok((Body::Fixed, cursor.take(8)?, Vec::new())),
        FIXED32 => Ok((Body::Fixed, cursor.take(4)?, Vec::new())),
        LENGTH_DELIMITED => {
            let (len, head) = varint(cursor)?;
            Ok((Body::Bytes, cursor.take(len)?, vec![head.span]))
        }
        GROUP_START => {
            let (field, end) = group(cursor, number, depth)?;
            Ok((Body::Group, field, vec![end]))
        }
        _other => Err(Reject::Malformed),
    }
}

/// 下一个字段.标签,长度前缀不是最短编码时拒绝.
pub(super) fn next<'a>(cursor: &mut Cursor<'a>) -> Result<Wire<'a>, Reject> {
    let start = cursor.remaining();
    let (tag, head) = varint(cursor)?;
    let (value_kind, value, mut framing) = body(cursor, tag, 0)?;
    framing.insert(0, head.span);
    let raw = start
        .get(..start.len() - cursor.remaining().len())
        .ok_or(Reject::Malformed)?;
    Ok(Wire {
        number: number_of(tag)?,
        body: value_kind,
        value,
        framing,
        tag: head.bytes,
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(bytes: &[u8]) -> Result<(u32, Body, Vec<u8>), Reject> {
        let field = Field {
            bytes,
            span: Span::new(0, bytes.len() as u32),
        };
        let mut cursor = Cursor::new(&field);
        let wire = next(&mut cursor)?;
        Ok((wire.number, wire.body, wire.value.bytes.to_vec()))
    }

    #[test]
    fn varint_is_strict() {
        for bad in [
            vec![0x80],                           // 截断
            vec![0xff; 11],                       // 过长
            [vec![0xff; 9], vec![0x02]].concat(), // 超过 64 位
        ] {
            assert_eq!(read_varint(&mut bad.as_slice()), Err(Reject::Malformed), "{bad:?}");
        }
        // 非最短的写法官方解析器接受,只是重新编码回不到原文.
        assert_eq!(read_varint(&mut [0x80, 0x00].as_slice()), Err(Reject::NonCanonical));
        let mut out = Vec::new();
        put_varint(&mut out, u64::MAX);
        assert_eq!(read_varint(&mut out.as_slice()), Ok(u64::MAX));
    }

    #[test]
    fn every_wire_type_has_a_value() {
        assert_eq!(wire(&[0x08, 0x05]), Ok((1, Body::Varint(5), vec![5])));
        assert_eq!(wire(&[0x12, 1, 9]), Ok((2, Body::Bytes, vec![9])));
        assert_eq!(wire(&[0x1d, 1, 2, 3, 4]), Ok((3, Body::Fixed, vec![1, 2, 3, 4])));
        assert_eq!(wire(&[0x21, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap().1, Body::Fixed);
        assert_eq!(wire(&[0x2b, 0x08, 0x01, 0x2c]), Ok((5, Body::Group, vec![0x08, 0x01])));
    }

    #[test]
    fn broken_framing_is_refused() {
        for bad in [
            vec![0x02, 0x00],                         // 编号 0
            vec![0x2c],                               // 没有开始的结束标签
            vec![0x2b, 0x08, 0x01],                   // 没有结束的分组
            vec![0x2b, 0x34],                         // 结束标签的编号不同
            vec![0x0e, 0x00],                         // 线格式类型 6
            vec![0x12, 0x05, 0x01],                   // 长度越界
            vec![0x88, 0x80, 0x80, 0x80, 0x10, 0x01], // 标签超过 32 位
        ] {
            assert_eq!(wire(&bad).unwrap_err(), Reject::Malformed, "{bad:?}");
        }
    }

    #[test]
    fn groups_cannot_nest_past_the_limit() {
        let depth = MAX_DEPTH as usize + 2;
        let bytes = [vec![0x0b; depth], vec![0x0c; depth]].concat();
        assert_eq!(wire(&bytes).unwrap_err(), Reject::TooDeep);
    }
}

//! RLP 项遍历器,只认容器格式:长度前缀和列表.字段含义来自官方类型.

use super::psbt::Cursor;
use super::psbt_values::Field;
use super::{Reject, Span};
use alloy_rlp::Header;

/// 一个 RLP 项.`head` 是长度前缀,单字节项的前缀是空的.
#[derive(Clone, Copy, Debug)]
pub(super) struct Item<'a> {
    pub(super) list: bool,
    pub(super) head: Span,
    pub(super) body: Field<'a>,
    /// 含长度前缀的原始字节,和官方类型的编码比较.
    pub(super) raw: &'a [u8],
}

/// `Header::decode` 拒绝非规范的长度前缀.
pub(super) fn item<'a>(cursor: &mut Cursor<'a>) -> Result<Item<'a>, Reject> {
    let start = cursor.remaining();
    let mut rest = start;
    let header = Header::decode(&mut rest).map_err(|_| Reject::Malformed)?;
    let head = cursor.take((start.len() - rest.len()) as u64)?;
    let body = cursor.take(header.payload_length as u64)?;
    let raw = start
        .get(..head.bytes.len() + body.bytes.len())
        .ok_or(Reject::Malformed)?;
    Ok(Item {
        list: header.list,
        head: head.span,
        body,
        raw,
    })
}

/// 读头剩下的字节必须恰好是一个列表.
pub(super) fn whole_list<'a>(cursor: &mut Cursor<'a>) -> Result<Item<'a>, Reject> {
    let list = item(cursor)?;
    if !list.list || !cursor.is_empty() {
        return Err(Reject::Malformed);
    }
    Ok(list)
}

/// 列表的全部子项.
pub(super) fn items<'a>(list: &Item<'a>) -> Result<Vec<Item<'a>>, Reject> {
    if !list.list {
        return Err(Reject::Malformed);
    }
    let mut cursor = Cursor::new(&list.body);
    let mut found = Vec::new();
    while !cursor.is_empty() {
        found.push(item(&mut cursor)?);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(bytes: &[u8]) -> Cursor<'_> {
        Cursor::new(&Field {
            bytes,
            span: Span::new(0, bytes.len() as u32),
        })
    }

    #[test]
    fn a_single_byte_has_an_empty_head() {
        let found = item(&mut cursor(&[0x7f])).unwrap();
        assert_eq!((found.head.len, found.body.bytes), (0, &[0x7f][..]));
    }

    #[test]
    fn spans_follow_the_nesting() {
        let bytes = [0xc4, 0x82, 0xaa, 0xbb, 0x01];
        let list = whole_list(&mut cursor(&bytes)).unwrap();
        let inner = items(&list).unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!((inner[0].head, inner[0].body.span), (Span::new(1, 1), Span::new(2, 2)));
        assert_eq!(inner[1].raw, &[0x01]);
    }

    #[test]
    fn non_canonical_lengths_are_refused() {
        // 单字节要用自身编码,长度小于 56 不能用长形式.
        assert!(item(&mut cursor(&[0x81, 0x01])).is_err());
        assert!(item(&mut cursor(&[0xb8, 0x01, 0xff])).is_err());
    }

    #[test]
    fn a_list_cannot_end_inside_an_item() {
        let cut = whole_list(&mut cursor(&[0xc2, 0x82, 0xaa])).unwrap();
        assert!(items(&cut).is_err());
        assert!(whole_list(&mut cursor(&[0xc0, 0x00])).is_err());
        assert!(items(&item(&mut cursor(&[0x01])).unwrap()).is_err());
    }
}

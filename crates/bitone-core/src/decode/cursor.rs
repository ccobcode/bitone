//! 载荷的读头.每条链的遍历器都用它取字节,范围都从它得到,不手算偏移.

use super::{Reject, Span};

/// 载荷里的一段字节.
#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    pub bytes: &'a [u8],
    pub span: Span,
}

impl<'a> Field<'a> {
    pub fn whole(payload: &'a [u8]) -> Self {
        Self {
            bytes: payload,
            span: Span::new(0, payload.len() as u32),
        }
    }
}

pub struct Cursor<'a> {
    rest: &'a [u8],
    offset: u32,
}

impl<'a> Cursor<'a> {
    pub fn new(field: &Field<'a>) -> Self {
        Self {
            rest: field.bytes,
            offset: field.span.offset,
        }
    }

    /// 下一个字节在载荷里的位置.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub fn remaining(&self) -> &'a [u8] {
        self.rest
    }

    pub fn take(&mut self, len: u64) -> Result<Field<'a>, Reject> {
        let count = usize::try_from(len).map_err(|_| Reject::Malformed)?;
        let (bytes, rest) = self.rest.split_at_checked(count).ok_or(Reject::Malformed)?;
        let span = Span::new(self.offset, count as u32);
        self.rest = rest;
        self.offset += count as u32;
        Ok(Field { bytes, span })
    }

    pub fn rest(&mut self) -> Field<'a> {
        let span = Span::new(self.offset, self.rest.len() as u32);
        let bytes = std::mem::take(&mut self.rest);
        self.offset += span.len;
        Field { bytes, span }
    }
}

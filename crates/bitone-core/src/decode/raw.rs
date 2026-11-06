//! 裸哈希.32 字节的摘要不属于哪条链,签名方直接签这 32 字节.

use super::{Decoder, Extent, Field, Input, Node, Reject, RoundTrip, Source, Value, Walked};

/// 裸哈希,载荷就是要签的 32 字节.
pub struct RawHashWalker;

impl Decoder for RawHashWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let digest = <[u8; 32]>::try_from(input.payload).map_err(|_| Reject::Malformed)?;
        let source = Source::Signed(Extent::Own(Field::whole(input.payload).span));
        Ok(Some(Walked {
            root: Node::new("raw_hash", Value::Hash(digest.to_vec()), source).action("raw-hash"),
            round_trips: vec![RoundTrip {
                original: input.payload.to_vec(),
                rebuilt: digest.to_vec(),
            }],
        }))
    }
}

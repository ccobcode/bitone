//! 没有官方 IDL 的原生程序,按官方 interface crate 解码.
//!
//! Vote 和 Loader v4 的指令是官方枚举的 bincode 编码,读法和程序相同:定长整数,允许尾部字节.
//! 字段名和分支名来自官方类型的 serde 定义,元组字段按位置命名,和 Rust 的 `.0`,`.1` 一致.
//! 官方解码器没有字节范围,指令数据整段归一个节点,字段继承它的范围,由官方类型重新编码证明.
//! Ed25519 预编译和 ZK ElGamal Proof 是定长的结构,字段名来自官方结构体的解构.
//! Secp256k1,Secp256r1 预编译的官方 crate 分别带进 `k256` 的系统随机数和 OpenSSL,网页端和手机都不能用,
//! 这两个程序按第三方程序处理.

#![deny(clippy::wildcard_enum_match_arm, clippy::wildcard_in_or_patterns)]

use crate::decode::{ABSENT_TEXT, Cursor, Extent, Field, Node, Reject, RoundTrip, Source, Span, Value};
use bincode::Options;
use bitcoin::base58;
use bytemuck::Pod;
use serde::Serialize;
use serde::ser::{self, Impossible};
use solana_loader_v4_interface::instruction::LoaderV4Instruction;
use solana_vote_interface::instruction::VoteInstruction;
use solana_zk_elgamal_proof_interface::instruction::ProofInstruction;
use solana_zk_elgamal_proof_interface::proof_data::{self as zk, ZkProofData};

/// 程序读指令数据的上限,等于一笔交易的最大字节数.
const PACKET_DATA_SIZE: u64 = 1232;

/// 官方类型的名字.serde 只给出类型名,地址和哈希按它们的官方类型显示.
const ADDRESS_TYPES: [&str; 2] = ["Address", "Pubkey"];
const HASH_TYPE: &str = "Hash";

/// 这几个原生程序.名字是 `solana-sdk-ids` 里的模块名.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Native {
    Vote,
    LoaderV4,
    Ed25519,
    ZkElGamalProof,
}

/// 一条原生指令解码的结果.`name` 是官方的分支名,预编译程序只有一种指令,没有名字.
pub(super) struct Decoded {
    pub(super) name: Option<String>,
    pub(super) data: Vec<Node>,
    pub(super) check: RoundTrip,
}

impl Native {
    pub(super) const ALL: [Native; 4] = [Native::Vote, Native::LoaderV4, Native::Ed25519, Native::ZkElGamalProof];

    pub(super) fn of(program: &[u8; 32]) -> Option<Self> {
        Self::ALL.into_iter().find(|native| native.id() == *program)
    }

    fn id(self) -> [u8; 32] {
        let id = match self {
            Native::Vote => solana_sdk_ids::vote::ID,
            Native::LoaderV4 => solana_sdk_ids::loader_v4::ID,
            Native::Ed25519 => solana_sdk_ids::ed25519_program::ID,
            Native::ZkElGamalProof => solana_sdk_ids::zk_elgamal_proof_program::ID,
        };
        id.to_bytes()
    }

    /// 规则表里的程序名.
    pub(super) fn name(self) -> &'static str {
        match self {
            Native::Vote => "vote",
            Native::LoaderV4 => "loader_v4",
            Native::Ed25519 => "ed25519_program",
            Native::ZkElGamalProof => "zk_elgamal_proof_program",
        }
    }

    /// 规则表里的种类名:程序名加官方分支名.
    pub(super) fn action(self, name: Option<&str>) -> String {
        match name {
            Some(name) => [self.name(), name].join("."),
            None => self.name().to_string(),
        }
    }

    /// 这个程序全部指令在规则表里的种类名.分支名来自官方类型:serde 的分支表,或者官方枚举的编号.
    pub(super) fn actions(self) -> Vec<String> {
        let names: Vec<String> = match self {
            Native::Vote => variants::<VoteInstruction>(),
            Native::LoaderV4 => variants::<LoaderV4Instruction>(),
            Native::Ed25519 => return vec![self.action(None)],
            Native::ZkElGamalProof => (0..=u8::MAX)
                .filter_map(|tag| ProofInstruction::instruction_type(&[tag]))
                .map(|instruction| format!("{instruction:?}"))
                .collect(),
        };
        names.iter().map(|name| self.action(Some(name))).collect()
    }

    /// 读不出官方类型时返回 `None`,整段数据按 `opaque` 显示,类别是无法识别.
    pub(super) fn decode(self, data: &Field<'_>) -> Result<Option<Decoded>, Reject> {
        match self {
            Native::Vote => bincoded(data, vote_fields),
            Native::LoaderV4 => bincoded(data, |instruction: &LoaderV4Instruction| serde_fields(instruction)),
            Native::Ed25519 => Ok(ed25519(data)),
            Native::ZkElGamalProof => Ok(zk_proof(data)),
        }
    }
}

/// 官方枚举的全部分支名.serde 的派生在读枚举时给出分支表,这里读到它就停下.
fn variants<T: serde::de::DeserializeOwned>() -> Vec<String> {
    let mut probe = Probe(Vec::new());
    let _ = T::deserialize(&mut probe);
    probe.0.into_iter().map(str::to_string).collect()
}

struct Probe(Vec<&'static str>);

impl<'de> serde::Deserializer<'de> for &mut Probe {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: serde::de::Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        Err(serde::de::Error::custom("只读分支表"))
    }

    fn deserialize_enum<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        variants: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.extend_from_slice(variants);
        Err(serde::de::Error::custom("只读分支表"))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

fn own(name: &str, value: Value, span: Span) -> Node {
    Node::new(name, value, Source::Signed(Extent::Own(span)))
}

fn inherited(name: &str, value: Value) -> Node {
    Node::new(name, value, Source::Signed(Extent::Inherited))
}

fn uint(value: u128) -> Value {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    Value::Uint(bytes[start..].to_vec())
}

/// 程序读指令数据的方式:定长整数,长度上限是一笔交易,允许尾部字节.
fn options() -> impl Options {
    bincode::options()
        .with_limit(PACKET_DATA_SIZE)
        .with_fixint_encoding()
        .allow_trailing_bytes()
}

/// 读官方枚举.`fields` 给出分支名和字段节点.尾部字节程序本身会忽略,作为 `opaque` 显示.
fn bincoded<T>(
    data: &Field<'_>,
    fields: impl Fn(&T) -> Result<(String, Vec<Node>), Reject>,
) -> Result<Option<Decoded>, Reject>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let mut reader = data.bytes;
    let Ok(instruction) = options().deserialize_from::<_, T>(&mut reader) else {
        return Ok(None);
    };
    let used = data.bytes.len() - reader.len();
    let rebuilt = options().serialize(&instruction).map_err(|_| Reject::RoundTrip)?;
    let mut cursor = Cursor::new(data);
    let body = cursor.take(used as u64)?;
    let trailing = cursor.rest();
    let (name, children) = fields(&instruction)?;
    let mut nodes = vec![own(&name, Value::Struct, body.span).children(children)];
    if !trailing.bytes.is_empty() {
        nodes.push(own(
            "trailing_data",
            Value::Opaque(trailing.bytes.to_vec()),
            trailing.span,
        ));
    }
    let mut whole = rebuilt;
    whole.extend_from_slice(trailing.bytes);
    Ok(Some(Decoded {
        name: Some(name),
        data: nodes,
        check: RoundTrip {
            original: data.bytes.to_vec(),
            rebuilt: whole,
        },
    }))
}

/// 官方枚举的分支名和字段.
fn serde_fields(value: &impl Serialize) -> Result<(String, Vec<Node>), Reject> {
    match value.serialize(Tree)? {
        Shape::Variant(name, content) => Ok((name.to_string(), content.fields())),
        Shape::Byte(_) | Shape::Leaf(_) | Shape::Fields(_) | Shape::Items(_) => Err(Reject::Malformed),
    }
}

/// 紧凑编码的 Vote 分支在官方 serde 里写的是线格式,字段按它们自己的官方类型读,其余分支按官方枚举.
fn vote_fields(instruction: &VoteInstruction) -> Result<(String, Vec<Node>), Reject> {
    let (name, fields) = serde_fields(instruction)?;
    let semantic = match instruction {
        VoteInstruction::CompactUpdateVoteState(update) => Some(vec![shape_of(update)?.node("0")]),
        VoteInstruction::CompactUpdateVoteStateSwitch(update, hash) => {
            Some(vec![shape_of(update)?.node("0"), shape_of(hash)?.node("1")])
        }
        VoteInstruction::TowerSync(sync) => Some(vec![shape_of(sync)?.node("0")]),
        VoteInstruction::TowerSyncSwitch(sync, hash) => {
            Some(vec![shape_of(sync)?.node("0"), shape_of(hash)?.node("1")])
        }
        VoteInstruction::InitializeAccount(_)
        | VoteInstruction::Authorize(_, _)
        | VoteInstruction::Vote(_)
        | VoteInstruction::Withdraw(_)
        | VoteInstruction::UpdateValidatorIdentity
        | VoteInstruction::UpdateCommission(_)
        | VoteInstruction::VoteSwitch(_, _)
        | VoteInstruction::AuthorizeChecked(_)
        | VoteInstruction::UpdateVoteState(_)
        | VoteInstruction::UpdateVoteStateSwitch(_, _)
        | VoteInstruction::AuthorizeWithSeed(_)
        | VoteInstruction::AuthorizeCheckedWithSeed(_)
        | VoteInstruction::InitializeAccountV2(_)
        | VoteInstruction::UpdateCommissionCollector(_) => None,
        VoteInstruction::UpdateCommissionBps { commission_bps, kind } => {
            let _ = (commission_bps, kind);
            None
        }
        VoteInstruction::DepositDelegatorRewards { deposit } => {
            let _ = deposit;
            None
        }
    };
    Ok((name, semantic.unwrap_or(fields)))
}

fn shape_of(value: &impl Serialize) -> Result<Shape, Reject> {
    Ok(value.serialize(Tree)?)
}

/// serde 看到的官方类型.
enum Shape {
    /// `u8`.全是 `u8` 的元组是定长字节数组.
    Byte(u8),
    Leaf(Value),
    Fields(Vec<(String, Shape)>),
    Items(Vec<Shape>),
    Variant(&'static str, Box<Shape>),
}

impl Shape {
    /// 分支的内容.结构体按字段名,元组按位置.
    fn fields(self) -> Vec<Node> {
        match self {
            Shape::Fields(fields) => fields.into_iter().map(|(name, shape)| shape.node(&name)).collect(),
            Shape::Items(items) => positional(items),
            Shape::Leaf(Value::Struct) => Vec::new(),
            Shape::Byte(byte) => vec![inherited("0", uint(byte.into()))],
            Shape::Leaf(value) => vec![inherited("0", value)],
            Shape::Variant(name, content) => vec![Shape::Variant(name, content).node("0")],
        }
    }

    fn node(self, name: &str) -> Node {
        match self {
            Shape::Byte(byte) => inherited(name, uint(byte.into())),
            Shape::Leaf(value) => inherited(name, value),
            Shape::Fields(fields) => {
                let children = fields.into_iter().map(|(name, shape)| shape.node(&name)).collect();
                inherited(name, Value::Struct).children(children)
            }
            Shape::Items(items) => {
                inherited(name, Value::List).children(items.into_iter().map(|item| item.node(ITEM)).collect())
            }
            Shape::Variant(variant, content) => {
                let children = content.fields();
                inherited(name, Value::Enum(variant.to_string())).children(children)
            }
        }
    }
}

/// 列表的每一项.
const ITEM: &str = "item";

fn positional(items: Vec<Shape>) -> Vec<Node> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| item.node(&index.to_string()))
        .collect()
}

/// 全是 `u8` 的元组是定长字节数组.
fn bytes_of_items(items: &[Shape]) -> Option<Vec<u8>> {
    items
        .iter()
        .map(|item| match item {
            Shape::Byte(byte) => Some(*byte),
            Shape::Leaf(_) | Shape::Fields(_) | Shape::Items(_) | Shape::Variant(_, _) => None,
        })
        .collect()
}

#[derive(Debug)]
struct Unsupported;

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("官方类型里有本机不显示的值")
    }
}

impl std::error::Error for Unsupported {}

impl ser::Error for Unsupported {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Unsupported
    }
}

impl From<Unsupported> for Reject {
    fn from(_: Unsupported) -> Self {
        Reject::Malformed
    }
}

/// 把官方类型的 serde 输出变成 `Shape`.
struct Tree;

struct Collect {
    items: Vec<Shape>,
}

struct Named {
    fields: Vec<(String, Shape)>,
}

struct VariantCollect {
    variant: &'static str,
    items: Vec<Shape>,
}

struct VariantNamed {
    variant: &'static str,
    fields: Vec<(String, Shape)>,
}

impl ser::Serializer for Tree {
    type Ok = Shape;
    type Error = Unsupported;
    type SerializeSeq = Collect;
    type SerializeTuple = Collect;
    type SerializeTupleStruct = Collect;
    type SerializeTupleVariant = VariantCollect;
    type SerializeMap = Impossible<Shape, Unsupported>;
    type SerializeStruct = Named;
    type SerializeStructVariant = VariantNamed;

    fn serialize_bool(self, v: bool) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Bool(v)))
    }
    fn serialize_i8(self, v: i8) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Int(v.into())))
    }
    fn serialize_i16(self, v: i16) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Int(v.into())))
    }
    fn serialize_i32(self, v: i32) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Int(v.into())))
    }
    fn serialize_i64(self, v: i64) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Int(v.into())))
    }
    fn serialize_i128(self, v: i128) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Int(v)))
    }
    fn serialize_u8(self, v: u8) -> Result<Shape, Unsupported> {
        Ok(Shape::Byte(v))
    }
    fn serialize_u16(self, v: u16) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(uint(v.into())))
    }
    fn serialize_u32(self, v: u32) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(uint(v.into())))
    }
    fn serialize_u64(self, v: u64) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(uint(v.into())))
    }
    fn serialize_u128(self, v: u128) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(uint(v)))
    }
    fn serialize_f32(self, _: f32) -> Result<Shape, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_f64(self, _: f64) -> Result<Shape, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_char(self, v: char) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Text(v.to_string())))
    }
    fn serialize_str(self, v: &str) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Text(v.to_string())))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Bytes(v.to_vec())))
    }
    fn serialize_none(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Enum(ABSENT_TEXT.to_string())))
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Shape, Unsupported> {
        value.serialize(Tree)
    }
    fn serialize_unit(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Struct))
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Shape, Unsupported> {
        Ok(Shape::Leaf(Value::Struct))
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Shape, Unsupported> {
        Ok(Shape::Variant(variant, Box::new(Shape::Leaf(Value::Struct))))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Shape, Unsupported> {
        let inner = value.serialize(Tree)?;
        let bytes = match &inner {
            Shape::Items(items) => bytes_of_items(items),
            Shape::Leaf(Value::Bytes(bytes)) => Some(bytes.clone()),
            Shape::Byte(_) | Shape::Leaf(_) | Shape::Fields(_) | Shape::Variant(_, _) => None,
        };
        Ok(match bytes {
            Some(bytes) if ADDRESS_TYPES.contains(&name) && bytes.len() == 32 => {
                Shape::Leaf(Value::Address(base58::encode(&bytes)))
            }
            Some(bytes) if name == HASH_TYPE => Shape::Leaf(Value::Hash(bytes)),
            Some(_) | None => inner,
        })
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Shape, Unsupported> {
        let inner = value.serialize(Tree)?;
        Ok(Shape::Variant(variant, Box::new(Shape::Items(vec![inner]))))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Collect, Unsupported> {
        Ok(Collect {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<Collect, Unsupported> {
        Ok(Collect {
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Collect, Unsupported> {
        Ok(Collect {
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantCollect, Unsupported> {
        Ok(VariantCollect {
            variant,
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Impossible<Shape, Unsupported>, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Named, Unsupported> {
        Ok(Named {
            fields: Vec::with_capacity(len),
        })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantNamed, Unsupported> {
        Ok(VariantNamed {
            variant,
            fields: Vec::with_capacity(len),
        })
    }
}

impl Collect {
    fn finish(self) -> Shape {
        match bytes_of_items(&self.items) {
            Some(bytes) if !self.items.is_empty() => Shape::Leaf(Value::Bytes(bytes)),
            Some(_) | None => Shape::Items(self.items),
        }
    }
}

impl ser::SerializeSeq for Collect {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.items.push(value.serialize(Tree)?);
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Items(self.items))
    }
}

impl ser::SerializeTuple for Collect {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.items.push(value.serialize(Tree)?);
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for Collect {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.items.push(value.serialize(Tree)?);
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Items(self.items))
    }
}

impl ser::SerializeTupleVariant for VariantCollect {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unsupported> {
        self.items.push(value.serialize(Tree)?);
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Variant(self.variant, Box::new(Shape::Items(self.items))))
    }
}

impl ser::SerializeStruct for Named {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Unsupported> {
        self.fields.push((key.to_string(), value.serialize(Tree)?));
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Fields(self.fields))
    }
}

impl ser::SerializeStructVariant for VariantNamed {
    type Ok = Shape;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Unsupported> {
        self.fields.push((key.to_string(), value.serialize(Tree)?));
        Ok(())
    }
    fn end(self) -> Result<Shape, Unsupported> {
        Ok(Shape::Variant(self.variant, Box::new(Shape::Fields(self.fields))))
    }
}

// Ed25519 预编译.数据是签名数,一个填充字节,若干组偏移,其余字节是偏移指向的内容.
// 偏移指向别的指令时内容不在这里.Ed25519 用 `u16::MAX` 表示本条指令,这时把签名,公钥,消息取出来显示.

/// Ed25519 偏移里表示本条指令的编号.
const CURRENT_INSTRUCTION: u16 = u16::MAX;

/// 本条指令里 `offset` 开始的 `len` 字节.
fn slice(data: &[u8], index: u16, offset: u16, len: usize) -> Option<&[u8]> {
    if index != CURRENT_INSTRUCTION {
        return None;
    }
    let start = usize::from(offset);
    data.get(start..start.checked_add(len)?)
}

/// 读出的偏移组,和由官方结构体重新编码的字节.
struct Precompile<'a> {
    nodes: Vec<Node>,
    offsets: Vec<Node>,
    shown: Vec<Node>,
    rebuilt: Vec<u8>,
    cursor: Cursor<'a>,
}

impl<'a> Precompile<'a> {
    /// 签名数和填充字节.
    fn header(data: &Field<'a>) -> Option<(Self, u8)> {
        let mut cursor = Cursor::new(data);
        let count = cursor.take(1).ok()?;
        let padding = cursor.take(1).ok()?;
        let nodes = vec![
            own("num_signatures", Value::Uint(count.bytes.to_vec()), count.span),
            own("padding", Value::Uint(padding.bytes.to_vec()), padding.span),
        ];
        let header = Self {
            nodes,
            offsets: Vec::new(),
            shown: Vec::new(),
            rebuilt: [count.bytes, padding.bytes].concat(),
            cursor,
        };
        Some((header, count.bytes[0]))
    }

    /// 一组偏移.字段名来自官方结构体的解构,`encoded` 是官方结构体重新编码的字节.
    fn group(&mut self, group: Field<'_>, rows: &[(&str, u16)], encoded: &[u8]) {
        let children = rows
            .iter()
            .map(|(name, value)| inherited(name, uint(u128::from(*value))))
            .collect();
        self.offsets
            .push(own("signature_offsets", Value::Struct, group.span).children(children));
        self.rebuilt.extend_from_slice(encoded);
    }

    fn show(&mut self, name: &str, bytes: Option<&[u8]>, value: fn(&[u8]) -> Value) {
        self.shown.extend(bytes.map(|bytes| inherited(name, value(bytes))));
    }

    fn finish(mut self, data: &Field<'_>) -> Decoded {
        let rest = self.cursor.rest();
        self.rebuilt.extend_from_slice(rest.bytes);
        self.nodes
            .push(Node::new("offsets", Value::List, Source::Signed(Extent::Container)).children(self.offsets));
        self.nodes
            .push(own("data", Value::Bytes(rest.bytes.to_vec()), rest.span).children(self.shown));
        Decoded {
            name: None,
            data: self.nodes,
            check: RoundTrip {
                original: data.bytes.to_vec(),
                rebuilt: self.rebuilt,
            },
        }
    }
}

fn ed25519(data: &Field<'_>) -> Option<Decoded> {
    use solana_ed25519_program::{
        Ed25519SignatureOffsets, PUBKEY_SERIALIZED_SIZE, SIGNATURE_OFFSETS_SERIALIZED_SIZE, SIGNATURE_SERIALIZED_SIZE,
    };
    let (mut precompile, count) = Precompile::header(data)?;
    for _ in 0..count {
        let group = precompile.cursor.take(SIGNATURE_OFFSETS_SERIALIZED_SIZE as u64).ok()?;
        let parsed: Ed25519SignatureOffsets = *bytemuck::try_from_bytes(group.bytes).ok()?;
        let Ed25519SignatureOffsets {
            signature_offset,
            signature_instruction_index,
            public_key_offset,
            public_key_instruction_index,
            message_data_offset,
            message_data_size,
            message_instruction_index,
        } = parsed;
        let rows = [
            ("signature_offset", signature_offset),
            ("signature_instruction_index", signature_instruction_index),
            ("public_key_offset", public_key_offset),
            ("public_key_instruction_index", public_key_instruction_index),
            ("message_data_offset", message_data_offset),
            ("message_data_size", message_data_size),
            ("message_instruction_index", message_instruction_index),
        ];
        precompile.group(group, &rows, bytemuck::bytes_of(&parsed));
        let signature = slice(
            data.bytes,
            signature_instruction_index,
            signature_offset,
            SIGNATURE_SERIALIZED_SIZE,
        );
        let key = slice(
            data.bytes,
            public_key_instruction_index,
            public_key_offset,
            PUBKEY_SERIALIZED_SIZE,
        );
        let message = slice(
            data.bytes,
            message_instruction_index,
            message_data_offset,
            usize::from(message_data_size),
        );
        precompile.show("signature", signature, |bytes| Value::Bytes(bytes.to_vec()));
        precompile.show("public_key", key, |bytes| Value::Address(base58::encode(bytes)));
        precompile.show("message", message, |bytes| Value::Bytes(bytes.to_vec()));
    }
    Some(precompile.finish(data))
}

// ZK ElGamal Proof.第一个字节是官方枚举的编号.证明在指令数据里时,其余字节正好是这种证明的官方结构体.
// 证明在账户里时,其余 4 个字节是账户里的偏移,和官方的 `encode_verify_proof_from_account` 相同.

/// 证明在账户里时数据的长度.
const PROOF_IN_ACCOUNT: usize = 1 + 4;

fn zk_proof(data: &Field<'_>) -> Option<Decoded> {
    use num_traits::ToPrimitive;
    let instruction = ProofInstruction::instruction_type(data.bytes)?;
    let name = format!("{instruction:?}");
    let mut cursor = Cursor::new(data);
    let tag = cursor.take(1).ok()?;
    let mut nodes = vec![own("instruction_type", Value::Enum(name.clone()), tag.span)];
    let mut rebuilt = vec![instruction.to_u8()?];
    let rest = cursor.rest();
    if matches!(instruction, ProofInstruction::CloseContextState) {
        if !rest.bytes.is_empty() {
            return None;
        }
    } else if data.bytes.len() == PROOF_IN_ACCOUNT {
        let offset = u32::from_le_bytes(rest.bytes.try_into().ok()?);
        nodes.push(own("offset", uint(offset.into()), rest.span));
        rebuilt.extend_from_slice(&offset.to_le_bytes());
    } else {
        let (fields, encoded) = zk_fields(instruction, &rest)?;
        nodes.extend(fields);
        rebuilt.extend(encoded);
    }
    Some(Decoded {
        name: Some(name),
        data: nodes,
        check: RoundTrip {
            original: data.bytes.to_vec(),
            rebuilt,
        },
    })
}

/// 证明数据按官方结构体读,长度必须正好相同.`context` 是官方 `context_data` 的内容,其余是证明本身.
fn proof<T, U>(rest: &Field<'_>) -> Option<(Vec<Node>, Vec<u8>)>
where
    T: Pod + ZkProofData<U>,
    U: Pod,
{
    let parsed: &T = bytemuck::try_from_bytes(rest.bytes).ok()?;
    let context = bytemuck::bytes_of(parsed.context_data());
    let mut cursor = Cursor::new(rest);
    let context_field = cursor.take(context.len() as u64).ok()?;
    let proof_field = cursor.rest();
    if context_field.bytes != context {
        return None;
    }
    let nodes = vec![
        own(
            "context",
            Value::Bytes(context_field.bytes.to_vec()),
            context_field.span,
        ),
        own("proof", Value::Bytes(proof_field.bytes.to_vec()), proof_field.span),
    ];
    Some((nodes, bytemuck::bytes_of(parsed).to_vec()))
}

fn zk_fields(instruction: ProofInstruction, rest: &Field<'_>) -> Option<(Vec<Node>, Vec<u8>)> {
    match instruction {
        ProofInstruction::CloseContextState => None,
        ProofInstruction::VerifyZeroCiphertext => proof::<zk::ZeroCiphertextProofData, _>(rest),
        ProofInstruction::VerifyCiphertextCiphertextEquality => {
            proof::<zk::CiphertextCiphertextEqualityProofData, _>(rest)
        }
        ProofInstruction::VerifyCiphertextCommitmentEquality => {
            proof::<zk::CiphertextCommitmentEqualityProofData, _>(rest)
        }
        ProofInstruction::VerifyPubkeyValidity => proof::<zk::PubkeyValidityProofData, _>(rest),
        ProofInstruction::VerifyPercentageWithCap => proof::<zk::PercentageWithCapProofData, _>(rest),
        ProofInstruction::VerifyBatchedRangeProofU64 => proof::<zk::BatchedRangeProofU64Data, _>(rest),
        ProofInstruction::VerifyBatchedRangeProofU128 => proof::<zk::BatchedRangeProofU128Data, _>(rest),
        ProofInstruction::VerifyBatchedRangeProofU256 => proof::<zk::BatchedRangeProofU256Data, _>(rest),
        ProofInstruction::VerifyGroupedCiphertext2HandlesValidity => {
            proof::<zk::GroupedCiphertext2HandlesValidityProofData, _>(rest)
        }
        ProofInstruction::VerifyBatchedGroupedCiphertext2HandlesValidity => {
            proof::<zk::BatchedGroupedCiphertext2HandlesValidityProofData, _>(rest)
        }
        ProofInstruction::VerifyGroupedCiphertext3HandlesValidity => {
            proof::<zk::GroupedCiphertext3HandlesValidityProofData, _>(rest)
        }
        ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity => {
            proof::<zk::BatchedGroupedCiphertext3HandlesValidityProofData, _>(rest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_pubkey::Pubkey;
    use solana_vote_interface::state::{TowerSync, VoteAuthorize};

    fn field(bytes: &[u8]) -> Field<'_> {
        Field::whole(bytes)
    }

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|node| node.name.as_str()).collect()
    }

    fn decoded(native: Native, bytes: &[u8]) -> Decoded {
        let decoded = native.decode(&field(bytes)).unwrap().unwrap();
        assert_eq!(decoded.check.original, decoded.check.rebuilt);
        decoded
    }

    #[test]
    fn the_branch_names_come_from_the_official_types() {
        let vote = Native::Vote.actions();
        assert_eq!(vote.len(), 20);
        assert_eq!(vote[1], "vote.Authorize");
        assert!(vote.contains(&"vote.TowerSync".to_string()));
        let loader = Native::LoaderV4.actions();
        assert_eq!(loader[0], "loader_v4.Write");
        assert_eq!(Native::Ed25519.actions(), ["ed25519_program"]);
        let zk = Native::ZkElGamalProof.actions();
        assert_eq!(zk.len(), 13);
        assert_eq!(zk[0], "zk_elgamal_proof_program.CloseContextState");
    }

    #[test]
    fn every_program_id_is_the_official_one() {
        for native in Native::ALL {
            assert_eq!(Native::of(&native.id()), Some(native));
        }
        let vote = base58::decode("Vote111111111111111111111111111111111111111").unwrap();
        assert_eq!(Native::of(&vote.try_into().unwrap()), Some(Native::Vote));
    }

    #[test]
    fn a_vote_authorize_reads_by_the_official_enum() {
        let new = Pubkey::new_from_array([7; 32]);
        let bytes = bincode::serialize(&VoteInstruction::Authorize(new, VoteAuthorize::Withdrawer)).unwrap();
        let decoded = decoded(Native::Vote, &bytes);
        assert_eq!(decoded.name.as_deref(), Some("Authorize"));
        let body = &decoded.data[0];
        assert_eq!(body.name, "Authorize");
        assert_eq!(names(&body.children), ["0", "1"]);
        assert_eq!(body.children[0].value, Value::Address(base58::encode(&[7; 32])));
        assert_eq!(body.children[1].value, Value::Enum("Withdrawer".into()));
    }

    #[test]
    fn a_tower_sync_shows_its_fields_not_the_compact_wire_form() {
        let sync = TowerSync::new_from_slot(42, solana_hash::Hash::new_from_array([3; 32]));
        let bytes = bincode::serialize(&VoteInstruction::TowerSync(sync)).unwrap();
        let decoded = decoded(Native::Vote, &bytes);
        let inner = &decoded.data[0].children[0];
        assert_eq!(
            names(&inner.children),
            ["lockouts", "root", "hash", "timestamp", "block_id"]
        );
        assert_eq!(inner.children[2].value, Value::Hash(vec![3; 32]));
    }

    #[test]
    fn trailing_bytes_are_opaque_and_the_rest_still_round_trips() {
        let mut bytes = bincode::serialize(&VoteInstruction::Withdraw(5)).unwrap();
        bytes.extend_from_slice(&[9, 9]);
        let decoded = decoded(Native::Vote, &bytes);
        assert_eq!(names(&decoded.data), ["Withdraw", "trailing_data"]);
        assert_eq!(decoded.data[1].value, Value::Opaque(vec![9, 9]));
    }

    #[test]
    fn an_unknown_branch_is_left_to_the_caller() {
        assert!(Native::Vote.decode(&field(&[200, 0, 0, 0])).unwrap().is_none());
        assert!(Native::Vote.decode(&field(&[])).unwrap().is_none());
        assert!(Native::ZkElGamalProof.decode(&field(&[200])).unwrap().is_none());
    }

    #[test]
    fn loader_v4_bytes_stay_bytes() {
        let write = LoaderV4Instruction::Write {
            offset: 4,
            bytes: vec![1, 2, 3],
        };
        let decoded = decoded(Native::LoaderV4, &bincode::serialize(&write).unwrap());
        let body = &decoded.data[0];
        assert_eq!(names(&body.children), ["offset", "bytes"]);
        assert_eq!(body.children[1].value, Value::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn an_ed25519_instruction_shows_what_its_offsets_point_at() {
        use solana_ed25519_program::{DATA_START, Ed25519SignatureOffsets};
        let key = [5u8; 32];
        let signature = [6u8; 64];
        let message = b"hello";
        let key_at = DATA_START as u16;
        let signature_at = key_at + 32;
        let message_at = signature_at + 64;
        let offsets = Ed25519SignatureOffsets {
            signature_offset: signature_at,
            signature_instruction_index: u16::MAX,
            public_key_offset: key_at,
            public_key_instruction_index: u16::MAX,
            message_data_offset: message_at,
            message_data_size: message.len() as u16,
            message_instruction_index: u16::MAX,
        };
        let mut bytes = vec![1, 0];
        bytes.extend_from_slice(bytemuck::bytes_of(&offsets));
        bytes.extend_from_slice(&key);
        bytes.extend_from_slice(&signature);
        bytes.extend_from_slice(message);
        let decoded = decoded(Native::Ed25519, &bytes);
        assert_eq!(names(&decoded.data), ["num_signatures", "padding", "offsets", "data"]);
        let shown = &decoded.data[3].children;
        assert_eq!(names(shown), ["signature", "public_key", "message"]);
        assert_eq!(shown[1].value, Value::Address(base58::encode(&key)));
        assert_eq!(shown[2].value, Value::Bytes(message.to_vec()));
    }

    #[test]
    fn a_short_precompile_is_left_to_the_caller() {
        assert!(Native::Ed25519.decode(&field(&[1, 0, 1, 2])).unwrap().is_none());
        assert!(Native::Ed25519.decode(&field(&[])).unwrap().is_none());
    }

    #[test]
    fn a_proof_in_an_account_is_an_offset() {
        let decoded = decoded(Native::ZkElGamalProof, &[1, 8, 0, 0, 0]);
        assert_eq!(decoded.name.as_deref(), Some("VerifyZeroCiphertext"));
        assert_eq!(names(&decoded.data), ["instruction_type", "offset"]);
        assert_eq!(decoded.data[1].value, Value::Uint(vec![8]));
    }

    #[test]
    fn a_proof_in_the_data_must_be_the_whole_official_structure() {
        let size = std::mem::size_of::<zk::ZeroCiphertextProofData>();
        let mut bytes = vec![1];
        bytes.resize(1 + size, 0);
        let decoded = decoded(Native::ZkElGamalProof, &bytes);
        assert_eq!(names(&decoded.data), ["instruction_type", "context", "proof"]);
        bytes.push(0);
        assert!(Native::ZkElGamalProof.decode(&field(&bytes)).unwrap().is_none());
    }

    /// 规则表从数据里读的路径都要落在官方类型上,账户按官方构建函数的位置.官方类型改字段名时这里失败.
    #[test]
    fn every_native_summary_path_is_in_the_official_type() {
        use solana_vote_interface::state::{VoteAuthorizeWithSeedArgs, VoteInit, VoteInitV2};
        let key = Pubkey::new_from_array([1; 32]);
        let samples: Vec<(&str, Vec<u8>)> = vec![
            (
                "vote.InitializeAccount",
                bincode::serialize(&VoteInstruction::InitializeAccount(VoteInit::default())).unwrap(),
            ),
            (
                "vote.Authorize",
                bincode::serialize(&VoteInstruction::Authorize(key, VoteAuthorize::Voter)).unwrap(),
            ),
            (
                "vote.Withdraw",
                bincode::serialize(&VoteInstruction::Withdraw(1)).unwrap(),
            ),
            (
                "vote.AuthorizeWithSeed",
                bincode::serialize(&VoteInstruction::AuthorizeWithSeed(VoteAuthorizeWithSeedArgs {
                    authorization_type: VoteAuthorize::Voter,
                    current_authority_derived_key_owner: key,
                    current_authority_derived_key_seed: "seed".into(),
                    new_authority: key,
                }))
                .unwrap(),
            ),
            (
                "vote.InitializeAccountV2",
                bincode::serialize(&VoteInstruction::InitializeAccountV2(VoteInitV2 {
                    node_pubkey: key,
                    authorized_voter: key,
                    authorized_voter_bls_pubkey: [0; 48],
                    authorized_voter_bls_proof_of_possession: [0; 96],
                    authorized_withdrawer: key,
                    inflation_rewards_commission_bps: 0,
                    block_revenue_commission_bps: 0,
                }))
                .unwrap(),
            ),
            (
                "vote.DepositDelegatorRewards",
                bincode::serialize(&VoteInstruction::DepositDelegatorRewards { deposit: 1 }).unwrap(),
            ),
        ];
        let rules = crate::chains::solana::policy::rules();
        for native in Native::ALL {
            for action in native.actions() {
                let Some(fields) = rules.summary_fields(&action) else {
                    continue;
                };
                let paths = [&fields.amount, &fields.party, &fields.next, &fields.current];
                for path in paths.into_iter().flatten() {
                    let parts: Vec<&str> = path.split('.').collect();
                    match parts.as_slice() {
                        ["accounts", index] => assert!(index.parse::<usize>().is_ok(), "{action}: {path}"),
                        ["data", rest @ ..] => {
                            let (_, bytes) = samples
                                .iter()
                                .find(|(kind, _)| *kind == action)
                                .unwrap_or_else(|| panic!("{action} 没有样本"));
                            let decoded = decoded(native, bytes);
                            let found = rest.iter().try_fold(decoded.data.as_slice(), |nodes, name| {
                                nodes
                                    .iter()
                                    .find(|node| node.name == *name)
                                    .map(|node| node.children.as_slice())
                            });
                            assert!(found.is_some(), "{action}: {path}");
                        }
                        other => panic!("{action}: {other:?}"),
                    }
                }
            }
        }
    }
}

//! Codama 类型节点的通用遍历器.指令和类型都在 assets/solana-idl.json,由 tools/crosscheck/codama/generate.mjs
//! 从 schemas/solana/ 生成,这里只认节点的种类.新增指令改的是 schema 和规则表,不是这个文件.
//!
//! 每个值都由读出的结果重新编码,结果和原字节比较,所以这里接受的编码都是规范的.见 docs/10-tx-decoding.md 第 2.2 节.

use super::psbt::Cursor;
use super::psbt_values::Field;
use super::{Amount, Coin, Extent, Node, Reject, Source, Span, Value};
use bitcoin::base58;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Idl {
    /// schema 文件的 SHA-256.`tools/crosscheck` 和 `schemas/lock.toml` 比较.
    sources: BTreeMap<String, String>,
    programs: Vec<Program>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Program {
    pub(super) name: String,
    ids: Vec<String>,
    /// 程序把空数据当成这条指令.
    empty_data: Option<String>,
    pub(super) batch: Option<Batch>,
    defined_types: BTreeMap<String, TypeNode>,
    instructions: Vec<Instruction>,
}

/// 内部指令在 IDL 里是字节数组,递归解码是遍历器的规则.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Batch {
    pub(super) instruction: String,
    pub(super) argument: String,
    pub(super) accounts: String,
    pub(super) payload: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Instruction {
    pub(super) name: String,
    pub(super) accounts: Vec<Account>,
    pub(super) arguments: Vec<Argument>,
    discriminators: Vec<Discriminator>,
    pub(super) remaining: Vec<Remaining>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Account {
    pub(super) name: String,
    pub(super) optional: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Argument {
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) node: TypeNode,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discriminator {
    offset: usize,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Remaining {
    pub(super) name: String,
    /// 这一组只收签名者.否则收下后面全部账户.
    pub(super) signer: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum NumberFormat {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I64,
    F64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NumberDisplay {
    decimals: Option<u8>,
    unit: Option<String>,
    #[serde(default)]
    time: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StructField {
    name: String,
    #[serde(rename = "type")]
    node: TypeNode,
}

/// C3:每个节点种类都是一个分支,`generate.mjs` 遇到别的种类时失败.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum TypeNode {
    #[serde(rename = "numberTypeNode")]
    Number {
        format: NumberFormat,
        display: Option<NumberDisplay>,
    },
    #[serde(rename = "publicKeyTypeNode")]
    PublicKey,
    #[serde(rename = "bytesTypeNode")]
    Bytes,
    #[serde(rename = "booleanTypeNode")]
    Boolean { size: Box<TypeNode> },
    #[serde(rename = "stringTypeNode")]
    Text,
    #[serde(rename = "sizePrefixTypeNode")]
    SizePrefix {
        prefix: Box<TypeNode>,
        #[serde(rename = "type")]
        node: Box<TypeNode>,
    },
    #[serde(rename = "fixedSizeTypeNode")]
    FixedSize {
        size: u64,
        #[serde(rename = "type")]
        node: Box<TypeNode>,
    },
    #[serde(rename = "optionTypeNode")]
    Optional { prefix: Box<TypeNode>, item: Box<TypeNode> },
    #[serde(rename = "zeroableOptionTypeNode")]
    ZeroableOption { item: Box<TypeNode> },
    #[serde(rename = "arrayTypeNode")]
    Array { item: Box<TypeNode>, count: CountNode },
    #[serde(rename = "structTypeNode")]
    Struct { fields: Vec<StructField> },
    #[serde(rename = "tupleTypeNode")]
    Tuple { items: Vec<TypeNode> },
    #[serde(rename = "enumTypeNode")]
    Enum {
        size: Box<TypeNode>,
        variants: Vec<Variant>,
    },
    #[serde(rename = "definedTypeLinkNode")]
    DefinedTypeLink { name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum CountNode {
    #[serde(rename = "fixedCountNode")]
    Fixed { value: u64 },
    #[serde(rename = "prefixedCountNode")]
    Prefixed { prefix: Box<TypeNode> },
    #[serde(rename = "remainderCountNode")]
    Remainder,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum Variant {
    #[serde(rename = "enumEmptyVariantTypeNode")]
    Empty { name: String, discriminator: Option<u64> },
    #[serde(rename = "enumStructVariantTypeNode")]
    Struct {
        name: String,
        discriminator: Option<u64>,
        #[serde(rename = "struct")]
        body: TypeNode,
    },
    #[serde(rename = "enumTupleVariantTypeNode")]
    Tuple {
        name: String,
        discriminator: Option<u64>,
        tuple: TypeNode,
    },
}

impl Variant {
    /// Codama 的约定:没写判别值时就是序号.
    fn tag(&self, index: usize) -> u64 {
        let declared = match self {
            Self::Empty {
                name: _name,
                discriminator,
            } => discriminator,
            Self::Struct {
                name: _name,
                discriminator,
                body: _body,
            } => discriminator,
            Self::Tuple {
                name: _name,
                discriminator,
                tuple: _tuple,
            } => discriminator,
        };
        declared.unwrap_or(index as u64)
    }
}

static IDL: OnceLock<Idl> = OnceLock::new();

fn idl() -> &'static Idl {
    IDL.get_or_init(|| serde_json::from_str(include_str!("../../assets/solana-idl.json")).expect("内置 IDL 有效"))
}

pub(super) fn programs() -> &'static [Program] {
    &idl().programs
}

pub(super) fn sources() -> &'static BTreeMap<String, String> {
    &idl().sources
}

pub(super) fn program(id: &[u8; 32]) -> Option<&'static Program> {
    let text = base58::encode(id);
    programs().iter().find(|program| program.ids.contains(&text))
}

impl Program {
    pub(super) fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }

    /// 只有一条指令的程序可以没有判别值,比如 Memo.
    pub(super) fn identify(&self, data: &[u8]) -> Option<&Instruction> {
        if data.is_empty()
            && let Some(name) = &self.empty_data
        {
            return self.instructions.iter().find(|instruction| instruction.name == *name);
        }
        match self.instructions.as_slice() {
            [only] if only.discriminators.is_empty() => Some(only),
            all => all.iter().find(|instruction| instruction.matches(data)),
        }
    }
}

impl Instruction {
    fn matches(&self, data: &[u8]) -> bool {
        !self.discriminators.is_empty()
            && self.discriminators.iter().all(|field| {
                let end = field.offset + field.bytes.len();
                data.get(field.offset..end) == Some(field.bytes.as_slice())
            })
    }

    pub(super) fn required_accounts(&self) -> usize {
        self.accounts.iter().filter(|account| !account.optional).count()
    }
}

impl NumberFormat {
    fn size(self) -> u64 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }

    fn signed(self) -> bool {
        match self {
            Self::I8 | Self::I16 | Self::I64 => true,
            Self::U8 | Self::U16 | Self::U32 | Self::U64 | Self::F64 => false,
        }
    }

    fn float(self) -> bool {
        match self {
            Self::F64 => true,
            Self::U8 | Self::U16 | Self::U32 | Self::U64 | Self::I8 | Self::I16 | Self::I64 => false,
        }
    }

    /// 小端整数的值.符号扩展后 `i128` 放得下全部格式.
    fn read(self, bytes: &[u8]) -> i128 {
        let mut wide = [0u8; 16];
        wide[..bytes.len()].copy_from_slice(bytes);
        let value = u128::from_le_bytes(wide);
        let width = self.size() as u32 * 8;
        if self.signed() {
            let shift = 128 - width;
            ((value << shift) as i128) >> shift
        } else {
            value as i128
        }
    }

    fn encode(self, value: i128) -> Vec<u8> {
        value.to_le_bytes()[..self.size() as usize].to_vec()
    }
}

fn own(name: &str, value: Value, span: Span) -> Node {
    Node::new(name, value, Source::Signed(Extent::Own(span)))
}

fn group(name: &str, value: Value, children: Vec<Node>) -> Node {
    Node::new(name, value, Source::Signed(Extent::Container)).children(children)
}

fn trimmed(value: u128) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    bytes[start..].to_vec()
}

const MILLIS_PER_SECOND: i64 = 1000;

/// 一条指令的数据.`rebuilt` 是由读出的值重新编码的字节.
pub(super) struct TypeWalk<'a> {
    pub(super) program: &'a Program,
    pub(super) coin: Option<Coin>,
    pub(super) native_unit: &'a str,
    pub(super) none: &'a str,
    pub(super) item: &'a str,
    pub(super) variant: &'a str,
    pub(super) rebuilt: Vec<u8>,
}

impl<'a> TypeWalk<'a> {
    fn number_value(&self, format: NumberFormat, number: i128, display: Option<&NumberDisplay>) -> Value {
        let plain = if format.signed() {
            Value::Int(number)
        } else {
            Value::Uint(trimmed(number as u128))
        };
        let Some(NumberDisplay { decimals, unit, time }) = display else {
            return plain;
        };
        if *time {
            return i64::try_from(number)
                .ok()
                .and_then(|seconds| seconds.checked_mul(MILLIS_PER_SECOND))
                .map_or(plain, Value::Time);
        }
        if format.signed() {
            return plain;
        }
        let raw = trimmed(number as u128);
        // 本机不认识的网络没有原生币的单位,金额显示原始整数.
        if unit.as_deref() == Some(self.native_unit) {
            return Value::Amount(Amount {
                raw,
                decimals: self.coin.map(|coin| coin.decimals),
                unit: self.coin.map(|coin| coin.unit.to_string()),
            });
        }
        Value::Amount(Amount {
            raw,
            decimals: *decimals,
            unit: unit.clone(),
        })
    }

    fn take_number(&mut self, format: NumberFormat, cursor: &mut Cursor<'_>) -> Result<(i128, Span), Reject> {
        let field = cursor.take(format.size())?;
        let number = format.read(field.bytes);
        self.rebuilt.extend_from_slice(&format.encode(number));
        Ok((number, field.span))
    }

    /// 长度,计数和枚举标签都是无符号整数.
    fn take_count(&mut self, node: &TypeNode, cursor: &mut Cursor<'_>) -> Result<(u64, Span), Reject> {
        let format = self.number_format(node)?;
        if format.signed() || format.float() {
            return Err(Reject::Malformed);
        }
        let (number, span) = self.take_number(format, cursor)?;
        let count = u64::try_from(number).map_err(|_| Reject::Malformed)?;
        Ok((count, span))
    }

    fn number_format(&self, node: &TypeNode) -> Result<NumberFormat, Reject> {
        let TypeNode::Number {
            format,
            display: _display,
        } = self.resolved(node)?
        else {
            return Err(Reject::Malformed);
        };
        Ok(*format)
    }

    fn linked(&self, name: &str) -> Result<&'a TypeNode, Reject> {
        self.program.defined_types.get(name).ok_or(Reject::Malformed)
    }

    /// 顺着类型链接走到真正的节点.
    fn resolved<'n>(&self, node: &'n TypeNode) -> Result<&'n TypeNode, Reject>
    where
        'a: 'n,
    {
        if let TypeNode::DefinedTypeLink { name } = node {
            self.resolved(self.linked(name)?)
        } else {
            Ok(node)
        }
    }

    /// 定长类型的字节数.
    fn fixed_size(&self, node: &TypeNode) -> Result<u64, Reject> {
        let resolved = self.resolved(node)?;
        if let TypeNode::PublicKey = resolved {
            return Ok(PUBLIC_KEY_BYTES);
        }
        if let TypeNode::FixedSize { size, node: _node } = resolved {
            return Ok(*size);
        }
        self.number_format(resolved).map(NumberFormat::size)
    }

    /// 读一个值.文本不是合法的 UTF-8 时是 `InvalidText`,其余读不完都是 `Malformed`.
    pub(super) fn read(&mut self, name: &str, node: &TypeNode, cursor: &mut Cursor<'_>) -> Result<Node, Reject> {
        match node {
            TypeNode::Number { format, display } => self.number(name, *format, display.as_ref(), cursor),
            TypeNode::PublicKey => self.public_key(name, cursor),
            TypeNode::Bytes => {
                let field = cursor.rest();
                self.rebuilt.extend_from_slice(field.bytes);
                Ok(own(name, Value::Bytes(field.bytes.to_vec()), field.span))
            }
            TypeNode::Boolean { size } => self.boolean(name, size, cursor),
            TypeNode::Text => {
                let field = cursor.rest();
                let text = std::str::from_utf8(field.bytes).map_err(|_| Reject::InvalidText)?;
                self.rebuilt.extend_from_slice(text.as_bytes());
                Ok(own(name, Value::Text(text.to_string()), field.span))
            }
            TypeNode::SizePrefix { prefix, node } => self.size_prefixed(name, prefix, node, cursor),
            TypeNode::FixedSize { size, node } => {
                let field = cursor.take(*size)?;
                self.bounded(name, node, &field)
            }
            TypeNode::Optional { prefix, item } => self.option(name, prefix, item, cursor),
            TypeNode::ZeroableOption { item } => self.zeroable(name, item, cursor),
            TypeNode::Array { item, count } => self.array(name, item, count, cursor),
            TypeNode::Struct { fields } => {
                let children = fields
                    .iter()
                    .map(|field| self.read(&field.name, &field.node, cursor))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(group(name, Value::Struct, children))
            }
            TypeNode::Tuple { items } => {
                let label = self.item;
                let children = items
                    .iter()
                    .map(|item| self.read(label, item, cursor))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(group(name, Value::Struct, children))
            }
            TypeNode::Enum { size, variants } => self.enumeration(name, size, variants, cursor),
            TypeNode::DefinedTypeLink { name: link } => {
                let target = self.linked(link)?;
                self.read(name, target, cursor)
            }
        }
    }

    fn number(
        &mut self,
        name: &str,
        format: NumberFormat,
        display: Option<&NumberDisplay>,
        cursor: &mut Cursor<'_>,
    ) -> Result<Node, Reject> {
        let (number, span) = self.take_number(format, cursor)?;
        let value = if format.float() {
            // 浮点数按位保存,NaN 的载荷也能原样重新编码.
            let bits = number as u64;
            Value::Text(f64::from_bits(bits).to_string())
        } else {
            self.number_value(format, number, display)
        };
        Ok(own(name, value, span))
    }

    fn public_key(&mut self, name: &str, cursor: &mut Cursor<'_>) -> Result<Node, Reject> {
        let field = cursor.take(PUBLIC_KEY_BYTES)?;
        self.rebuilt.extend_from_slice(field.bytes);
        Ok(own(name, Value::Address(base58::encode(field.bytes)), field.span))
    }

    fn boolean(&mut self, name: &str, size: &TypeNode, cursor: &mut Cursor<'_>) -> Result<Node, Reject> {
        let (number, span) = self.take_count(size, cursor)?;
        match number {
            0 => Ok(own(name, Value::Bool(false), span)),
            1 => Ok(own(name, Value::Bool(true), span)),
            2.. => Err(Reject::Malformed),
        }
    }

    fn size_prefixed(
        &mut self,
        name: &str,
        prefix: &TypeNode,
        node: &TypeNode,
        cursor: &mut Cursor<'_>,
    ) -> Result<Node, Reject> {
        let format = self.number_format(prefix)?;
        let start = self.rebuilt.len();
        let (size, span) = self.take_count(prefix, cursor)?;
        let body = cursor.take(size)?;
        self.rebuilt.truncate(start);
        let walked = self.bounded(name, node, &body)?;
        let written = (self.rebuilt.len() - start) as i128;
        self.rebuilt.splice(start..start, format.encode(written));
        let mut framing = walked.framing.clone();
        framing.push(span);
        Ok(walked.framed(framing))
    }

    /// 这段字节必须正好读完.
    fn bounded(&mut self, name: &str, node: &TypeNode, field: &Field<'_>) -> Result<Node, Reject> {
        let mut inner = Cursor::new(field);
        let walked = self.read(name, node, &mut inner)?;
        if inner.is_empty() {
            Ok(walked)
        } else {
            Err(Reject::Malformed)
        }
    }

    fn absent(&self, name: &str, span: Span) -> Node {
        own(name, Value::Enum(self.none.to_string()), span)
    }

    fn option(
        &mut self,
        name: &str,
        prefix: &TypeNode,
        item: &TypeNode,
        cursor: &mut Cursor<'_>,
    ) -> Result<Node, Reject> {
        let (tag, span) = self.take_count(prefix, cursor)?;
        match tag {
            0 => Ok(self.absent(name, span)),
            1 => {
                let walked = self.read(name, item, cursor)?;
                let mut framing = walked.framing.clone();
                framing.push(span);
                Ok(walked.framed(framing))
            }
            2.. => Err(Reject::Malformed),
        }
    }

    fn zeroable(&mut self, name: &str, item: &TypeNode, cursor: &mut Cursor<'_>) -> Result<Node, Reject> {
        let field = cursor.take(self.fixed_size(item)?)?;
        if field.bytes.iter().all(|byte| *byte == 0) {
            self.rebuilt.extend_from_slice(field.bytes);
            return Ok(self.absent(name, field.span));
        }
        self.bounded(name, item, &field)
    }

    fn array(
        &mut self,
        name: &str,
        item: &TypeNode,
        count: &CountNode,
        cursor: &mut Cursor<'_>,
    ) -> Result<Node, Reject> {
        let (total, framing) = match count {
            CountNode::Fixed { value } => (Some(*value), Vec::new()),
            CountNode::Prefixed { prefix } => {
                let (value, span) = self.take_count(prefix, cursor)?;
                (Some(value), vec![span])
            }
            CountNode::Remainder => (None, Vec::new()),
        };
        let mut children = Vec::new();
        let label = self.item;
        while total.map_or(!cursor.is_empty(), |total| (children.len() as u64) < total) {
            let before = cursor.remaining().len();
            children.push(self.read(label, item, cursor)?);
            // 宽度为 0 的元素不消耗字节,计数不能决定循环次数.
            if cursor.remaining().len() == before {
                return Err(Reject::Malformed);
            }
        }
        Ok(group(name, Value::List, children).framed(framing))
    }

    fn enumeration(
        &mut self,
        name: &str,
        size: &TypeNode,
        variants: &[Variant],
        cursor: &mut Cursor<'_>,
    ) -> Result<Node, Reject> {
        let (tag, span) = self.take_count(size, cursor)?;
        let (_, found) = variants
            .iter()
            .enumerate()
            .find(|(index, variant)| variant.tag(*index) == tag)
            .ok_or(Reject::Malformed)?;
        let tagged = |variant: &str, label: &str| own(variant, Value::Enum(label.to_string()), span);
        match found {
            Variant::Empty {
                name: label,
                discriminator: _discriminator,
            } => Ok(tagged(name, label)),
            Variant::Struct {
                name: label,
                discriminator: _discriminator,
                body,
            }
            | Variant::Tuple {
                name: label,
                discriminator: _discriminator,
                tuple: body,
            } => {
                let variant = tagged(self.variant, label);
                let payload = self.read(name, body, cursor)?;
                let children = std::iter::once(variant).chain(payload.children).collect();
                Ok(group(name, Value::Struct, children).framed(payload.framing))
            }
        }
    }
}

const PUBLIC_KEY_BYTES: u64 = 32;

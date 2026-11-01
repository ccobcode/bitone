//! TRON 解码.字段的名字,编号和类型在 assets/tron-schema.json,由 `tools/protogen` 从官方 proto 生成,
//! 这里只认 protobuf 线格式.见 docs/10-tx-decoding.md 第 2.3 节和第 5.2 节.
//!
//! 第 1.1 节:冷钱包可能多年不更新,所以未知的合约类型和未知字段都是 `opaque` 节点,可以签名.
//! 签名覆盖的是这些字节本身,所以乱序,重复,非最短编码这些不能重新编码回来的写法都拒绝.

use super::evm::{Coin, whole};
use super::generated::protocol::transaction::Raw;
use super::generated::protocol::transaction::contract::ContractType;
use super::generated::recode::recode;
use super::protobuf::{Body, Wire, next, put_bytes, put_varint};
use super::psbt::Cursor;
use super::psbt_values::Field;
use super::{Amount, Decoder, Extent, Input, Node, Reject, RoundTrip, Source, Value, Walked};
use alloy_primitives::{B256, keccak256};
use prost::Message;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const CONTRACT_ROW: &str = "contract";
const TYPE_ROW: &str = "type";
const PARAMETER_ROW: &str = "parameter";
const VALUE_ROW: &str = "value";
const DATA_ROW: &str = "data";
const PERMISSION_ROW: &str = "Permission_id";
const OPERATION_ROW: &str = "operation";
const ROOT_ROW: &str = "raw_data";
const UNKNOWN_FIELD_PREFIX: &str = "field_";
const CONTRACT_UNKNOWN: &str = "contract-unknown";
const ENUM_UNKNOWN: &str = "enum-unknown";
const NEGATIVE: &str = "negative-integer";
const UNKNOWN_FIELD: &str = "unknown-field";
/// 地址的网络前缀和总长度.
const ADDRESS_PREFIX: u8 = 0x41;
const ADDRESS_LEN: usize = 21;
/// tronweb 的 `TRON_MESSAGE_PREFIX`.
const MESSAGE_PREFIX: &str = "\x19TRON Signed Message:\n";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FieldKind {
    Int32,
    Int64,
    Bool,
    Bytes,
    Text,
    Enum,
    Message,
}

/// 同一种线格式的不同含义,来自 `schemas/tron-fields.json`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Show {
    Coin,
    Token,
    Time,
    Text,
    Hash,
    Bytes,
    Address,
    Operations,
    Contract,
    ContractType,
    TypeUrl,
    ContractValue,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    number: u32,
    name: String,
    kind: FieldKind,
    of: Option<String>,
    #[serde(default)]
    repeated: bool,
    #[serde(default)]
    map: bool,
    #[serde(default)]
    required: bool,
    /// `repeated` 字段,节点只接受一项.
    #[serde(default)]
    single: bool,
    show: Option<Show>,
    action: Option<String>,
    len: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractSpec {
    name: String,
    message: Option<String>,
}

/// `Transaction` 的两个字段的编号.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    raw_data: u32,
    signature: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    root: String,
    envelope: Envelope,
    type_url_prefix: String,
    owner_fields: Vec<String>,
    contracts: BTreeMap<i32, ContractSpec>,
    enums: BTreeMap<String, BTreeMap<i32, String>>,
    messages: BTreeMap<String, Vec<Spec>>,
}

static SCHEMA: OnceLock<Schema> = OnceLock::new();

fn schema() -> &'static Schema {
    SCHEMA
        .get_or_init(|| serde_json::from_str(include_str!("../../assets/tron-schema.json")).expect("内置 TRON 表有效"))
}

/// 规则表里要有的全部种类名.
pub fn tron_actions() -> Vec<&'static str> {
    let schema = schema();
    let contracts = schema.contracts.values().map(|contract| contract.name.as_str());
    let fields = schema
        .messages
        .values()
        .flatten()
        .filter_map(|spec| spec.action.as_deref());
    let mut actions: Vec<&str> = contracts
        .chain(fields)
        .chain([CONTRACT_UNKNOWN, ENUM_UNKNOWN, NEGATIVE, UNKNOWN_FIELD])
        .collect();
    actions.sort_unstable();
    actions.dedup();
    actions
}

/// 全部字段名,显示文字的数据文件要覆盖它们.
pub fn tron_field_names() -> Vec<&'static str> {
    let names = schema().messages.values().flatten().map(|spec| spec.name.as_str());
    let mut names: Vec<&str> = names.chain([ROOT_ROW, OPERATION_ROW]).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// 官方枚举的全部合约类型名.
pub fn tron_contracts() -> Vec<(i32, &'static str)> {
    let contracts = schema().contracts.iter();
    contracts
        .map(|(number, contract)| (*number, contract.name.as_str()))
        .collect()
}

fn base58_address(bytes: &[u8]) -> Result<String, Reject> {
    match bytes.first() {
        Some(&ADDRESS_PREFIX) if bytes.len() == ADDRESS_LEN => Ok(bitcoin::base58::encode_check(bytes)),
        Some(_) | None => Err(Reject::Malformed),
    }
}

fn utf8_or_bytes(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(|_| Value::Bytes(bytes.to_vec()), |text| Value::Text(text.to_string()))
}

fn trimmed(value: i64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    bytes.get(start..).unwrap_or_default().to_vec()
}

fn leaf(spec: &Spec, wire: &Wire<'_>, value: Value) -> Node {
    Node::new(spec.name.as_str(), value, Source::Signed(Extent::Own(wire.value.span))).framed(wire.framing.clone())
}

fn flagged(spec: &Spec, node: Node) -> Node {
    match &spec.action {
        Some(action) => node.action(action.as_str()),
        None => node,
    }
}

fn varint(wire: &Wire<'_>) -> Result<u64, Reject> {
    match wire.body {
        Body::Varint(value) => Ok(value),
        Body::Fixed | Body::Bytes | Body::Group => Err(Reject::Malformed),
    }
}

/// 声明是字节串的字段,线格式也必须是长度前缀.
fn delimited<'a>(wire: &Wire<'a>) -> Result<Field<'a>, Reject> {
    match wire.body {
        Body::Bytes => Ok(wire.value),
        Body::Varint(_) | Body::Fixed | Body::Group => Err(Reject::Malformed),
    }
}

/// 本机不认识的字段.以后的 java-tron 可能加字段,第 1.1 节要求这样的交易也能签,
/// 字段的字节原样显示.标签和长度是最短编码,所以这段重新编码后还是它自己.
fn unknown_field(wire: &Wire<'_>) -> Node {
    let name = [UNKNOWN_FIELD_PREFIX, &wire.number.to_string()].concat();
    let value = Value::Opaque(wire.value.bytes.to_vec());
    Node::new(name, value, Source::Signed(Extent::Own(wire.value.span)))
        .framed(wire.framing.clone())
        .action(UNKNOWN_FIELD)
}

/// `operations` 每一位对应一种合约类型,这些节点继承这个字段的字节.
fn operations(bytes: &[u8]) -> Vec<Node> {
    let bits = bytes.iter().enumerate().flat_map(|(index, byte)| {
        (0..8)
            .filter(move |bit| byte >> bit & 1 == 1)
            .map(move |bit| index as i32 * 8 + bit)
    });
    bits.map(|number| {
        let name = schema().contracts.get(&number).map(|contract| contract.name.clone());
        let name = name.unwrap_or_else(|| number.to_string());
        Node::new(OPERATION_ROW, Value::Enum(name), Source::Signed(Extent::Inherited))
    })
    .collect()
}

/// 一条消息的结果.`known` 是去掉未知字段和 map 之后的字节,交给官方类型比较.
#[derive(Default)]
struct Fields {
    nodes: Vec<Node>,
    known: Vec<u8>,
}

impl Fields {
    fn push_framed(&mut self, wire: &Wire<'_>, body: &[u8]) {
        self.known.extend_from_slice(wire.tag);
        put_varint(&mut self.known, body.len() as u64);
        self.known.extend_from_slice(body);
    }

    fn add(&mut self, spec: &Spec, node: Node, again: bool) -> Result<(), Reject> {
        if !spec.repeated {
            self.nodes.push(node);
            return Ok(());
        }
        if !again {
            let list = Node::new(spec.name.as_str(), Value::List, Source::Signed(Extent::Container));
            self.nodes.push(list);
        }
        let list = self.nodes.last_mut().ok_or(Reject::Malformed)?;
        // Java 的 map 后一个覆盖前一个,重新序列化之后这段字节就没有了.
        if spec.map && list.children.iter().any(|entry| same_key(entry, &node)) {
            return Err(Reject::Malformed);
        }
        list.children.push(node);
        Ok(())
    }
}

fn same_key(left: &Node, right: &Node) -> bool {
    let key = |entry: &Node| entry.children.first().map(|key| key.value.clone());
    key(left) == key(right)
}

/// 一次遍历.`kind` 是合约类型的编号,字段 `type` 写 0 时不上线.
struct Walk {
    coin: Option<Coin>,
    kind: i32,
    checks: Vec<RoundTrip>,
}

impl Walk {
    fn compare(&mut self, original: Vec<u8>, rebuilt: Vec<u8>) {
        self.checks.push(RoundTrip { original, rebuilt });
    }

    fn message(&mut self, name: &str, body: &Field<'_>) -> Result<Fields, Reject> {
        let specs = schema().messages.get(name).ok_or(Reject::Malformed)?;
        let mut cursor = Cursor::new(body);
        let mut fields = Fields::default();
        let mut last = 0;
        while !cursor.is_empty() {
            let wire = next(&mut cursor)?;
            // 节点按编号重新序列化,乱序和分开写的重复字段都不是签名的那段字节.
            if wire.number < last {
                return Err(Reject::Malformed);
            }
            let again = wire.number == last;
            last = wire.number;
            match specs.iter().find(|spec| spec.number == wire.number) {
                Some(spec) if again && (!spec.repeated || spec.single) => return Err(Reject::Malformed),
                Some(spec) => {
                    let node = self.field(spec, &wire, &mut fields)?;
                    fields.add(spec, node, again)?;
                }
                None => fields.nodes.push(unknown_field(&wire)),
            }
        }
        let present = |spec: &Spec| fields.nodes.iter().any(|node| node.name == spec.name);
        if specs.iter().any(|spec| spec.required && !present(spec)) {
            return Err(Reject::Malformed);
        }
        Ok(fields)
    }

    fn field(&mut self, spec: &Spec, wire: &Wire<'_>, fields: &mut Fields) -> Result<Node, Reject> {
        match spec.kind {
            FieldKind::Message => self.nested(spec, wire, fields),
            FieldKind::Int32 | FieldKind::Int64 => {
                fields.known.extend_from_slice(wire.raw);
                self.integer(spec, wire)
            }
            FieldKind::Bool => {
                fields.known.extend_from_slice(wire.raw);
                Ok(flagged(spec, leaf(spec, wire, Value::Bool(varint(wire)? != 0))))
            }
            FieldKind::Enum => {
                fields.known.extend_from_slice(wire.raw);
                self.enumeration(spec, wire)
            }
            FieldKind::Text => {
                fields.known.extend_from_slice(wire.raw);
                self.text(spec, wire)
            }
            FieldKind::Bytes if spec.show == Some(Show::ContractValue) => self.contract_value(spec, wire, fields),
            FieldKind::Bytes => {
                fields.known.extend_from_slice(wire.raw);
                bytes_node(spec, wire)
            }
        }
    }

    fn nested(&mut self, spec: &Spec, wire: &Wire<'_>, fields: &mut Fields) -> Result<Node, Reject> {
        let of = spec.of.as_deref().ok_or(Reject::Malformed)?;
        let mut inner = self.message(of, &delimited(wire)?)?;
        // map 的条目在 Java 里总是写出键和值,而 prost 跳过默认值,这段字节由遍历器负责.
        if !spec.map {
            fields.push_framed(wire, &inner.known);
        }
        if spec.show == Some(Show::Contract) {
            self.add_default_type(of, &mut inner)?;
        }
        Ok(
            Node::new(spec.name.as_str(), Value::Struct, Source::Signed(Extent::Container))
                .framed(wire.framing.clone())
                .children(inner.nodes),
        )
    }

    /// 类型 0 不上线,这一行不能没有.
    fn add_default_type(&self, of: &str, inner: &mut Fields) -> Result<(), Reject> {
        let specs = schema().messages.get(of).ok_or(Reject::Malformed)?;
        let spec = specs
            .iter()
            .find(|spec| spec.show == Some(Show::ContractType))
            .ok_or(Reject::Malformed)?;
        if inner.nodes.iter().any(|node| node.name == spec.name) {
            return Ok(());
        }
        let contract = schema().contracts.get(&self.kind).ok_or(Reject::Malformed)?;
        let node = Node::new(spec.name.as_str(), Value::Enum(contract.name.clone()), Source::Derived);
        inner.nodes.insert(0, node.action(contract.name.as_str()));
        Ok(())
    }

    fn integer(&self, spec: &Spec, wire: &Wire<'_>) -> Result<Node, Reject> {
        let number = varint(wire)? as i64;
        let negative = number < 0;
        let value = if spec.show == Some(Show::Time) {
            Value::Time(number)
        } else if negative {
            Value::Int(i128::from(number))
        } else if spec.show == Some(Show::Coin) {
            Value::Amount(Amount {
                raw: trimmed(number),
                decimals: self.coin.map(|coin| coin.decimals),
                unit: self.coin.map(|coin| coin.unit.to_string()),
            })
        } else if spec.show == Some(Show::Token) {
            Value::Amount(Amount {
                raw: trimmed(number),
                decimals: None,
                unit: None,
            })
        } else {
            Value::Int(i128::from(number))
        };
        let node = leaf(spec, wire, value);
        // 第 5.2 节:协议里没有负数的含义,显示出来按无法识别处理.
        Ok(if negative {
            node.action(NEGATIVE)
        } else {
            flagged(spec, node)
        })
    }

    fn enumeration(&mut self, spec: &Spec, wire: &Wire<'_>) -> Result<Node, Reject> {
        let number = i32::try_from(varint(wire)? as i64).map_err(|_| Reject::Malformed)?;
        let table = spec.of.as_ref().and_then(|of| schema().enums.get(of));
        let name = table.and_then(|table| table.get(&number));
        let shown = name.map_or_else(|| number.to_string(), Clone::clone);
        let node = leaf(spec, wire, Value::Enum(shown));
        if spec.show == Some(Show::ContractType) {
            self.kind = number;
            return Ok(node.action(name.map_or(CONTRACT_UNKNOWN, String::as_str)));
        }
        Ok(if name.is_some() {
            flagged(spec, node)
        } else {
            node.action(ENUM_UNKNOWN)
        })
    }

    fn text(&self, spec: &Spec, wire: &Wire<'_>) -> Result<Node, Reject> {
        let text = std::str::from_utf8(delimited(wire)?.bytes).map_err(|_| Reject::Malformed)?;
        if spec.show == Some(Show::TypeUrl) {
            self.check_type_url(text)?;
        }
        Ok(flagged(spec, leaf(spec, wire, Value::Text(text.to_string()))))
    }

    /// `type_url` 和 `type` 说的是不同的合约时,这个请求自相矛盾.
    fn check_type_url(&self, text: &str) -> Result<(), Reject> {
        let schema = schema();
        let named =
            |contract: &ContractSpec| text.strip_prefix(schema.type_url_prefix.as_str()) == Some(&contract.name);
        match schema.contracts.get(&self.kind) {
            Some(contract) if !named(contract) => Err(Reject::Malformed),
            None if schema.contracts.values().any(named) => Err(Reject::Malformed),
            Some(_) | None => Ok(()),
        }
    }

    /// `Any.value`.合约类型本机认识时按它的消息解码,不认识时整段是 `opaque`.
    fn contract_value(&mut self, spec: &Spec, wire: &Wire<'_>, fields: &mut Fields) -> Result<Node, Reject> {
        let body = delimited(wire)?;
        let contract = schema().contracts.get(&self.kind);
        let message = contract.and_then(|contract| contract.message.as_deref());
        let (Some(message), Ok(kind)) = (message, ContractType::try_from(self.kind)) else {
            fields.known.extend_from_slice(wire.raw);
            return Ok(leaf(spec, wire, Value::Opaque(body.bytes.to_vec())));
        };
        let inner = self.message(message, &body)?;
        let rebuilt = recode(kind, &inner.known).ok_or(Reject::Malformed)?;
        let rebuilt = rebuilt.map_err(|_| Reject::Malformed)?;
        fields.push_framed(wire, &inner.known);
        self.compare(inner.known, rebuilt);
        Ok(
            Node::new(spec.name.as_str(), Value::Struct, Source::Signed(Extent::Container))
                .framed(wire.framing.clone())
                .children(inner.nodes),
        )
    }
}

fn bytes_node(spec: &Spec, wire: &Wire<'_>) -> Result<Node, Reject> {
    let bytes = delimited(wire)?.bytes;
    if spec.len.is_some_and(|len| len != bytes.len()) {
        return Err(Reject::Malformed);
    }
    let value = if spec.show == Some(Show::Address) {
        Value::Address(base58_address(bytes)?)
    } else if spec.show == Some(Show::Text) {
        utf8_or_bytes(bytes)
    } else if spec.show == Some(Show::Hash) {
        Value::Hash(bytes.to_vec())
    } else {
        Value::Bytes(bytes.to_vec())
    };
    let node = flagged(spec, leaf(spec, wire, value));
    Ok(if spec.show == Some(Show::Operations) {
        node.children(operations(bytes))
    } else {
        node
    })
}

/// `TriggerSmartContract.data` 这一行,结构和 `tron_owner` 取发起方相同.
fn call_data(root: &mut Node) -> Option<&mut Node> {
    let list = root.children.iter_mut().find(|node| node.name == CONTRACT_ROW)?;
    let contract = list.children.first_mut()?;
    let parameter = contract.children.iter_mut().find(|node| node.name == PARAMETER_ROW)?;
    let value = parameter.children.iter_mut().find(|node| node.name == VALUE_ROW)?;
    value.children.iter_mut().find(|node| node.name == DATA_ROW)
}

/// 解码 `Transaction.raw`.`coin` 是原生币,本机不认识的网络没有这一项,金额显示原始整数.
/// `call` 是 `tx` 层按描述文件解出的字段,来源都是 `descriptor`,解码器不读清单.
#[derive(Clone, Copy, Debug)]
pub struct TronWalker<'a> {
    pub coin: Option<Coin>,
    pub call: &'a [Node],
}

impl Decoder for TronWalker<'_> {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let mut walk = Walk {
            coin: self.coin,
            kind: 0,
            checks: Vec::new(),
        };
        let raw = walk.message(&schema().root, &whole(input.payload))?;
        let official = Raw::decode(raw.known.as_slice()).map_err(|_| Reject::Malformed)?;
        walk.compare(raw.known, official.encode_to_vec());
        let mut root = Node::new(ROOT_ROW, Value::Struct, Source::Signed(Extent::Container)).children(raw.nodes);
        if let Some(data) = call_data(&mut root) {
            data.children.extend_from_slice(self.call);
        }
        Ok(Some(Walked {
            root,
            round_trips: walk.checks,
        }))
    }
}

fn child<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    node.children.iter().find(|child| child.name == name)
}

/// 唯一的 `contract` 这一项.
fn contract_node(root: &Node) -> Option<&Node> {
    child(root, CONTRACT_ROW)?.children.first()
}

/// 发起方,和 java-tron 的 `getOwner` 一样按名字取.没有这个字段的合约是 `None`.
pub fn tron_owner(root: &Node) -> Option<&str> {
    let value = child(child(contract_node(root)?, PARAMETER_ROW)?, VALUE_ROW)?;
    let names = schema().owner_fields.iter();
    let owner = names.filter_map(|name| child(value, name)).next()?;
    if let Value::Address(text) = &owner.value {
        Some(text)
    } else {
        None
    }
}

/// 这笔交易的合约:规则表里的种类名和合约的字段.未知的合约类型没有字段.
pub fn tron_contract(root: &Node) -> Option<(&str, &Node)> {
    let contract = contract_node(root)?;
    let kind = child(contract, TYPE_ROW)?.action.as_deref()?;
    Some((kind, child(child(contract, PARAMETER_ROW)?, VALUE_ROW)?))
}

/// `Permission_id`.0 不上线,就是账户的 owner 权限.
pub fn tron_permission_id(root: &Node) -> i128 {
    let node = contract_node(root).and_then(|contract| child(contract, PERMISSION_ROW));
    if let Some(Value::Int(number)) = node.map(|node| &node.value) {
        *number
    } else {
        0
    }
}

/// 这种合约的 `type_url`.
pub fn tron_type_url(name: &str) -> String {
    [schema().type_url_prefix.as_str(), name].concat()
}

/// TIP-191 的摘要,和 tronweb 的 `utils.message.hashMessage` 相同.前缀不同于 EIP-191,其余一样.
pub fn tron_message_hash(message: &[u8]) -> B256 {
    let mut preimage = MESSAGE_PREFIX.as_bytes().to_vec();
    preimage.extend_from_slice(message.len().to_string().as_bytes());
    preimage.extend_from_slice(message);
    keccak256(preimage)
}

/// TIP-191,载荷是消息本身,前缀由签名方加上.
pub struct TronPersonalWalker;

impl Decoder for TronPersonalWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let span = whole(input.payload).span;
        let message = Node::new(
            "message",
            utf8_or_bytes(input.payload),
            Source::Signed(Extent::Own(span)),
        );
        let digest = Node::new(
            "signing_hash",
            Value::Hash(tron_message_hash(input.payload).to_vec()),
            Source::Derived,
        );
        let root = Node::new("personal_message", Value::Struct, Source::Signed(Extent::Container))
            .children(vec![message, digest])
            .action("personal-sign");
        Ok(Some(Walked {
            root,
            round_trips: vec![RoundTrip {
                original: input.payload.to_vec(),
                rebuilt: input.payload.to_vec(),
            }],
        }))
    }
}

/// 可以广播的 `Transaction`.`raw_data` 原样写入,未知字段也在里面.
pub fn tron_signed(payload: &[u8], signature: &[u8]) -> Vec<u8> {
    let envelope = &schema().envelope;
    let mut out = Vec::new();
    put_bytes(&mut out, envelope.raw_data, payload);
    put_bytes(&mut out, envelope.signature, signature);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::generated::google::protobuf::Any;
    use crate::decode::generated::protocol::transaction::Contract;
    use crate::decode::generated::protocol::{TransferContract, TriggerSmartContract};
    use crate::decode::{Decoded, Failure, Kind, decode, display};
    use crate::{Chain, Mode};

    const COIN: Coin = Coin {
        unit: "TRX",
        decimals: 6,
    };
    const OWNER: [u8; 21] = [0x41; 21];
    const PEER: [u8; 21] = {
        let mut bytes = [0x22; 21];
        bytes[0] = 0x41;
        bytes
    };
    const NOW: i64 = 1_800_000_000_000;

    fn run(payload: &[u8]) -> Result<Decoded, Failure> {
        decode(
            &TronWalker {
                coin: Some(COIN),
                call: &[],
            },
            &Input {
                chain: Chain::Tron,
                mode: Some(Mode::Mainnet),
                network: "tron",
                kind: 0,
                paths: &[],
                payload,
                signer: None,
            },
        )
    }

    fn field(number: u32, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, number, value);
        out
    }

    fn number_field(number: u32, value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, u64::from(number << 3));
        put_varint(&mut out, value);
        out
    }

    fn raw(kind: i32, name: &str, value: Vec<u8>) -> Raw {
        Raw {
            ref_block_bytes: vec![0xab, 0xcd],
            ref_block_hash: vec![1, 2, 3, 4, 5, 6, 7, 8],
            expiration: NOW + 1_800_000,
            contract: vec![Contract {
                r#type: kind,
                parameter: Some(Any {
                    type_url: tron_type_url(name),
                    value,
                }),
                ..Contract::default()
            }],
            timestamp: NOW,
            ..Raw::default()
        }
    }

    fn transfer_value() -> Vec<u8> {
        TransferContract {
            owner_address: OWNER.to_vec(),
            to_address: PEER.to_vec(),
            amount: 5,
        }
        .encode_to_vec()
    }

    fn transfer() -> Raw {
        raw(1, "TransferContract", transfer_value())
    }

    fn with(tx: Raw, change: impl FnOnce(&mut Raw)) -> Vec<u8> {
        let mut tx = tx;
        change(&mut tx);
        tx.encode_to_vec()
    }

    fn shown(decoded: &Decoded, name: &str) -> Vec<String> {
        let visits = decoded.visits();
        let rows = visits.iter().filter(|visit| visit.node.name == name);
        rows.map(|visit| display(&visit.node.value)).collect()
    }

    fn has(decoded: &Decoded, kind: &str) -> bool {
        decoded.actions().iter().any(|(_, _, found)| *found == kind)
    }

    fn base58(bytes: &[u8]) -> String {
        bitcoin::base58::encode_check(bytes)
    }

    #[test]
    fn a_transfer_decodes_field_by_field() {
        let decoded = run(&transfer().encode_to_vec()).unwrap();
        assert_eq!(decoded.root().name, ROOT_ROW);
        assert_eq!(shown(&decoded, "ref_block_bytes"), ["abcd"]);
        assert_eq!(shown(&decoded, "ref_block_hash"), ["0102030405060708"]);
        assert_eq!(shown(&decoded, "expiration"), ["2027-01-15T08:30:00Z"]);
        assert_eq!(shown(&decoded, "timestamp"), ["2027-01-15T08:00:00Z"]);
        assert_eq!(shown(&decoded, "type"), ["TransferContract"]);
        assert_eq!(shown(&decoded, "type_url"), [tron_type_url("TransferContract")]);
        assert_eq!(shown(&decoded, "owner_address"), [base58(&OWNER)]);
        assert_eq!(shown(&decoded, "to_address"), [base58(&PEER)]);
        assert_eq!(shown(&decoded, "amount"), ["0.000005 TRX"]);
        assert!(has(&decoded, "TransferContract"));
        assert_eq!(tron_owner(decoded.root()), Some(base58(&OWNER).as_str()));
        assert_eq!(tron_permission_id(decoded.root()), 0);
    }

    #[test]
    fn an_unknown_network_shows_raw_integers() {
        let input = Input {
            chain: Chain::Tron,
            mode: None,
            network: "local",
            kind: 0,
            paths: &[],
            payload: &transfer().encode_to_vec(),
            signer: None,
        };
        let decoded = decode(&TronWalker { coin: None, call: &[] }, &input).unwrap();
        assert_eq!(shown(&decoded, "amount"), ["5"]);
    }

    #[test]
    fn type_zero_is_not_on_the_wire_but_is_in_the_tree() {
        let value = crate::decode::generated::protocol::AccountCreateContract {
            owner_address: OWNER.to_vec(),
            account_address: PEER.to_vec(),
            r#type: 0,
        };
        let decoded = run(&raw(0, "AccountCreateContract", value.encode_to_vec()).encode_to_vec()).unwrap();
        assert_eq!(shown(&decoded, "type"), ["AccountCreateContract"]);
        assert!(has(&decoded, "AccountCreateContract"));
    }

    #[test]
    fn an_unknown_contract_type_keeps_its_outer_fields() {
        let tx = with(raw(99, "FutureContract", vec![1, 2, 3]), |tx| tx.fee_limit = 7);
        let decoded = run(&tx).unwrap();
        assert!(has(&decoded, CONTRACT_UNKNOWN));
        assert_eq!(shown(&decoded, "type"), ["99"]);
        assert_eq!(shown(&decoded, "fee_limit"), ["0.000007 TRX"]);
        assert_eq!(shown(&decoded, "value"), ["010203"]);
        assert_eq!(tron_owner(decoded.root()), None);
        let visits = decoded.visits();
        let value = visits.iter().find(|visit| visit.node.name == "value").unwrap();
        assert_eq!(value.node.value.kind(), Kind::Opaque);
    }

    #[test]
    fn contract_types_without_a_message_are_opaque() {
        for (kind, name) in [(20, "CustomContract"), (32, "GetContract")] {
            let decoded = run(&raw(kind, name, vec![1]).encode_to_vec()).unwrap();
            assert!(has(&decoded, name), "{name}");
            assert!(!has(&decoded, CONTRACT_UNKNOWN), "{name}");
        }
    }

    #[test]
    fn unknown_fields_are_opaque_at_every_level() {
        let top = [transfer().encode_to_vec(), number_field(99, 1)].concat();
        assert_eq!(shown(&run(&top).unwrap(), "field_99"), ["01"]);

        let inner = [transfer_value(), field(9, &[7, 8])].concat();
        let decoded = run(&raw(1, "TransferContract", inner).encode_to_vec()).unwrap();
        assert_eq!(shown(&decoded, "field_9"), ["0708"]);
        assert!(has(&decoded, UNKNOWN_FIELD));

        let contract = transfer().contract[0].encode_to_vec();
        let unknown = [contract, number_field(6, 1)].concat();
        let base = with(transfer(), |tx| tx.contract.clear());
        let tx = reorder(&[base, field(11, &unknown)].concat());
        assert_eq!(shown(&run(&tx).unwrap(), "field_6"), ["01"]);
    }

    /// 把字段按编号排好.
    fn reorder(bytes: &[u8]) -> Vec<u8> {
        let span = whole(bytes);
        let mut cursor = Cursor::new(&span);
        let mut wires = Vec::new();
        while !cursor.is_empty() {
            let wire = next(&mut cursor).unwrap();
            wires.push((wire.number, wire.raw.to_vec()));
        }
        wires.sort_by_key(|(number, _)| *number);
        wires.into_iter().flat_map(|(_, raw)| raw).collect()
    }

    #[test]
    fn unknown_fields_of_every_wire_type_are_kept() {
        let tails: [(&[u8], &str); 4] = [
            (&[0xc8, 0x06, 0x05], "05"),
            (&[0xc9, 0x06, 1, 2, 3, 4, 5, 6, 7, 8], "0102030405060708"),
            (&[0xcd, 0x06, 1, 2, 3, 4], "01020304"),
            (&[0xcb, 0x06, 0x08, 0x01, 0xcc, 0x06], "0801"),
        ];
        for (tail, value) in tails {
            let tx = [transfer().encode_to_vec(), tail.to_vec()].concat();
            assert_eq!(shown(&run(&tx).unwrap(), "field_105"), [value]);
        }
    }

    #[test]
    fn an_empty_unknown_field_is_still_flagged() {
        let tx = [transfer().encode_to_vec(), field(99, &[])].concat();
        assert!(has(&run(&tx).unwrap(), UNKNOWN_FIELD));
    }

    #[test]
    fn an_unknown_field_must_keep_the_field_order() {
        let mut tx = transfer().encode_to_vec();
        tx.splice(0..0, number_field(99, 1));
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn duplicates_are_refused() {
        let tx = [field(1, &[0xab, 0xcd]), transfer().encode_to_vec()].concat();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
        let inner = [transfer_value(), number_field(3, 9)].concat();
        let tx = raw(1, "TransferContract", inner).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn field_order_and_explicit_defaults_are_refused() {
        let tx = transfer();
        let contract = tx.contract[0].encode_to_vec();
        let swapped = [
            field(1, &tx.ref_block_bytes),
            field(4, &tx.ref_block_hash),
            number_field(8, tx.expiration as u64),
            number_field(14, tx.timestamp as u64),
            field(11, &contract),
        ]
        .concat();
        assert_eq!(run(&swapped).unwrap_err().reason, Reject::Malformed);
        let explicit = [tx.encode_to_vec(), number_field(18, 0)].concat();
        assert_eq!(run(&explicit).unwrap_err().reason, Reject::RoundTrip);
    }

    #[test]
    fn non_minimal_encodings_are_refused() {
        let inner = [transfer_value(), vec![0x98, 0x00, 0x05]].concat();
        let tx = raw(1, "TransferContract", inner).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn exactly_one_contract() {
        let mut two = transfer();
        two.contract.push(two.contract[0].clone());
        assert_eq!(run(&two.encode_to_vec()).unwrap_err().reason, Reject::Malformed);
        let none = with(transfer(), |tx| tx.contract.clear());
        assert_eq!(run(&none).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn type_url_must_match_the_contract_type() {
        let wrong = raw(1, "TriggerSmartContract", transfer_value()).encode_to_vec();
        assert_eq!(run(&wrong).unwrap_err().reason, Reject::Malformed);
        // 未知编号拿已知合约的名字也是矛盾.
        let claimed = raw(99, "TransferContract", transfer_value()).encode_to_vec();
        assert_eq!(run(&claimed).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn a_known_contract_needs_its_owner() {
        let value = TransferContract {
            to_address: PEER.to_vec(),
            amount: 1,
            ..TransferContract::default()
        };
        let tx = raw(1, "TransferContract", value.encode_to_vec()).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn addresses_need_the_network_prefix_and_length() {
        let bad = TransferContract {
            owner_address: OWNER.to_vec(),
            to_address: vec![0xa0; 21],
            amount: 1,
        };
        let tx = raw(1, "TransferContract", bad.encode_to_vec()).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
        let short = TransferContract {
            to_address: vec![0x41, 1, 2],
            ..bad
        };
        let tx = raw(1, "TransferContract", short.encode_to_vec()).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn ref_block_lengths_are_fixed() {
        let tx = with(transfer(), |tx| tx.ref_block_bytes = vec![1, 2, 3]);
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
        let tx = with(transfer(), |tx| tx.ref_block_hash = vec![1]);
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn negative_integers_are_shown_and_flagged() {
        let value = TransferContract {
            owner_address: OWNER.to_vec(),
            to_address: PEER.to_vec(),
            amount: -1,
        };
        let decoded = run(&raw(1, "TransferContract", value.encode_to_vec()).encode_to_vec()).unwrap();
        assert_eq!(shown(&decoded, "amount"), ["-1"]);
        assert!(has(&decoded, NEGATIVE));
    }

    #[test]
    fn a_wrong_wire_type_is_refused() {
        let inner = [
            field(1, &OWNER),
            field(2, &PEER),
            field(3, &[1]), // `amount` 是 varint.
        ]
        .concat();
        let tx = raw(1, "TransferContract", inner).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn map_entries_keep_their_wire_order_and_zero_values() {
        // Java 总是写出键和值,tronweb 按字符串排序,这两种写法节点重新序列化后都不变.
        let entry = |key: u64, value: u64| {
            let mut bytes = vec![0x08];
            put_varint(&mut bytes, key);
            bytes.push(0x10);
            put_varint(&mut bytes, value);
            field(2, &bytes)
        };
        let value = [field(1, &OWNER), entry(10, 0), entry(9, 5)].concat();
        let decoded = run(&raw(16, "ProposalCreateContract", value).encode_to_vec()).unwrap();
        assert_eq!(shown(&decoded, "key"), ["10", "9"]);
        assert_eq!(shown(&decoded, "value").len(), 3);
        let twice = [field(1, &OWNER), entry(9, 1), entry(9, 2)].concat();
        let tx = raw(16, "ProposalCreateContract", twice).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
        let bare = [field(1, &OWNER), field(2, &[0x08, 0x09])].concat();
        let tx = raw(16, "ProposalCreateContract", bare).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn trigger_value_fields_are_decoded_and_flagged() {
        let value = TriggerSmartContract {
            owner_address: OWNER.to_vec(),
            contract_address: PEER.to_vec(),
            call_value: 2_000_000,
            data: vec![1, 2],
            call_token_value: 7,
            token_id: 1_000_001,
        };
        let decoded = run(&raw(31, "TriggerSmartContract", value.encode_to_vec()).encode_to_vec()).unwrap();
        assert_eq!(shown(&decoded, "call_value"), ["2 TRX"]);
        assert_eq!(shown(&decoded, "call_token_value"), ["7"]);
        assert_eq!(shown(&decoded, "token_id"), ["1000001"]);
        assert_eq!(shown(&decoded, "data"), ["0102"]);
        assert!(has(&decoded, "call-value") && has(&decoded, "call-token-value"));
    }

    #[test]
    fn memo_is_text_when_it_can_be() {
        let decoded = run(&with(transfer(), |tx| tx.data = "备注".as_bytes().to_vec())).unwrap();
        assert_eq!(shown(&decoded, "data"), ["备注"]);
        assert!(has(&decoded, "memo"));
        let decoded = run(&with(transfer(), |tx| tx.data = vec![0xff, 0xfe])).unwrap();
        assert_eq!(shown(&decoded, "data"), ["fffe"]);
    }

    #[test]
    fn permission_id_is_read_from_the_contract() {
        let tx = with(transfer(), |tx| tx.contract[0].permission_id = 2);
        let decoded = run(&tx).unwrap();
        assert_eq!(tron_permission_id(decoded.root()), 2);
        assert!(has(&decoded, "permission-id"));
    }

    #[test]
    fn unused_fields_are_flagged() {
        for tx in [
            with(transfer(), |tx| tx.ref_block_num = 1),
            with(transfer(), |tx| tx.scripts = vec![1]),
            with(transfer(), |tx| tx.contract[0].provider = vec![1]),
            with(transfer(), |tx| tx.contract[0].contract_name = vec![1]),
        ] {
            assert!(has(&run(&tx).unwrap(), "field-unused"));
        }
    }

    #[test]
    fn operations_name_the_contract_types() {
        let mut bits = vec![0u8; 32];
        bits[0] = 0b10; // TransferContract.
        bits[3] = 0b1000_0000; // TriggerSmartContract.
        bits[12] = 1; // 没有这个编号.
        let permission = crate::decode::generated::protocol::Permission {
            r#type: 2,
            threshold: 1,
            operations: bits,
            keys: vec![crate::decode::generated::protocol::Key {
                address: PEER.to_vec(),
                weight: 1,
            }],
            ..Default::default()
        };
        let value = crate::decode::generated::protocol::AccountPermissionUpdateContract {
            owner_address: OWNER.to_vec(),
            owner: Some(permission.clone()),
            witness: None,
            actives: vec![permission],
        };
        let tx = raw(46, "AccountPermissionUpdateContract", value.encode_to_vec()).encode_to_vec();
        let decoded = run(&tx).unwrap();
        let expected = ["TransferContract", "TriggerSmartContract", "96"];
        assert_eq!(shown(&decoded, OPERATION_ROW), [expected, expected].concat());
    }

    #[test]
    fn operations_are_32_bytes() {
        let permission = crate::decode::generated::protocol::Permission {
            threshold: 1,
            operations: vec![1; 31],
            keys: vec![crate::decode::generated::protocol::Key {
                address: PEER.to_vec(),
                weight: 1,
            }],
            ..Default::default()
        };
        let value = crate::decode::generated::protocol::AccountPermissionUpdateContract {
            owner_address: OWNER.to_vec(),
            owner: Some(permission),
            ..Default::default()
        };
        let tx = raw(46, "AccountPermissionUpdateContract", value.encode_to_vec()).encode_to_vec();
        assert_eq!(run(&tx).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn labels_cover_every_field() {
        let labels: BTreeMap<String, String> =
            serde_json::from_str(include_str!("../../assets/field-labels.json")).unwrap();
        for name in tron_field_names() {
            let key = [Chain::Tron.id(), ".", name].concat();
            assert!(labels.contains_key(&key) || labels.contains_key(name), "{name}");
        }
    }

    #[test]
    fn the_schema_has_the_whole_enum() {
        assert_eq!(tron_contracts().len(), 41);
        let without_message = schema()
            .contracts
            .values()
            .filter(|contract| contract.message.is_none());
        let names: Vec<&str> = without_message.map(|contract| contract.name.as_str()).collect();
        assert_eq!(names, ["CustomContract", "GetContract"]);
    }

    #[test]
    fn every_contract_with_an_owner_requires_it() {
        for contract in schema().contracts.values() {
            let Some(message) = &contract.message else {
                continue;
            };
            let specs = &schema().messages[message];
            if let Some(owner) = specs.iter().find(|spec| spec.name == schema().owner_fields[0]) {
                assert!(owner.required, "{}", contract.name);
                assert_eq!(owner.show, Some(Show::Address), "{}", contract.name);
            } else {
                assert_eq!(contract.name, "ShieldedTransferContract");
            }
        }
    }
}

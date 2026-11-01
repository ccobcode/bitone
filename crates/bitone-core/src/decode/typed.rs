//! EIP-712 和 TIP-712 的 JSON.结构来自请求里的 `types`,原子类型的含义来自 `alloy-dyn-abi`.
//!
//! JSON 不进签名原文,C1 是每个成员都被类型消费,重复键和多余成员都失败.
//! C2 是 alloy 的 `Resolver` 重新计算 `hashStruct`.树和摘要都来自 `TypedMessage` 里同一份值.

use super::abi::{AddressText, abi_node};
use super::evm::{absent_text, whole};
use super::{Decoder, Extent, Input, Node, Reject, RoundTrip, Source, Value, Walked};
use crate::hex::FromHex;
use alloy_dyn_abi::{DynSolType, DynSolValue, PropertyDef, Resolver, TypeDef};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value as Json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::OnceLock;

const TYPES: &str = "types";
const PRIMARY_TYPE: &str = "primaryType";
const DOMAIN: &str = "domain";
const MESSAGE: &str = "message";
const FIELD_NAME: &str = "name";
const FIELD_TYPE: &str = "type";
const DOMAIN_TYPE: &str = "EIP712Domain";
const CHAIN_ID: &str = "chainId";
const VERIFYING_CONTRACT: &str = "verifyingContract";
/// `types` 没写 `EIP712Domain` 时,viem 和 alloy 都按这个顺序取出现的成员.
const DOMAIN_FIELDS: [(&str, &str); 5] = [
    ("name", "string"),
    ("version", "string"),
    (CHAIN_ID, "uint256"),
    (VERIFYING_CONTRACT, "address"),
    ("salt", "bytes32"),
];
/// TIP-712:`trcToken` 是原子类型,按 `uint256` 编码,类型串里保留原文.
const TRC_TOKEN: &str = "trcToken";
const UINT256: &str = "uint256";
const TRON_ADDRESS_PREFIX: u8 = 0x41;
const TRON_ADDRESS_LEN: usize = 21;
/// `types` 的字段总数上限.`hashStruct` 对每个结构体都要算一次类型串,这个上限让最坏情况也有界.
const MAX_TYPE_FIELDS: usize = 512;

const PERMIT_FILES: [&str; 2] = [
    include_str!("../../../../schemas/erc7730/eip712-erc2612-permit.json"),
    include_str!("../../../../schemas/erc7730/bitone-eip712.json"),
];

#[derive(Deserialize)]
struct PermitDisplay {
    formats: BTreeMap<String, Json>,
}

#[derive(Deserialize)]
struct PermitFile {
    display: PermitDisplay,
}

static PERMITS: OnceLock<BTreeSet<String>> = OnceLock::new();

/// Permit 和 Permit2 的规范类型串.名字相同但字段不同的是另一种消息,所以逐字比对.
/// DAI 的 Permit 用 `allowed` 代替 `value`,按这里的页面显示的额度就和实际授权无关.
pub fn permit_types() -> &'static BTreeSet<String> {
    PERMITS.get_or_init(|| {
        let files = PERMIT_FILES
            .iter()
            .map(|text| serde_json::from_str::<PermitFile>(text).expect("内置描述文件有效"));
        files.flat_map(|file| file.display.formats.into_keys()).collect()
    })
}

/// `serde_json` 对重复键保留最后一个,不同解析器看到的内容就不同了,所以这里自己收集.
/// 小数和超过 64 位的数字也一样,各个解析器取到的值不同,放在字符串里才有确定的值.
struct Strict(Json);

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Json, E> {
        Ok(Json::from(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::from(value))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Json, E> {
        Err(E::custom("小数和超过 64 位的数字"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_string()))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = access.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Json, A::Error> {
        let mut object = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            let Strict(value) = access.next_value()?;
            if object.insert(key, value).is_some() {
                return Err(de::Error::custom("重复的键"));
            }
        }
        Ok(Json::Object(object))
    }
}

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Strict)
    }
}

/// 这份 JSON 的规则.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypedFlavor {
    Eip712,
    Tip712,
}

type Fields = Vec<(String, String)>;

fn field_of(json: &Json) -> Result<(String, String), Reject> {
    let object = json.as_object().ok_or(Reject::Malformed)?;
    let name = object.get(FIELD_NAME).and_then(Json::as_str);
    let kind = object.get(FIELD_TYPE).and_then(Json::as_str);
    match name.zip(kind) {
        Some((name, kind)) if object.len() == 2 => Ok((name.to_string(), kind.to_string())),
        Some(_) | None => Err(Reject::Malformed),
    }
}

fn fields_of(json: &Json) -> Result<Fields, Reject> {
    json.as_array().ok_or(Reject::Malformed)?.iter().map(field_of).collect()
}

/// tronweb 的 `toHex`:Base58,`41` 开头或 `0x` 开头的十六进制.
fn tron_address(text: &str) -> Option<Address> {
    if let Some(hex) = text.strip_prefix("0x") {
        return hex.parse().ok();
    }
    let bytes = bitcoin::base58::decode_check(text).ok();
    let bytes = bytes.or_else(|| Vec::from_hex(text).ok())?;
    let (prefix, rest) = bytes.split_first()?;
    (*prefix == TRON_ADDRESS_PREFIX && bytes.len() == TRON_ADDRESS_LEN).then(|| Address::from_slice(rest))
}

struct Schema {
    types: BTreeMap<String, Fields>,
    flavor: TypedFlavor,
    /// 每个类型的 `typeHash`.
    hashes: RefCell<BTreeMap<String, B256>>,
}

impl Schema {
    fn new(types: BTreeMap<String, Fields>, flavor: TypedFlavor) -> Self {
        Self {
            types,
            flavor,
            hashes: RefCell::default(),
        }
    }

    fn dependencies<'a>(&'a self, kind: &'a str, found: &mut BTreeSet<&'a str>) {
        let name = kind.split('[').next().unwrap_or(kind);
        if found.contains(name) {
            return;
        }
        if let Some(fields) = self.types.get(name) {
            found.insert(name);
            fields.iter().for_each(|(_, kind)| self.dependencies(kind, found));
        }
    }

    /// EIP-712 的 `encodeType`.类型串来自请求,`trcToken` 这样的原文也进哈希.
    fn encode_type(&self, primary: &str) -> Result<String, Reject> {
        let mut found = BTreeSet::new();
        self.dependencies(primary, &mut found);
        found.remove(primary);
        let mut out = String::new();
        for name in std::iter::once(primary).chain(found.iter().copied()) {
            let fields = self.types.get(name).ok_or(Reject::Malformed)?;
            let fields: Vec<String> = fields
                .iter()
                .map(|(field, kind)| [kind.as_str(), field].join(" "))
                .collect();
            out.extend([name, "(", fields.join(",").as_str(), ")"]);
        }
        Ok(out)
    }

    fn type_hash(&self, name: &str) -> Result<B256, Reject> {
        if let Some(hash) = self.hashes.borrow().get(name) {
            return Ok(*hash);
        }
        let hash = keccak256(self.encode_type(name)?);
        self.hashes.borrow_mut().insert(name.to_string(), hash);
        Ok(hash)
    }

    fn atom(&self, kind: &str, json: &Json) -> Result<DynSolValue, Reject> {
        let tron = self.flavor == TypedFlavor::Tip712;
        let parsed = if tron && kind == TRC_TOKEN {
            DynSolType::Uint(256)
        } else {
            DynSolType::parse(kind).map_err(|_| Reject::Malformed)?
        };
        match &parsed {
            DynSolType::Address if tron => json
                .as_str()
                .and_then(tron_address)
                .map(DynSolValue::Address)
                .ok_or(Reject::Malformed),
            DynSolType::Bool
            | DynSolType::Int(_)
            | DynSolType::Uint(_)
            | DynSolType::FixedBytes(_)
            | DynSolType::Address
            | DynSolType::Bytes
            | DynSolType::String => parsed.coerce_json(json).map_err(|_| Reject::Malformed),
            // 这些不是 EIP-712 的类型,哈希没有定义.
            DynSolType::Function | DynSolType::Array(_) | DynSolType::FixedArray(_, _) | DynSolType::Tuple(_) => {
                Err(Reject::Malformed)
            }
            DynSolType::CustomStruct {
                name: _name,
                prop_names: _prop_names,
                tuple: _tuple,
            } => Err(Reject::Malformed),
        }
    }

    fn value(&self, kind: &str, json: &Json) -> Result<DynSolValue, Reject> {
        if let Some(inner) = kind.strip_suffix(']') {
            let (element, size) = inner.rsplit_once('[').ok_or(Reject::Malformed)?;
            let items = json.as_array().ok_or(Reject::Malformed)?;
            let values = items
                .iter()
                .map(|item| self.value(element, item))
                .collect::<Result<Vec<_>, _>>()?;
            return if size.is_empty() {
                Ok(DynSolValue::Array(values))
            } else if size.parse() == Ok(values.len()) {
                Ok(DynSolValue::FixedArray(values))
            } else {
                Err(Reject::Malformed)
            };
        }
        if self.types.contains_key(kind) {
            self.structure(kind, json)
        } else {
            self.atom(kind, json)
        }
    }

    /// 不进哈希的成员不能留下,否则屏幕上的内容可以和签名无关.
    fn structure(&self, name: &str, json: &Json) -> Result<DynSolValue, Reject> {
        let object = json.as_object().ok_or(Reject::Malformed)?;
        let fields = self.types.get(name).ok_or(Reject::Malformed)?;
        if object.len() != fields.len() {
            return Err(Reject::Malformed);
        }
        let tuple = fields
            .iter()
            .map(|(field, kind)| self.value(kind, object.get(field).ok_or(Reject::Malformed)?))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DynSolValue::CustomStruct {
            name: name.to_string(),
            prop_names: fields.iter().map(|(field, _)| field.clone()).collect(),
            tuple,
        })
    }

    fn words(&self, values: &[DynSolValue]) -> Result<Vec<u8>, Reject> {
        let mut joined = Vec::with_capacity(values.len() * B256::len_bytes());
        for value in values {
            joined.extend_from_slice(self.word(value)?.as_slice());
        }
        Ok(joined)
    }

    /// `encodeData` 里的一项.
    fn word(&self, value: &DynSolValue) -> Result<B256, Reject> {
        match value {
            DynSolValue::CustomStruct {
                name,
                prop_names: _prop_names,
                tuple,
            } => {
                let mut encoded = self.type_hash(name)?.to_vec();
                encoded.extend(self.words(tuple)?);
                Ok(keccak256(encoded))
            }
            DynSolValue::Array(values) | DynSolValue::FixedArray(values) => Ok(keccak256(self.words(values)?)),
            DynSolValue::Bytes(bytes) => Ok(keccak256(bytes)),
            DynSolValue::String(text) => Ok(keccak256(text)),
            DynSolValue::Bool(_)
            | DynSolValue::Int(_, _)
            | DynSolValue::Uint(_, _)
            | DynSolValue::FixedBytes(_, _)
            | DynSolValue::Address(_)
            | DynSolValue::Function(_)
            | DynSolValue::Tuple(_) => value.as_word().ok_or(Reject::Malformed),
        }
    }

    /// alloy 的 `Resolver` 不认识 `trcToken`.类型串换成 `uint256` 后两边的结果才能比较,值不变.
    fn plain(&self) -> Self {
        let plain = |(field, kind): &(String, String)| {
            let tail = kind
                .strip_prefix(TRC_TOKEN)
                .filter(|tail| tail.is_empty() || tail.starts_with('['));
            (
                field.clone(),
                tail.map_or_else(|| kind.clone(), |tail| [UINT256, tail].concat()),
            )
        };
        let types = self.types.iter();
        let types = types.map(|(name, fields)| (name.clone(), fields.iter().map(plain).collect()));
        Self::new(types.collect(), TypedFlavor::Eip712)
    }

    /// 不认识的类型名或互相引用的类型时是 `None`.
    fn resolver(&self) -> Option<Resolver> {
        let mut resolver = Resolver::default();
        for (name, fields) in &self.types {
            let props = fields
                .iter()
                .map(|(field, kind)| PropertyDef::new(kind.as_str(), field.as_str()));
            let props = props.collect::<Result<Vec<_>, _>>().ok()?;
            resolver.ingest(TypeDef::new(name.as_str(), props).ok()?);
        }
        Some(resolver)
    }
}

/// C2.`values` 是域和消息.alloy 表示不了多个类型互相引用,这种消息没有第二份实现,靠 viem 的向量.
fn second_opinion(schema: &Schema, values: &[&DynSolValue]) -> Result<RoundTrip, Reject> {
    let plain = schema.plain();
    let ours = values
        .iter()
        .map(|value| plain.word(value))
        .collect::<Result<Vec<_>, _>>()?;
    let resolver = plain.resolver();
    let theirs = values
        .iter()
        .map(|value| resolver.as_ref()?.eip712_data_word(value).ok())
        .collect::<Option<Vec<_>>>();
    Ok(RoundTrip {
        original: ours.concat(),
        rebuilt: theirs.map_or_else(|| ours.concat(), |words| words.concat()),
    })
}

/// 解析好的消息.树和摘要都来自这里.
#[derive(Clone, Debug, PartialEq)]
pub struct TypedMessage {
    pub primary: String,
    pub domain: DynSolValue,
    pub message: DynSolValue,
    /// `primaryType` 的 `encodeType`.
    pub encode_type: String,
    pub types: BTreeMap<String, Fields>,
    pub domain_separator: B256,
    /// 主类型是 `EIP712Domain` 时没有这一项,和 viem,MetaMask 一致.
    pub hash_struct: Option<B256>,
    pub digest: B256,
    check: RoundTrip,
}

/// 结构体的一个成员.
pub fn typed_member<'a>(value: &'a DynSolValue, name: &str) -> Option<&'a DynSolValue> {
    let DynSolValue::CustomStruct {
        name: _name,
        prop_names,
        tuple,
    } = value
    else {
        return None;
    };
    let index = prop_names.iter().position(|prop| prop == name)?;
    tuple.get(index)
}

impl TypedMessage {
    /// 类型是 `uint256` 才算绑定了链.
    pub fn chain_id(&self) -> Option<U256> {
        if let Some(DynSolValue::Uint(number, 256)) = typed_member(&self.domain, CHAIN_ID) {
            Some(*number)
        } else {
            None
        }
    }

    pub fn verifying_contract(&self) -> Option<Address> {
        if let Some(DynSolValue::Address(address)) = typed_member(&self.domain, VERIFYING_CONTRACT) {
            Some(*address)
        } else {
            None
        }
    }

    pub fn is_permit(&self) -> bool {
        permit_types().contains(&self.encode_type)
    }
}

fn required<'a>(object: &'a Map<String, Json>, name: &str) -> Result<&'a Json, Reject> {
    object.get(name).ok_or(Reject::Malformed)
}

/// 解析并算出摘要.重复键,多余成员,类型和值不符都失败.
pub fn parse_typed(payload: &[u8], flavor: TypedFlavor) -> Result<TypedMessage, Reject> {
    let Strict(root) = serde_json::from_slice(payload).map_err(|_| Reject::Malformed)?;
    let root = root.as_object().ok_or(Reject::Malformed)?;
    if root.len() != [TYPES, PRIMARY_TYPE, DOMAIN, MESSAGE].len() {
        return Err(Reject::Malformed);
    }
    let primary = required(root, PRIMARY_TYPE)?.as_str().ok_or(Reject::Malformed)?;
    let domain = required(root, DOMAIN)?;
    let message = required(root, MESSAGE)?;
    let declared = required(root, TYPES)?.as_object().ok_or(Reject::Malformed)?;
    let mut types = declared
        .iter()
        .map(|(name, fields)| Ok((name.clone(), fields_of(fields)?)))
        .collect::<Result<BTreeMap<_, _>, Reject>>()?;
    if types.values().map(Vec::len).sum::<usize>() > MAX_TYPE_FIELDS {
        return Err(Reject::Malformed);
    }
    if !types.contains_key(DOMAIN_TYPE) {
        let present = domain.as_object().ok_or(Reject::Malformed)?;
        let fields = DOMAIN_FIELDS.iter().filter(|(name, _)| present.contains_key(*name));
        let fields = fields.map(|(name, kind)| (name.to_string(), kind.to_string()));
        types.insert(DOMAIN_TYPE.to_string(), fields.collect());
    }
    let schema = Schema::new(types, flavor);
    let domain = schema.structure(DOMAIN_TYPE, domain)?;
    let domain_separator = schema.word(&domain)?;
    let (message, hash_struct) = if primary != DOMAIN_TYPE {
        let body = schema.structure(primary, message)?;
        let hash = schema.word(&body)?;
        (body, Some(hash))
    } else if message.as_object().is_some_and(Map::is_empty) {
        // 这时只哈希域,`message` 不进摘要,所以不能有内容.
        (domain.clone(), None)
    } else {
        return Err(Reject::Malformed);
    };
    let mut preimage = vec![0x19, 0x01];
    preimage.extend_from_slice(domain_separator.as_slice());
    preimage.extend(hash_struct.iter().flat_map(|hash| hash.0));
    let check = second_opinion(&schema, &[&domain, &message])?;
    Ok(TypedMessage {
        primary: primary.to_string(),
        encode_type: schema.encode_type(primary)?,
        domain,
        message,
        domain_separator,
        hash_struct,
        digest: keccak256(preimage),
        types: schema.types,
        check,
    })
}

fn inherited(name: impl Into<String>, value: Value) -> Node {
    Node::new(name, value, Source::Signed(Extent::Inherited))
}

fn derived_hash(name: &str, hash: B256) -> Node {
    Node::new(name, Value::Hash(hash.to_vec()), Source::Derived)
}

/// `labels` 是描述文件给的字段名,没有描述文件时是 `None`.`chain_id` 是这笔签名所在网络的 Chain ID,不知道时不比较.
pub struct TypedWalker<'a> {
    pub parsed: &'a TypedMessage,
    pub chain_id: Option<u64>,
    pub format: AddressText<'a>,
    pub labels: Option<&'a [(String, String)]>,
}

impl TypedWalker<'_> {
    fn action(&self) -> &'static str {
        if self.parsed.is_permit() {
            "eip712-permit"
        } else if self.labels.is_some() {
            "eip712-described"
        } else {
            "eip712-message"
        }
    }

    /// 域没有绑定的内容要有一行,规则挂在这一行上.
    fn domain(&self) -> Node {
        let source = Source::Signed(Extent::Inherited);
        let mut node = abi_node(DOMAIN.to_string(), &self.parsed.domain, source, self.format);
        let absent = |name: &str, kind: &str| Node::new(name, Value::Enum(absent_text()), Source::Derived).action(kind);
        match (self.parsed.chain_id(), self.chain_id) {
            (None, _) => node.children.push(absent(CHAIN_ID, "eip712-no-chain-id")),
            (Some(found), Some(expected)) if found != U256::from(expected) => {
                let row = node.children.iter_mut().find(|row| row.name == CHAIN_ID);
                row.into_iter()
                    .for_each(|row| row.action = Some("eip712-other-chain".to_string()));
            }
            (Some(_), Some(_) | None) => {}
        }
        if self.parsed.verifying_contract().is_none() {
            node.children.push(absent(VERIFYING_CONTRACT, "eip712-no-contract"));
        }
        node
    }

    fn message(&self) -> Node {
        let source = Source::Signed(Extent::Inherited);
        let mut node = abi_node(MESSAGE.to_string(), &self.parsed.message, source, self.format);
        for (path, label) in self.labels.unwrap_or_default() {
            let row = node.children.iter_mut().find(|row| row.name == *path);
            row.into_iter().for_each(|row| row.name.clone_from(label));
        }
        node
    }

    fn types(&self) -> Node {
        let rows = self.parsed.types.iter().map(|(name, fields)| {
            let fields = fields
                .iter()
                .map(|(field, kind)| inherited(field.as_str(), Value::Text(kind.clone())));
            inherited(name.as_str(), Value::Struct).children(fields.collect())
        });
        inherited(TYPES, Value::List).children(rows.collect())
    }
}

impl Decoder for TypedWalker<'_> {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let parsed = self.parsed;
        let mut children = vec![
            self.domain(),
            inherited("primary_type", Value::Text(parsed.primary.clone())),
            self.message(),
            self.types(),
            derived_hash("domain_separator", parsed.domain_separator),
        ];
        children.extend(parsed.hash_struct.map(|hash| derived_hash("hash_struct", hash)));
        children.push(derived_hash("signing_hash", parsed.digest));
        let root = Node::new(
            "typed_data",
            Value::Struct,
            Source::Signed(Extent::Own(whole(input.payload).span)),
        );
        Ok(Some(Walked {
            root: root.children(children).action(self.action()),
            round_trips: vec![parsed.check.clone()],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_dyn_abi::TypedData;

    const MAIL: &str = r#"{
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ],
            "Person": [{"name": "wallet", "type": "address"}],
            "Mail": [{"name": "from", "type": "Person"}, {"name": "note", "type": "string"}]
        },
        "primaryType": "Mail",
        "domain": {
            "name": "Mail",
            "chainId": 1,
            "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
        },
        "message": {"from": {"wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"}, "note": "hi"}
    }"#;

    /// C2 比较的是 `Resolver` 算出的结果,不是自己和自己比.`TypedData` 是 alloy 自己的另一条路径.
    #[test]
    fn alloy_gets_the_same_hash() {
        let parsed = parse_typed(MAIL.as_bytes(), TypedFlavor::Eip712).unwrap();
        assert_eq!(parsed.check.original, parsed.check.rebuilt);
        assert_eq!(parsed.check.rebuilt.len(), 2 * B256::len_bytes());
        let official: TypedData = serde_json::from_str(MAIL).unwrap();
        assert_eq!(official.eip712_signing_hash().unwrap(), parsed.digest);
        assert_eq!(official.hash_struct().unwrap(), parsed.hash_struct.unwrap());
    }

    #[test]
    fn a_wrong_hash_fails_the_comparison() {
        let parsed = parse_typed(MAIL.as_bytes(), TypedFlavor::Eip712).unwrap();
        let schema = Schema::new(parsed.types.clone(), TypedFlavor::Eip712);
        let other = DynSolValue::CustomStruct {
            name: "Person".to_string(),
            prop_names: vec!["wallet".to_string()],
            tuple: vec![DynSolValue::Address(Address::ZERO)],
        };
        let mut check = second_opinion(&schema, &[&parsed.domain, &other]).unwrap();
        assert_eq!(check.original, check.rebuilt);
        check.original[0] ^= 1;
        assert_eq!(
            super::super::check::round_trip(&[check]).unwrap_err(),
            Reject::RoundTrip
        );
    }

    #[test]
    fn a_trc_token_only_counts_for_tip712() {
        let payload = br#"{
            "types": {"EIP712Domain": [], "T": [{"name": "id", "type": "trcToken"}, {"name": "ids", "type": "trcToken[]"}]},
            "primaryType": "T",
            "domain": {},
            "message": {"id": "1000001", "ids": ["1"]}
        }"#;
        assert!(parse_typed(payload, TypedFlavor::Eip712).is_err());
        let parsed = parse_typed(payload, TypedFlavor::Tip712).unwrap();
        assert_eq!(parsed.encode_type, "T(trcToken id,trcToken[] ids)");
        assert_eq!(
            typed_member(&parsed.message, "id"),
            Some(&DynSolValue::Uint(U256::from(1000001), 256))
        );
    }

    #[test]
    fn tron_addresses_have_three_spellings() {
        let raw = "a2726afbecbd8e936000ed684cef5e2f5cf43008";
        let expected: Address = raw.parse().unwrap();
        for text in [
            "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE",
            &format!("41{raw}"),
            &format!("0x{raw}"),
        ] {
            assert_eq!(tron_address(text), Some(expected), "{text}");
        }
        for text in [raw, "42a2726afbecbd8e936000ed684cef5e2f5cf43008", "T", ""] {
            assert_eq!(tron_address(text), None, "{text}");
        }
    }

    #[test]
    fn too_many_fields_are_refused_before_any_hashing() {
        let fields: Vec<Json> = (0..=MAX_TYPE_FIELDS)
            .map(|index| serde_json::json!({"name": index.to_string(), "type": "uint8"}))
            .collect();
        let data = serde_json::json!({
            "types": {"EIP712Domain": [], "T": fields},
            "primaryType": "T",
            "domain": {},
            "message": {}
        });
        let payload = serde_json::to_vec(&data).unwrap();
        assert_eq!(
            parse_typed(&payload, TypedFlavor::Eip712).unwrap_err(),
            Reject::Malformed
        );
    }
}

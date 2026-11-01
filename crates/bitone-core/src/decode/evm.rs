//! EVM 解码.类型字节分发到 alloy 的官方类型,字段的名字和顺序在 assets/evm-schema.json,
//! 这里只认 RLP 项.已知的签名域按真实含义解码,不算未知格式.

use super::psbt::Cursor;
use super::psbt_values::Field;
use super::rlp::{Item, items, whole_list};
use super::{Amount, Decoder, Extent, Input, Node, Reject, RoundTrip, Source, Span, Value, Walked};
use alloy_consensus::{
    EthereumTypedTransaction, SignableTransaction, Transaction, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
    TxType,
};
use alloy_eips::eip2930::{AccessList, AccessListItem};
use alloy_eips::eip4844::DATA_GAS_PER_BLOB;
use alloy_eips::eip7702::constants::{MAGIC, SECP256K1N_HALF};
use alloy_eips::eip7702::{Authorization, SignedAuthorization};
use alloy_primitives::utils::{EIP191_PREFIX, eip191_message};
use alloy_primitives::{Address, B256, Bytes, TxKind, U256, eip191_hash_message, keccak256};
use alloy_rlp::{Decodable, EMPTY_LIST_CODE, Encodable};
use bitcoin::secp256k1::{Message, Secp256k1, ecdsa};
use serde::Deserialize;
use std::sync::OnceLock;

/// 官方类型.blob 交易只有签名原文,边车不进签名.
pub type Tx = EthereumTypedTransaction<TxEip4844>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GasUnit {
    unit: String,
    decimals: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TxSchema {
    label: String,
    action: String,
    fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transactions {
    legacy: TxSchema,
    eip2930: TxSchema,
    eip1559: TxSchema,
    eip4844: TxSchema,
    eip7702: TxSchema,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Eip191 {
    tag: u8,
    validator: u8,
    typed_data: u8,
    personal: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    gas: GasUnit,
    transactions: Transactions,
    eip155: Vec<String>,
    access_entry: Vec<String>,
    authorization: Vec<String>,
    authorization_message: Vec<String>,
    eip191: Eip191,
    contract_creation: String,
    no_chain_id: String,
    unrecovered: String,
}

static SCHEMA: OnceLock<Schema> = OnceLock::new();

fn schema() -> &'static Schema {
    SCHEMA.get_or_init(|| serde_json::from_str(include_str!("../../assets/evm-schema.json")).expect("内置 EVM 表有效"))
}

/// 全部交易类型在规则表里的种类名.
pub fn transaction_actions() -> [&'static str; 5] {
    let Transactions {
        legacy,
        eip2930,
        eip1559,
        eip4844,
        eip7702,
    } = &schema().transactions;
    [legacy, eip2930, eip1559, eip4844, eip7702].map(|entry| entry.action.as_str())
}

/// 原生币.本机不认识的网络没有这一项,金额显示原始整数.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coin {
    pub unit: &'static str,
    pub decimals: u8,
}

/// 载荷的格式.`0x19`,`0x05` 是已知的签名域,解不开时拒绝,永远不算未知格式.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvmPayload {
    Transaction(Box<Tx>),
    Authorization(Authorization),
    /// EIP-191 和 EIP-712 的摘要,内容由遍历器检查.
    Message,
    Unknown,
}

fn is_transaction(first: u8) -> bool {
    // 字面的 0x00 不是类型字节,旧式交易没有类型字节.
    first >= EMPTY_LIST_CODE || TxType::try_from(first).is_ok_and(|ty| !ty.is_legacy())
}

fn parse_transaction(payload: &[u8]) -> Result<Tx, Reject> {
    let mut rest = payload;
    let tx = Tx::decode_unsigned(&mut rest).map_err(|_| Reject::Malformed)?;
    if !rest.is_empty() {
        return Err(Reject::Malformed);
    }
    if tx.encoded_for_signing() != payload {
        return Err(Reject::RoundTrip);
    }
    Ok(tx)
}

fn parse_authorization(mut body: &[u8]) -> Result<Authorization, Reject> {
    let authorization = Authorization::decode(&mut body).map_err(|_| Reject::Malformed)?;
    if !body.is_empty() {
        return Err(Reject::Malformed);
    }
    Ok(authorization)
}

pub fn parse_evm(payload: &[u8]) -> Result<EvmPayload, Reject> {
    let (first, rest) = payload.split_first().ok_or(Reject::Malformed)?;
    if *first == schema().eip191.tag {
        Ok(EvmPayload::Message)
    } else if *first == MAGIC {
        parse_authorization(rest).map(EvmPayload::Authorization)
    } else if is_transaction(*first) {
        parse_transaction(payload).map(|tx| EvmPayload::Transaction(Box::new(tx)))
    } else {
        Ok(EvmPayload::Unknown)
    }
}

/// 手续费上限,类型 0x03 加上 blob 费.
pub fn fee_cap(tx: &Tx) -> U256 {
    let gas = U256::from(tx.gas_limit()).saturating_mul(U256::from(tx.max_fee_per_gas()));
    let blobs = tx.blob_versioned_hashes().map_or(0, <[B256]>::len);
    let blob_gas = U256::from(blobs).saturating_mul(U256::from(DATA_GAS_PER_BLOB));
    let blob_fee = U256::from(tx.max_fee_per_blob_gas().unwrap_or_default()).saturating_mul(blob_gas);
    gas.saturating_add(blob_fee)
}

/// 由签名恢复授权人.EIP-7702 要求低 s,恢复位只能是 0 或 1.
pub fn authority(authorization: &SignedAuthorization) -> Option<Address> {
    let signature = authorization.signature().ok()?;
    if signature.s() > SECP256K1N_HALF {
        return None;
    }
    let bytes = signature.as_bytes();
    let (_, compact) = bytes.split_last()?;
    let recovery = ecdsa::RecoveryId::from_i32(i32::from(signature.v())).ok()?;
    let recoverable = ecdsa::RecoverableSignature::from_compact(compact, recovery).ok()?;
    let digest = authorization.inner().signature_hash();
    let key = Secp256k1::verification_only()
        .recover_ecdsa(&Message::from_digest(digest.0), &recoverable)
        .ok()?;
    let uncompressed = key.serialize_uncompressed();
    let (_, raw) = uncompressed.split_first()?;
    Some(Address::from_raw_public_key(raw))
}

fn trimmed(value: U256) -> Vec<u8> {
    value.to_be_bytes_trimmed_vec()
}

fn number(value: U256) -> Value {
    Value::Uint(trimmed(value))
}

fn gas_price(value: u128) -> Value {
    let gas = &schema().gas;
    Value::Amount(Amount {
        raw: trimmed(U256::from(value)),
        decimals: Some(gas.decimals),
        unit: Some(gas.unit.clone()),
    })
}

fn message_value(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(|_| Value::Bytes(bytes.to_vec()), |text| Value::Text(text.to_string()))
}

fn leaf(name: &str, item: &Item<'_>, value: Value) -> Node {
    Node::new(name, value, Source::Signed(Extent::Own(item.body.span))).framed(vec![item.head])
}

fn group(name: &str, item: &Item<'_>, value: Value, children: Vec<Node>) -> Node {
    Node::new(name, value, Source::Signed(Extent::Container))
        .framed(vec![item.head])
        .children(children)
}

fn own(name: &str, value: Value, span: Span) -> Node {
    Node::new(name, value, Source::Signed(Extent::Own(span)))
}

fn signing_hash(digest: B256) -> Node {
    Node::new("signing_hash", Value::Hash(digest.to_vec()), Source::Derived)
}

pub(super) fn whole(payload: &[u8]) -> Field<'_> {
    Field {
        bytes: payload,
        span: Span::new(0, payload.len() as u32),
    }
}

/// 不存在的字段显示的文字.
pub(super) fn absent_text() -> String {
    schema().no_chain_id.clone()
}

/// 列表的子项和 schema 的字段一一对应.
struct Named<'a> {
    fields: Vec<(&'static str, Item<'a>)>,
}

impl<'a> Named<'a> {
    fn of(list: &Item<'a>, names: &'static [String]) -> Result<Self, Reject> {
        Self::with(list, names.iter().map(String::as_str).collect())
    }

    fn with(list: &Item<'a>, names: Vec<&'static str>) -> Result<Self, Reject> {
        let found = items(list)?;
        if found.len() != names.len() {
            return Err(Reject::Malformed);
        }
        Ok(Self {
            fields: names.into_iter().zip(found).collect(),
        })
    }

    fn take(&mut self, name: &str) -> Result<Item<'a>, Reject> {
        let index = self
            .fields
            .iter()
            .position(|(field, _)| *field == name)
            .ok_or(Reject::Malformed)?;
        Ok(self.fields.swap_remove(index).1)
    }

    /// schema 有而官方类型没有的字段走不到这里.
    fn finish(self) -> Result<(), Reject> {
        if self.fields.is_empty() {
            Ok(())
        } else {
            Err(Reject::Malformed)
        }
    }
}

/// 一次遍历.每个字段都和官方类型的编码比较,schema 的顺序错了 C2 不通过.
struct Walk {
    coin: Option<Coin>,
    checks: Vec<RoundTrip>,
    /// 描述文件解出的字段,挂在 `data` 下面.
    call: Vec<Node>,
}

impl Walk {
    fn new(coin: Option<Coin>, payload: &[u8], rebuilt: Vec<u8>) -> Self {
        Self {
            coin,
            checks: vec![RoundTrip {
                original: payload.to_vec(),
                rebuilt,
            }],
            call: Vec::new(),
        }
    }

    fn compare(&mut self, raw: &[u8], value: &impl Encodable) {
        self.checks.push(RoundTrip {
            original: raw.to_vec(),
            rebuilt: alloy_rlp::encode(value),
        });
    }

    fn scalar<'a>(
        &mut self,
        fields: &mut Named<'a>,
        name: &str,
        value: &impl Encodable,
        shown: Value,
    ) -> Result<Node, Reject> {
        let item = fields.take(name)?;
        self.compare(item.raw, value);
        Ok(leaf(name, &item, shown))
    }

    fn coin_amount(&self, value: U256) -> Value {
        Value::Amount(Amount {
            raw: trimmed(value),
            decimals: self.coin.map(|coin| coin.decimals),
            unit: self.coin.map(|coin| coin.unit.to_string()),
        })
    }

    fn chain_id(&mut self, fields: &mut Named<'_>, chain_id: u64) -> Result<Node, Reject> {
        self.scalar(fields, "chain_id", &chain_id, number(U256::from(chain_id)))
    }

    fn nonce(&mut self, fields: &mut Named<'_>, nonce: u64) -> Result<Node, Reject> {
        self.scalar(fields, "nonce", &nonce, number(U256::from(nonce)))
    }

    fn gas_limit(&mut self, fields: &mut Named<'_>, gas_limit: u64) -> Result<Node, Reject> {
        self.scalar(fields, "gas_limit", &gas_limit, number(U256::from(gas_limit)))
    }

    fn price(&mut self, fields: &mut Named<'_>, name: &str, price: u128) -> Result<Node, Reject> {
        self.scalar(fields, name, &price, gas_price(price))
    }

    /// 动作按 `to` 和 `data` 判定:创建合约,原生币转账或合约调用.
    fn call(&mut self, fields: &mut Named<'_>, to: TxKind, value: U256, input: &Bytes) -> Result<[Node; 3], Reject> {
        let target = match to {
            TxKind::Call(address) => Value::Address(address.to_string()),
            TxKind::Create => Value::Enum(schema().contract_creation.clone()),
        };
        let target = self.scalar(fields, "to", &to, target)?;
        let amount = self.coin_amount(value);
        let amount = self.scalar(fields, "value", &value, amount)?;
        let data = self.scalar(fields, "data", input, Value::Bytes(input.to_vec()))?;
        Ok(match (to, input.is_empty()) {
            (TxKind::Create, true | false) => [target.action("contract-creation"), amount, data],
            (TxKind::Call(_), true) => [target.action("native-transfer"), amount, data],
            (TxKind::Call(_), false) => {
                let fields = std::mem::take(&mut self.call);
                [target, amount, data.action("call").children(fields)]
            }
        })
    }

    fn access_list(&mut self, fields: &mut Named<'_>, list: &AccessList) -> Result<Node, Reject> {
        let item = fields.take("access_list")?;
        self.compare(item.raw, list);
        let rows = items(&item)?;
        if rows.len() != list.0.len() {
            return Err(Reject::Malformed);
        }
        let entries = rows
            .iter()
            .zip(&list.0)
            .map(|(row, entry)| self.access_entry(row, entry))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(group("access_list", &item, Value::List, entries))
    }

    fn access_entry(&mut self, row: &Item<'_>, entry: &AccessListItem) -> Result<Node, Reject> {
        let AccessListItem { address, storage_keys } = entry;
        let mut fields = Named::of(row, &schema().access_entry)?;
        let target = self.scalar(&mut fields, "address", address, Value::Address(address.to_string()))?;
        let keys = self.hashes(&mut fields, "storage_keys", "storage_key", storage_keys)?;
        fields.finish()?;
        Ok(group("access_entry", row, Value::Struct, vec![target, keys]))
    }

    fn hashes(&mut self, fields: &mut Named<'_>, name: &str, entry: &str, hashes: &[B256]) -> Result<Node, Reject> {
        let item = fields.take(name)?;
        let rows = items(&item)?;
        if rows.len() != hashes.len() {
            return Err(Reject::Malformed);
        }
        let children = rows
            .iter()
            .zip(hashes)
            .map(|(row, hash)| {
                self.compare(row.raw, hash);
                leaf(entry, row, Value::Hash(hash.to_vec()))
            })
            .collect();
        Ok(group(name, &item, Value::List, children))
    }

    /// 授权元组的三个字段.`expected` 是这笔签名所在网络的 Chain ID,不知道时不比较.
    fn authorization_fields(
        &mut self,
        fields: &mut Named<'_>,
        authorization: &Authorization,
        expected: Option<u64>,
    ) -> Result<[Node; 3], Reject> {
        let Authorization {
            chain_id,
            address,
            nonce,
        } = authorization;
        let target = self.scalar(fields, "chain_id", chain_id, number(*chain_id))?;
        let target = if chain_id.is_zero() {
            target.action("authorization-every-chain")
        } else if expected.is_some_and(|id| *chain_id != U256::from(id)) {
            target.action("authorization-other-chain")
        } else {
            target
        };
        let delegate = self.scalar(fields, "address", address, Value::Address(address.to_string()))?;
        let delegate = if address.is_zero() {
            delegate.action("authorization-clear")
        } else {
            delegate
        };
        Ok([target, delegate, self.nonce(fields, *nonce)?])
    }

    fn authorization(
        &mut self,
        row: &Item<'_>,
        authorization: &SignedAuthorization,
        expected: Option<u64>,
    ) -> Result<Node, Reject> {
        self.compare(row.raw, authorization);
        let mut fields = Named::of(row, &schema().authorization)?;
        let mut children = self
            .authorization_fields(&mut fields, authorization.inner(), expected)?
            .to_vec();
        let y_parity = authorization.y_parity();
        children.push(self.scalar(&mut fields, "y_parity", &y_parity, number(U256::from(y_parity)))?);
        let [r, s] = [("r", authorization.r()), ("s", authorization.s())];
        for (name, value) in [r, s] {
            children.push(self.scalar(&mut fields, name, &value, Value::Hash(value.to_be_bytes_vec()))?);
        }
        fields.finish()?;
        children.push(match authority(authorization) {
            Some(address) => Node::new("authority", Value::Address(address.to_string()), Source::Derived),
            None => Node::new("authority", Value::Enum(schema().unrecovered.clone()), Source::Derived)
                .action("authorization-unrecovered"),
        });
        Ok(group("authorization", row, Value::Struct, children).action("authorization"))
    }

    fn authorization_list(
        &mut self,
        fields: &mut Named<'_>,
        list: &[SignedAuthorization],
        chain_id: u64,
    ) -> Result<Node, Reject> {
        let item = fields.take("authorization_list")?;
        let rows = items(&item)?;
        // 空的授权列表链也不接受.
        if rows.len() != list.len() || list.is_empty() {
            return Err(Reject::Malformed);
        }
        let entries = rows
            .iter()
            .zip(list)
            .map(|(row, entry)| self.authorization(row, entry, Some(chain_id)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(group("authorization_list", &item, Value::List, entries))
    }

    fn legacy(&mut self, tx: &TxLegacy, entry: &'static TxSchema, list: &Item<'_>) -> Result<Vec<Node>, Reject> {
        let TxLegacy {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        } = tx;
        let replay = chain_id.iter().flat_map(|_| &schema().eip155);
        let names = entry.fields.iter().chain(replay).map(String::as_str).collect();
        let mut fields = Named::with(list, names)?;
        let mut nodes = vec![
            self.nonce(&mut fields, *nonce)?,
            self.price(&mut fields, "gas_price", *gas_price)?,
            self.gas_limit(&mut fields, *gas_limit)?,
        ];
        nodes.extend(self.call(&mut fields, *to, *value, input)?);
        match chain_id {
            Some(id) => {
                nodes.push(self.chain_id(&mut fields, *id)?);
                // 九项形式的末两项必须为 0,官方解码器不查.
                nodes.push(self.scalar(&mut fields, "eip155_r", &0u8, number(U256::ZERO))?);
                nodes.push(self.scalar(&mut fields, "eip155_s", &0u8, number(U256::ZERO))?);
            }
            None => nodes.push(
                Node::new("chain_id", Value::Enum(schema().no_chain_id.clone()), Source::Derived).action("no-chain-id"),
            ),
        }
        fields.finish()?;
        Ok(nodes)
    }

    fn eip2930(&mut self, tx: &TxEip2930, entry: &'static TxSchema, list: &Item<'_>) -> Result<Vec<Node>, Reject> {
        let TxEip2930 {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            access_list,
            input,
        } = tx;
        let mut fields = Named::of(list, &entry.fields)?;
        let mut nodes = vec![
            self.chain_id(&mut fields, *chain_id)?,
            self.nonce(&mut fields, *nonce)?,
            self.price(&mut fields, "gas_price", *gas_price)?,
            self.gas_limit(&mut fields, *gas_limit)?,
        ];
        nodes.extend(self.call(&mut fields, *to, *value, input)?);
        nodes.push(self.access_list(&mut fields, access_list)?);
        fields.finish()?;
        Ok(nodes)
    }

    fn dynamic_fees(
        &mut self,
        fields: &mut Named<'_>,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
    ) -> Result<Vec<Node>, Reject> {
        Ok(vec![
            self.chain_id(fields, chain_id)?,
            self.nonce(fields, nonce)?,
            self.price(fields, "max_priority_fee_per_gas", max_priority_fee_per_gas)?,
            self.price(fields, "max_fee_per_gas", max_fee_per_gas)?,
            self.gas_limit(fields, gas_limit)?,
        ])
    }

    fn eip1559(&mut self, tx: &TxEip1559, entry: &'static TxSchema, list: &Item<'_>) -> Result<Vec<Node>, Reject> {
        let TxEip1559 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        } = tx;
        let mut fields = Named::of(list, &entry.fields)?;
        let mut nodes = self.dynamic_fees(
            &mut fields,
            *chain_id,
            *nonce,
            *max_priority_fee_per_gas,
            *max_fee_per_gas,
            *gas_limit,
        )?;
        nodes.extend(self.call(&mut fields, *to, *value, input)?);
        nodes.push(self.access_list(&mut fields, access_list)?);
        fields.finish()?;
        Ok(nodes)
    }

    fn eip4844(&mut self, tx: &TxEip4844, entry: &'static TxSchema, list: &Item<'_>) -> Result<Vec<Node>, Reject> {
        let TxEip4844 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            input,
        } = tx;
        let mut fields = Named::of(list, &entry.fields)?;
        let mut nodes = self.dynamic_fees(
            &mut fields,
            *chain_id,
            *nonce,
            *max_priority_fee_per_gas,
            *max_fee_per_gas,
            *gas_limit,
        )?;
        nodes.extend(self.call(&mut fields, TxKind::Call(*to), *value, input)?);
        nodes.push(self.access_list(&mut fields, access_list)?);
        nodes.push(self.price(&mut fields, "max_fee_per_blob_gas", *max_fee_per_blob_gas)?);
        nodes.push(self.hashes(
            &mut fields,
            "blob_versioned_hashes",
            "blob_versioned_hash",
            blob_versioned_hashes,
        )?);
        fields.finish()?;
        Ok(nodes)
    }

    fn eip7702(&mut self, tx: &TxEip7702, entry: &'static TxSchema, list: &Item<'_>) -> Result<Vec<Node>, Reject> {
        let TxEip7702 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            authorization_list,
            input,
        } = tx;
        let mut fields = Named::of(list, &entry.fields)?;
        let mut nodes = self.dynamic_fees(
            &mut fields,
            *chain_id,
            *nonce,
            *max_priority_fee_per_gas,
            *max_fee_per_gas,
            *gas_limit,
        )?;
        nodes.extend(self.call(&mut fields, TxKind::Call(*to), *value, input)?);
        nodes.push(self.access_list(&mut fields, access_list)?);
        nodes.push(self.authorization_list(&mut fields, authorization_list, *chain_id)?);
        fields.finish()?;
        Ok(nodes)
    }
}

fn type_node(entry: &TxSchema, tx: &Tx, tag: Option<Span>) -> Node {
    let label = format!("0x{:02x} {}", tx.tx_type() as u8, entry.label);
    let source = tag.map_or(Source::Derived, |span| Source::Signed(Extent::Own(span)));
    Node::new("tx_type", Value::Enum(label), source).action(entry.action.clone())
}

fn walk_transaction(tx: &Tx, payload: &[u8], coin: Option<Coin>, call: &[Node]) -> Result<Walked, Reject> {
    let mut walk = Walk::new(coin, payload, tx.encoded_for_signing());
    walk.call = call.to_vec();
    let mut cursor = Cursor::new(&whole(payload));
    let tag = (!tx.tx_type().is_legacy())
        .then(|| cursor.take(1))
        .transpose()?
        .map(|field| field.span);
    let list = whole_list(&mut cursor)?;
    let Transactions {
        legacy,
        eip2930,
        eip1559,
        eip4844,
        eip7702,
    } = &schema().transactions;
    let (entry, fields) = match tx {
        Tx::Legacy(tx) => (legacy, walk.legacy(tx, legacy, &list)?),
        Tx::Eip2930(tx) => (eip2930, walk.eip2930(tx, eip2930, &list)?),
        Tx::Eip1559(tx) => (eip1559, walk.eip1559(tx, eip1559, &list)?),
        Tx::Eip4844(tx) => (eip4844, walk.eip4844(tx, eip4844, &list)?),
        Tx::Eip7702(tx) => (eip7702, walk.eip7702(tx, eip7702, &list)?),
    };
    let cap = Node::new("fee_cap", walk.coin_amount(fee_cap(tx)), Source::Derived);
    let children = [cap, type_node(entry, tx, tag)].into_iter().chain(fields).collect();
    Ok(Walked {
        root: group("transaction", &list, Value::Struct, children),
        round_trips: walk.checks,
    })
}

fn walk_authorization(authorization: &Authorization, payload: &[u8], chain_id: Option<u64>) -> Result<Walked, Reject> {
    let mut cursor = Cursor::new(&whole(payload));
    let magic = cursor.take(1)?;
    let list = whole_list(&mut cursor)?;
    let mut rebuilt = magic.bytes.to_vec();
    authorization.encode(&mut rebuilt);
    let mut walk = Walk::new(None, payload, rebuilt);
    walk.compare(list.raw, authorization);
    let mut fields = Named::of(&list, &schema().authorization_message)?;
    let mut children = walk
        .authorization_fields(&mut fields, authorization, chain_id)?
        .to_vec();
    fields.finish()?;
    children.push(signing_hash(authorization.signature_hash()));
    let root = group("eip7702_authorization", &list, Value::Struct, children);
    Ok(Walked {
        root: root.framed(vec![magic.span, list.head]).action("eip7702-authorization"),
        round_trips: walk.checks,
    })
}

/// 消息的结果.`tag`,`prefix` 这类结构字节进 `framing`.
fn message(
    name: &str,
    action: &str,
    framing: Vec<Span>,
    children: Vec<Node>,
    rebuilt: Vec<u8>,
    payload: &[u8],
) -> Walked {
    let digest = signing_hash(keccak256(payload));
    let root = Node::new(name, Value::Struct, Source::Signed(Extent::Container))
        .framed(framing)
        .children(children.into_iter().chain([digest]).collect())
        .action(action);
    Walked {
        root,
        round_trips: vec![RoundTrip {
            original: payload.to_vec(),
            rebuilt,
        }],
    }
}

/// `0x19 0x01`,域分隔符和消息的哈希.
fn typed_digest(tag: Field<'_>, version: Field<'_>, cursor: &mut Cursor<'_>, payload: &[u8]) -> Result<Walked, Reject> {
    let domain = cursor.take(B256::len_bytes() as u64)?;
    let body = cursor.take(B256::len_bytes() as u64)?;
    if !cursor.is_empty() {
        return Err(Reject::Malformed);
    }
    let rebuilt = [tag.bytes, version.bytes, domain.bytes, body.bytes].concat();
    let children = vec![
        own("domain_separator", Value::Hash(domain.bytes.to_vec()), domain.span),
        own("hash_struct", Value::Hash(body.bytes.to_vec()), body.span),
    ];
    Ok(message(
        "eip712_digest",
        "eip712-digest",
        vec![tag.span, version.span],
        children,
        rebuilt,
        payload,
    ))
}

/// 声明的长度和剩下的字节数相同的那个分界.消息本身可以以数字开头.
fn length_digits(rest: &[u8]) -> Option<usize> {
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    (1..=digits).find(|count| {
        let (text, body) = rest.split_at(*count);
        let declared = std::str::from_utf8(text)
            .ok()
            .and_then(|text| text.parse::<usize>().ok());
        declared == Some(body.len())
    })
}

/// `0x19 0x45`,personal_sign 的完整原文.
fn personal_digest(payload: &[u8]) -> Result<Walked, Reject> {
    let mut cursor = Cursor::new(&whole(payload));
    let prefix = cursor.take(EIP191_PREFIX.len() as u64)?;
    if prefix.bytes != EIP191_PREFIX.as_bytes() {
        return Err(Reject::Malformed);
    }
    let digits = length_digits(cursor.remaining()).ok_or(Reject::Malformed)?;
    let length = cursor.take(digits as u64)?;
    let body = cursor.rest();
    let children = vec![own("message", message_value(body.bytes), body.span)];
    Ok(message(
        "personal_message",
        "personal-sign",
        vec![prefix.span, length.span],
        children,
        eip191_message(body.bytes),
        payload,
    ))
}

/// `0x19 0x00`,验证者地址加数据.
fn validator_data(
    tag: Field<'_>,
    version: Field<'_>,
    cursor: &mut Cursor<'_>,
    payload: &[u8],
) -> Result<Walked, Reject> {
    let validator = cursor.take(Address::len_bytes() as u64)?;
    let body = cursor.rest();
    let rebuilt = [tag.bytes, version.bytes, validator.bytes, body.bytes].concat();
    let address = Address::try_from(validator.bytes).map_err(|_| Reject::Malformed)?;
    let children = vec![
        own("eip191_version", number(U256::ZERO), version.span),
        own("validator", Value::Address(address.to_string()), validator.span),
        own("data", Value::Bytes(body.bytes.to_vec()), body.span),
    ];
    Ok(message(
        "eip191_message",
        "eip191-validator",
        vec![tag.span],
        children,
        rebuilt,
        payload,
    ))
}

/// 以后的 EIP-191 版本.外层是已知的签名域,所以不走 `opaque` 根节点.
fn other_version(
    tag: Field<'_>,
    version: Field<'_>,
    cursor: &mut Cursor<'_>,
    payload: &[u8],
) -> Result<Walked, Reject> {
    let byte = version.bytes.first().ok_or(Reject::Malformed)?;
    let body = cursor.rest();
    let rebuilt = [tag.bytes, version.bytes, body.bytes].concat();
    let children = vec![
        own("eip191_version", number(U256::from(*byte)), version.span),
        own("data", Value::Opaque(body.bytes.to_vec()), body.span),
    ];
    Ok(message(
        "eip191_message",
        "eip191-unknown",
        vec![tag.span],
        children,
        rebuilt,
        payload,
    ))
}

fn walk_message(payload: &[u8]) -> Result<Walked, Reject> {
    let versions = &schema().eip191;
    let mut cursor = Cursor::new(&whole(payload));
    let tag = cursor.take(1)?;
    let version = cursor.take(1)?;
    let byte = version.bytes.first().ok_or(Reject::Malformed)?;
    if *byte == versions.typed_data {
        typed_digest(tag, version, &mut cursor, payload)
    } else if *byte == versions.personal {
        personal_digest(payload)
    } else if *byte == versions.validator {
        validator_data(tag, version, &mut cursor, payload)
    } else {
        other_version(tag, version, &mut cursor, payload)
    }
}

/// 交易和已知的签名域.`coin` 是原生币,`chain_id` 是这笔签名所在网络的 Chain ID.
/// `call` 是 `tx` 层按描述文件解出的字段,来源都是 `descriptor`,解码器不读清单.
#[derive(Clone, Copy, Debug)]
pub struct EvmWalker<'a> {
    pub coin: Option<Coin>,
    pub chain_id: Option<u64>,
    pub call: &'a [Node],
}

impl Decoder for EvmWalker<'_> {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        match parse_evm(input.payload)? {
            EvmPayload::Transaction(tx) => walk_transaction(&tx, input.payload, self.coin, self.call).map(Some),
            EvmPayload::Authorization(authorization) => {
                walk_authorization(&authorization, input.payload, self.chain_id).map(Some)
            }
            EvmPayload::Message => walk_message(input.payload).map(Some),
            EvmPayload::Unknown => Ok(None),
        }
    }
}

/// 裸哈希,载荷就是要签的 32 字节.
pub struct RawHashWalker;

impl Decoder for RawHashWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let digest = B256::try_from(input.payload).map_err(|_| Reject::Malformed)?;
        let source = Source::Signed(Extent::Own(whole(input.payload).span));
        Ok(Some(Walked {
            root: Node::new("raw_hash", Value::Hash(digest.to_vec()), source).action("raw-hash"),
            round_trips: vec![RoundTrip {
                original: input.payload.to_vec(),
                rebuilt: digest.to_vec(),
            }],
        }))
    }
}

/// personal_sign,载荷是消息本身,前缀由签名方加上.
pub struct PersonalWalker;

impl Decoder for PersonalWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let message = own("message", message_value(input.payload), whole(input.payload).span);
        let digest = signing_hash(eip191_hash_message(input.payload));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{Decoded, Failure, Kind, decode, display};
    use crate::{Chain, Mode};
    use alloy_consensus::transaction::RlpEcdsaEncodableTx;
    use alloy_primitives::{Signature, address, b256};
    use bitcoin::secp256k1::SecretKey;

    const COIN: Coin = Coin {
        unit: "ETH",
        decimals: 18,
    };
    const WALKER: EvmWalker<'static> = EvmWalker {
        coin: Some(COIN),
        chain_id: Some(1),
        call: &[],
    };
    const PEER: Address = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    const KEY: B256 = b256!("0x1111111111111111111111111111111111111111111111111111111111111111");

    fn run(decoder: &impl Decoder, payload: &[u8]) -> Result<Decoded, Failure> {
        decode(
            decoder,
            &Input {
                chain: Chain::Evm,
                mode: Some(Mode::Mainnet),
                network: "ethereum",
                kind: 0,
                paths: &[],
                payload,
                signer: None,
            },
        )
    }

    fn evm(payload: &[u8]) -> Result<Decoded, Failure> {
        run(&WALKER, payload)
    }

    fn encoded(tx: impl Into<Tx>) -> Vec<u8> {
        tx.into().encoded_for_signing()
    }

    fn shown(decoded: &Decoded, name: &str) -> Vec<String> {
        let visits = decoded.visits();
        let rows = visits.iter().filter(|visit| visit.node.name == name);
        rows.map(|visit| display(&visit.node.value)).collect()
    }

    fn kinds(decoded: &Decoded) -> Vec<String> {
        decoded.actions().iter().map(|(_, _, kind)| kind.to_string()).collect()
    }

    fn has(decoded: &Decoded, kind: &str) -> bool {
        kinds(decoded).iter().any(|found| found == kind)
    }

    fn eip1559() -> TxEip1559 {
        TxEip1559 {
            chain_id: 1,
            nonce: 7,
            gas_limit: 21_000,
            max_fee_per_gas: 3_000_000_000,
            max_priority_fee_per_gas: 2_000_000_000,
            to: TxKind::Call(PEER),
            value: U256::from(1_500_000_000_000_000_000u64),
            access_list: AccessList::default(),
            input: Bytes::new(),
        }
    }

    fn legacy(chain_id: Option<u64>) -> TxLegacy {
        TxLegacy {
            chain_id,
            nonce: 3,
            gas_price: 3_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(PEER),
            value: U256::from(1),
            input: Bytes::new(),
        }
    }

    fn entries() -> AccessList {
        AccessList(vec![AccessListItem {
            address: PEER,
            storage_keys: vec![B256::repeat_byte(1), B256::repeat_byte(2)],
        }])
    }

    fn signed(chain_id: u64, delegate: Address) -> SignedAuthorization {
        let inner = Authorization {
            chain_id: U256::from(chain_id),
            address: delegate,
            nonce: 0,
        };
        let secp = Secp256k1::new();
        let key = SecretKey::from_slice(KEY.as_slice()).unwrap();
        let digest = Message::from_digest(inner.signature_hash().0);
        let (recovery, compact) = secp.sign_ecdsa_recoverable(&digest, &key).serialize_compact();
        let signature = Signature::from_bytes_and_parity(&compact, recovery.to_i32() == 1);
        inner.into_signed(signature)
    }

    fn eip7702(authorization_list: Vec<SignedAuthorization>) -> TxEip7702 {
        TxEip7702 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 50_000,
            max_fee_per_gas: 2,
            max_priority_fee_per_gas: 1,
            to: PEER,
            value: U256::ZERO,
            access_list: AccessList::default(),
            authorization_list,
            input: Bytes::new(),
        }
    }

    fn eip4844() -> TxEip4844 {
        TxEip4844 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: PEER,
            value: U256::ZERO,
            access_list: entries(),
            blob_versioned_hashes: vec![B256::repeat_byte(3), B256::repeat_byte(4)],
            max_fee_per_blob_gas: 5,
            input: Bytes::new(),
        }
    }

    #[test]
    fn every_type_passes_the_coverage_checks() {
        let eip2930 = TxEip2930 {
            chain_id: 1,
            nonce: 1,
            gas_price: 2,
            gas_limit: 21_000,
            to: TxKind::Call(PEER),
            value: U256::ZERO,
            access_list: entries(),
            input: Bytes::from_static(&[1, 2]),
        };
        let payloads = [
            encoded(legacy(None)),
            encoded(legacy(Some(1))),
            encoded(eip2930),
            encoded(eip1559()),
            encoded(eip4844()),
            encoded(eip7702(vec![signed(1, PEER)])),
        ];
        for payload in payloads {
            let decoded = evm(&payload).unwrap();
            assert_eq!(decoded.root().name, "transaction");
            assert_eq!(decoded.node_count() as usize, decoded.visits().len());
        }
    }

    #[test]
    fn fields_come_from_the_official_type_in_wire_order() {
        let decoded = evm(&encoded(eip1559())).unwrap();
        assert_eq!(shown(&decoded, "tx_type"), ["0x02 EIP-1559"]);
        assert_eq!(shown(&decoded, "max_priority_fee_per_gas"), ["2 Gwei"]);
        assert_eq!(shown(&decoded, "max_fee_per_gas"), ["3 Gwei"]);
        assert_eq!(shown(&decoded, "value"), ["1.5 ETH"]);
        assert_eq!(shown(&decoded, "to"), [PEER.to_string()]);
        assert_eq!(shown(&decoded, "fee_cap"), ["0.000063 ETH"]);
    }

    #[test]
    fn an_unknown_network_shows_raw_integers() {
        let walker = EvmWalker {
            coin: None,
            chain_id: None,
            call: &[],
        };
        let decoded = run(&walker, &encoded(eip1559())).unwrap();
        assert_eq!(shown(&decoded, "value"), ["1500000000000000000"]);
    }

    #[test]
    fn the_call_decides_the_action() {
        assert!(has(&evm(&encoded(eip1559())).unwrap(), "native-transfer"));
        let call = TxEip1559 {
            input: Bytes::from_static(&[1]),
            ..eip1559()
        };
        assert!(has(&evm(&encoded(call)).unwrap(), "call"));
        let create = TxEip1559 {
            to: TxKind::Create,
            ..eip1559()
        };
        let decoded = evm(&encoded(create)).unwrap();
        assert!(has(&decoded, "contract-creation"));
        assert_eq!(shown(&decoded, "to"), ["创建合约"]);
    }

    #[test]
    fn a_legacy_transaction_without_a_chain_id_says_so() {
        let decoded = evm(&encoded(legacy(None))).unwrap();
        assert!(has(&decoded, "no-chain-id"));
        assert_eq!(shown(&decoded, "chain_id"), ["无"]);
        let protected = evm(&encoded(legacy(Some(1)))).unwrap();
        assert!(!has(&protected, "no-chain-id"));
        assert_eq!(shown(&protected, "eip155_r"), ["0"]);
    }

    #[test]
    fn the_eip155_tail_must_be_zero() {
        let mut payload = encoded(legacy(Some(1)));
        let last = payload.len() - 1;
        payload[last] = 0x01;
        assert_eq!(evm(&payload).unwrap_err().reason, Reject::RoundTrip);
    }

    #[test]
    fn seven_legacy_items_are_refused() {
        let mut body = Vec::new();
        legacy(None).rlp_encode_fields(&mut body);
        1u8.encode(&mut body);
        let mut payload = Vec::new();
        let header = alloy_rlp::Header {
            list: true,
            payload_length: body.len(),
        };
        header.encode(&mut payload);
        payload.extend_from_slice(&body);
        assert!(evm(&payload).is_err());
    }

    #[test]
    fn access_lists_are_listed_in_full() {
        let decoded = evm(&encoded(TxEip1559 {
            access_list: entries(),
            ..eip1559()
        }))
        .unwrap();
        assert_eq!(shown(&decoded, "address"), [PEER.to_string()]);
        assert_eq!(shown(&decoded, "storage_key").len(), 2);
        assert_eq!(shown(&decoded, "storage_key")[1], "02".repeat(32));
    }

    #[test]
    fn blob_hashes_and_blob_fees_are_shown() {
        let decoded = evm(&encoded(eip4844())).unwrap();
        assert_eq!(shown(&decoded, "blob_versioned_hash").len(), 2);
        assert_eq!(shown(&decoded, "max_fee_per_blob_gas"), ["0.000000005 Gwei"]);
        assert!(has(&decoded, "tx-eip4844"));
        // 21000 × 10 加 2 × 131072 × 5.
        let cap = 210_000 + 2 * 131_072 * 5;
        assert_eq!(fee_cap(&eip4844().into()), U256::from(cap));
    }

    fn wrapped(ty: TxType, fields: &[u8]) -> Vec<u8> {
        let header = alloy_rlp::Header {
            list: true,
            payload_length: fields.len(),
        };
        let mut payload = vec![ty as u8];
        header.encode(&mut payload);
        payload.extend_from_slice(fields);
        payload
    }

    /// 类型 0x03,0x04 的 `to` 不能为空.
    #[test]
    fn blob_and_delegation_transactions_need_a_target() {
        let mut common = Vec::new();
        TxEip1559 {
            to: TxKind::Create,
            ..eip1559()
        }
        .rlp_encode_fields(&mut common);
        let mut blob = common.clone();
        5u8.encode(&mut blob);
        vec![B256::repeat_byte(3)].encode(&mut blob);
        assert!(evm(&wrapped(TxType::Eip4844, &blob)).is_err());
        let mut delegation = common;
        vec![signed(1, PEER)].encode(&mut delegation);
        assert!(evm(&wrapped(TxType::Eip7702, &delegation)).is_err());
    }

    #[test]
    fn authorizations_are_listed_with_the_recovered_authority() {
        let decoded = evm(&encoded(eip7702(vec![signed(1, PEER), signed(1, PEER)]))).unwrap();
        for name in ["y_parity", "r", "s", "nonce"] {
            assert!(!shown(&decoded, name).is_empty(), "{name}");
        }
        let key = SecretKey::from_slice(KEY.as_slice())
            .unwrap()
            .public_key(&Secp256k1::new());
        let uncompressed = key.serialize_uncompressed();
        let expected = Address::from_raw_public_key(uncompressed.split_first().unwrap().1);
        assert_eq!(
            shown(&decoded, "authority"),
            [expected.to_string(), expected.to_string()]
        );
        let plain = kinds(&decoded).iter().filter(|kind| *kind == "authorization").count();
        assert_eq!(plain, 2);
    }

    #[test]
    fn dangerous_authorizations_are_marked_not_refused() {
        let cases = [
            (signed(0, PEER), "authorization-every-chain"),
            (signed(137, PEER), "authorization-other-chain"),
            (signed(1, Address::ZERO), "authorization-clear"),
        ];
        for (authorization, kind) in cases {
            assert!(
                has(&evm(&encoded(eip7702(vec![authorization]))).unwrap(), kind),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_bad_authorization_signature_is_unrecovered_not_refused() {
        let good = signed(1, PEER);
        let bad = [
            SignedAuthorization::new_unchecked(good.inner().clone(), 2, good.r(), good.s()),
            SignedAuthorization::new_unchecked(good.inner().clone(), 0, good.r(), SECP256K1N_HALF + U256::from(1)),
            SignedAuthorization::new_unchecked(good.inner().clone(), 0, U256::ZERO, U256::ZERO),
        ];
        for authorization in bad {
            let decoded = evm(&encoded(eip7702(vec![authorization]))).unwrap();
            assert!(has(&decoded, "authorization-unrecovered"));
            assert_eq!(shown(&decoded, "authority"), ["恢复失败"]);
        }
    }

    #[test]
    fn a_delegation_needs_an_authorization() {
        assert!(evm(&encoded(eip7702(Vec::new()))).is_err());
    }

    #[test]
    fn trailing_bytes_are_refused() {
        for tx in [encoded(eip1559()), encoded(legacy(None))] {
            let mut payload = tx;
            payload.push(0);
            assert!(evm(&payload).is_err());
        }
    }

    #[test]
    fn unknown_type_bytes_are_opaque() {
        for first in [0x00, 0x06, 0x07, 0x7f, 0x80, 0xbf] {
            let decoded = evm(&[first, 1, 2]).unwrap();
            assert_eq!(decoded.root().value.kind(), Kind::Opaque, "{first:#x}");
        }
    }

    #[test]
    fn an_empty_payload_is_refused() {
        assert!(evm(&[]).is_err());
    }

    /// `0x19 0x01` 加两个哈希,keccak 就是 EIP-712 的签名原文.
    fn permit_digest() -> Vec<u8> {
        [&[0x19, 0x01][..], &[0xaa; 32], &[0xbb; 32]].concat()
    }

    #[test]
    fn an_eip712_digest_is_decoded_as_what_it_is() {
        let decoded = evm(&permit_digest()).unwrap();
        assert_eq!(decoded.root().name, "eip712_digest");
        assert_ne!(decoded.root().value.kind(), Kind::Opaque);
        assert_eq!(shown(&decoded, "domain_separator"), ["aa".repeat(32)]);
        assert_eq!(shown(&decoded, "hash_struct"), ["bb".repeat(32)]);
        let hash = keccak256(permit_digest()).to_vec();
        assert_eq!(decoded.visits().last().unwrap().node.value, Value::Hash(hash));
    }

    #[test]
    fn a_malformed_signature_domain_never_becomes_opaque() {
        let mut long = permit_digest();
        long.push(0);
        let short = &permit_digest()[..65];
        let malformed = [
            vec![0x19],
            vec![0x19, 0x01],
            short.to_vec(),
            long,
            vec![0x19, 0x45],
            vec![0x19, 0x00, 1],
            vec![0x05],
            vec![0x05, 0xc0],
            vec![0x05, 0x01],
        ];
        for payload in malformed {
            assert!(evm(&payload).is_err(), "{payload:02x?}");
        }
    }

    #[test]
    fn personal_messages_keep_their_declared_length() {
        let decoded = evm(&eip191_message("12345")).unwrap();
        assert_eq!(decoded.root().name, "personal_message");
        assert_eq!(shown(&decoded, "message"), ["12345"]);
        let empty = evm(&eip191_message("")).unwrap();
        assert_eq!(shown(&empty, "message"), [""]);
    }

    #[test]
    fn a_wrong_personal_length_is_refused() {
        let mut payload = eip191_message("hello");
        payload.push(b'!');
        assert!(evm(&payload).is_err());
        let padded = [EIP191_PREFIX.as_bytes(), b"05hello"].concat();
        assert!(evm(&padded).is_err());
        assert!(evm(b"\x19Ethereum Signed Mess").is_err());
    }

    #[test]
    fn binary_messages_are_bytes() {
        let decoded = evm(&eip191_message([0xff, 0xfe])).unwrap();
        assert_eq!(shown(&decoded, "message"), ["fffe"]);
    }

    #[test]
    fn version_zero_names_the_validator() {
        let payload = [&[0x19, 0x00][..], PEER.as_slice(), &[1, 2]].concat();
        let decoded = evm(&payload).unwrap();
        assert_eq!(shown(&decoded, "validator"), [PEER.to_string()]);
        assert_eq!(shown(&decoded, "data"), ["0102"]);
        assert!(has(&decoded, "eip191-validator"));
    }

    #[test]
    fn a_later_eip191_version_keeps_its_domain() {
        let decoded = evm(&[0x19, 0x54, 1, 2]).unwrap();
        assert_eq!(decoded.root().name, "eip191_message");
        assert_ne!(decoded.root().value.kind(), Kind::Opaque);
        assert!(has(&decoded, "eip191-unknown"));
    }

    #[test]
    fn an_authorization_message_is_decoded_as_what_it_is() {
        let inner = signed(137, PEER).strip_signature();
        let payload = [vec![MAGIC], alloy_rlp::encode(&inner)].concat();
        let decoded = evm(&payload).unwrap();
        assert_eq!(decoded.root().name, "eip7702_authorization");
        assert_eq!(shown(&decoded, "address"), [PEER.to_string()]);
        assert!(has(&decoded, "authorization-other-chain"));
        let hash = inner.signature_hash().to_vec();
        assert_eq!(decoded.visits().last().unwrap().node.value, Value::Hash(hash));
    }

    #[test]
    fn a_raw_hash_is_exactly_32_bytes() {
        let decoded = run(&RawHashWalker, &[7; 32]).unwrap();
        assert!(has(&decoded, "raw-hash"));
        assert_eq!(decoded.root().value.kind(), Kind::Hash);
        assert!(run(&RawHashWalker, &[7; 31]).is_err());
        assert!(run(&RawHashWalker, &[7; 33]).is_err());
    }

    #[test]
    fn personal_sign_hashes_with_the_prefix() {
        let decoded = run(&PersonalWalker, b"hello").unwrap();
        assert_eq!(shown(&decoded, "message"), ["hello"]);
        let expected = keccak256(eip191_message("hello")).to_vec();
        assert_eq!(decoded.visits().last().unwrap().node.value, Value::Hash(expected));
    }

    #[test]
    fn the_schema_order_is_checked_against_the_official_encoding() {
        let tx = eip1559();
        let list = encoded(tx.clone());
        let mut cursor = Cursor::new(&whole(&list));
        cursor.take(1).unwrap();
        let list = whole_list(&mut cursor).unwrap();
        let mut walk = Walk::new(None, &[], Vec::new());
        // 名字配错时,字段的编码和这一项的字节不同.
        let names = vec![
            "nonce",
            "chain_id",
            "max_priority_fee_per_gas",
            "max_fee_per_gas",
            "gas_limit",
            "to",
            "value",
            "data",
            "access_list",
        ];
        let mut fields = Named::with(&list, names).unwrap();
        walk.nonce(&mut fields, tx.nonce).unwrap();
        walk.chain_id(&mut fields, tx.chain_id).unwrap();
        assert!(walk.checks.iter().any(|check| check.original != check.rebuilt));
    }
}

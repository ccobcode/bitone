//! PSBT 键值遍历器.键的编号,名字,取值格式都在 assets/psbt-schema.json,这里只认容器格式:
//! 魔数,键值对,映射结束符.字段含义来自 `bitcoin`,`psbt-v2` 的类型.见 docs/10-tx-decoding.md 第 5.4 节.

use super::psbt_official::{self, unsigned_bytes};
use super::psbt_values::{Field, key_node, value_node};
use super::script::{ScriptClass, address_node, estimated_vsize, lock_time_node, output_action, script_type_node};
use super::script::{sequence_node, uint};
use super::{Decoder, Extent, Input, Node, Reject, RoundTrip, Source, Span, Value, Walked};
use crate::Mode;
use crate::hex::FromHex;
use bitcoin::consensus::encode::{Decodable, VarInt, deserialize, deserialize_partial, serialize};
use bitcoin::psbt::Psbt;
use bitcoin::{Amount, Sequence, TxOut};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// 第 5.4 节只认这两个 PSBT 版本,其他版本不知道怎么算 sighash,签名无法完成.
const V0: u32 = 0;
const V2: u32 = 2;

/// 键数据的格式.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum KeyCodec {
    None,
    Pubkey,
    Xpub,
    Xonly,
    XonlyLeaf,
    ControlBlock,
    Hash,
    Proprietary,
    Opaque,
}

/// 值的格式.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ValueCodec {
    UnsignedTx,
    PrevTx,
    TxOut,
    TxVersion,
    PsbtVersion,
    InputCount,
    OutputCount,
    Script,
    Witness,
    Sighash,
    Uint32,
    Sequence,
    Amount,
    Flags,
    Txid,
    TimeLock,
    HeightLock,
    KeySource,
    TapKeySource,
    EcdsaSignature,
    TapSignature,
    Xonly,
    Hash32,
    Preimage,
    LeafScript,
    TapTree,
    Utf8,
    DnssecProof,
    Opaque,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Origin {
    Signed,
    Context,
}

impl Origin {
    pub(super) fn own(self, span: Span) -> Source {
        match self {
            Self::Signed => Source::Signed(Extent::Own(span)),
            Self::Context => Source::Context(Extent::Own(span)),
        }
    }

    fn container(self) -> Source {
        match self {
            Self::Signed => Source::Signed(Extent::Container),
            Self::Context => Source::Context(Extent::Container),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    #[serde(rename = "type")]
    key_type: u64,
    pub(super) name: String,
    pub(super) key: KeyCodec,
    pub(super) value: ValueCodec,
    pub(super) source: Origin,
    /// 只允许出现在这个 PSBT 版本里.
    only: Option<u32>,
    /// 在 `only` 的版本里必须出现.
    #[serde(default)]
    required: bool,
    /// 这个键决定交易本身,官方解码器不把它放进键值表.
    #[serde(default)]
    structural: bool,
    /// 规则表的种类名.
    pub(super) action: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coin {
    unit: String,
    decimals: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    magic: String,
    coin: Coin,
    global: Vec<Entry>,
    input: Vec<Entry>,
    output: Vec<Entry>,
}

static SCHEMA: OnceLock<Schema> = OnceLock::new();

fn schema() -> &'static Schema {
    SCHEMA
        .get_or_init(|| serde_json::from_str(include_str!("../../assets/psbt-schema.json")).expect("内置 PSBT 表有效"))
}

fn magic() -> Vec<u8> {
    Vec::from_hex(&schema().magic).expect("内置 PSBT 表有效")
}

/// 原生币金额.单位和小数位是链本身的数据,不来自清单.
pub(super) fn coin(sats: u64) -> Value {
    let coin = &schema().coin;
    Value::Amount(super::Amount {
        raw: sats.to_be_bytes().to_vec(),
        decimals: Some(coin.decimals),
        unit: Some(coin.unit.clone()),
    })
}

/// 判断载荷是不是 PSBT.`SignRequest` 没有链字段时用它识别 Bitcoin 请求.
pub fn is_psbt(payload: &[u8]) -> bool {
    payload.starts_with(&magic())
}

/// 载荷的读头.范围都从它得到,不手算偏移.
pub(super) struct Cursor<'a> {
    rest: &'a [u8],
    offset: u32,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(field: &Field<'a>) -> Self {
        Self {
            rest: field.bytes,
            offset: field.span.offset,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub(super) fn take(&mut self, len: u64) -> Result<Field<'a>, Reject> {
        let count = usize::try_from(len).map_err(|_| Reject::Malformed)?;
        let (bytes, rest) = self.rest.split_at_checked(count).ok_or(Reject::Malformed)?;
        let span = Span::new(self.offset, count as u32);
        self.rest = rest;
        self.offset += count as u32;
        Ok(Field { bytes, span })
    }

    pub(super) fn rest(&mut self) -> Field<'a> {
        let span = Span::new(self.offset, self.rest.len() as u32);
        let bytes = std::mem::take(&mut self.rest);
        self.offset += span.len;
        Field { bytes, span }
    }

    /// 共识编码的一个值.`Decodable` 拒绝非规范的长度前缀.
    pub(super) fn read<T: Decodable>(&mut self) -> Result<T, Reject> {
        let (value, used) = deserialize_partial(self.rest).map_err(|_| Reject::Malformed)?;
        self.take(used as u64)?;
        Ok(value)
    }

    fn compact(&mut self) -> Result<u64, Reject> {
        self.read::<VarInt>().map(|VarInt(value)| value)
    }
}

/// 一条键值对.
pub(super) struct Record<'a> {
    entry: Option<&'static Entry>,
    key_type: u64,
    /// 键的原始字节,含长度前缀.只用来重新排列后交给官方解码器.
    raw_key: &'a [u8],
    key: Field<'a>,
    value: Field<'a>,
    framing: Vec<Span>,
}

impl<'a> Record<'a> {
    fn structural(&self) -> bool {
        self.entry.is_some_and(|entry| entry.structural)
    }

    pub(super) fn has_codec(&self, codec: ValueCodec) -> bool {
        self.entry.is_some_and(|entry| entry.value == codec)
    }

    pub(super) fn raw_key(&self) -> &'a [u8] {
        self.raw_key
    }

    pub(super) fn value_bytes(&self) -> &'a [u8] {
        self.value.bytes
    }

    pub(super) fn sort_key(&self) -> (u64, &'a [u8]) {
        (self.key_type, self.key.bytes)
    }
}

/// 一个键值映射.
pub(super) struct Map<'a> {
    pub(super) records: Vec<Record<'a>>,
    end: Span,
}

impl<'a> Map<'a> {
    pub(super) fn value_of(&self, codec: ValueCodec) -> Option<&'a [u8]> {
        self.records
            .iter()
            .find(|record| record.has_codec(codec))
            .map(Record::value_bytes)
    }

    /// 官方解码器放进键值表的条目数.
    pub(super) fn entries(&self) -> usize {
        self.records.iter().filter(|record| !record.structural()).count()
    }
}

fn walk_map<'a>(cursor: &mut Cursor<'a>, entries: &'static [Entry]) -> Result<Map<'a>, Reject> {
    let mut records = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let (start, before) = (cursor.offset, cursor.rest);
        let key_len = cursor.compact()?;
        if key_len == 0 {
            let end = Span::new(start, cursor.offset - start);
            return Ok(Map { records, end });
        }
        let key_bytes = cursor.take(key_len)?;
        let raw_key = before
            .get(..(cursor.offset - start) as usize)
            .ok_or(Reject::Malformed)?;
        let mut inner = Cursor::new(&key_bytes);
        let key_type = inner.compact()?;
        let key = inner.rest();
        if !seen.insert((key_type, key.bytes)) {
            return Err(Reject::Malformed);
        }
        let head = Span::new(start, key.span.offset - start);
        let length_start = cursor.offset;
        let value_len = cursor.compact()?;
        let length = Span::new(length_start, cursor.offset - length_start);
        let value = cursor.take(value_len)?;
        records.push(Record {
            entry: entries.iter().find(|entry| entry.key_type == key_type),
            key_type,
            raw_key,
            key,
            value,
            framing: vec![head, length],
        });
    }
}

/// BIP-370 的版本规则:每个版本有必须出现和必须排除的键.
fn check_versions(map: &Map<'_>, entries: &[Entry], version: u32) -> Result<(), Reject> {
    let excluded = map.records.iter().any(|record| {
        record
            .entry
            .is_some_and(|entry| entry.only.is_some_and(|only| only != version))
    });
    let missing = entries.iter().any(|entry| {
        entry.required
            && entry.only == Some(version)
            && !map.records.iter().any(|record| record.key_type == entry.key_type)
    });
    if excluded || missing {
        return Err(Reject::Malformed);
    }
    Ok(())
}

/// 读完的 PSBT.`psbt` 是官方解码的结果,v2 也规整成同一个类型,核对,签名,核验只看它.
pub struct Parsed<'a> {
    pub psbt: Psbt,
    version: u32,
    magic: Span,
    global: Map<'a>,
    inputs: Vec<Map<'a>>,
    outputs: Vec<Map<'a>>,
}

fn walk_maps<'a>(cursor: &mut Cursor<'a>, entries: &'static [Entry], count: u64) -> Result<Vec<Map<'a>>, Reject> {
    (0..count).map(|_| walk_map(cursor, entries)).collect()
}

fn count_of(global: &Map<'_>, codec: ValueCodec) -> Result<u64, Reject> {
    let bytes = global.value_of(codec).ok_or(Reject::Malformed)?;
    deserialize::<VarInt>(bytes)
        .map(|VarInt(count)| count)
        .map_err(|_| Reject::Malformed)
}

/// 解析 PSBT.不是 PSBT 时返回 `None`,字节不合规时返回错误.
pub fn parse(payload: &[u8]) -> Result<Option<Parsed<'_>>, Reject> {
    let schema = schema();
    let magic = self::magic();
    if !payload.starts_with(&magic) {
        return Ok(None);
    }
    let whole = Field {
        bytes: payload,
        span: Span::new(0, u32::try_from(payload.len()).map_err(|_| Reject::Malformed)?),
    };
    let mut cursor = Cursor::new(&whole);
    let head = cursor.take(magic.len() as u64)?;
    let global = walk_map(&mut cursor, &schema.global)?;
    let version = match global.value_of(ValueCodec::PsbtVersion) {
        Some(bytes) => deserialize::<u32>(bytes).map_err(|_| Reject::Malformed)?,
        None => V0,
    };
    check_versions(&global, &schema.global, version)?;
    let (psbt, inputs, outputs) = match version {
        V0 => {
            let psbt = Psbt::deserialize(payload).map_err(|_| Reject::Malformed)?;
            let inputs = walk_maps(&mut cursor, &schema.input, psbt.unsigned_tx.input.len() as u64)?;
            let outputs = walk_maps(&mut cursor, &schema.output, psbt.unsigned_tx.output.len() as u64)?;
            (psbt, inputs, outputs)
        }
        V2 => {
            let inputs = walk_maps(&mut cursor, &schema.input, count_of(&global, ValueCodec::InputCount)?)?;
            let outputs = walk_maps(&mut cursor, &schema.output, count_of(&global, ValueCodec::OutputCount)?)?;
            let psbt = psbt_official::decode_v2(head.bytes, &global, &inputs, &outputs)?;
            (psbt, inputs, outputs)
        }
        _other => return Err(Reject::Malformed),
    };
    if !cursor.is_empty() {
        return Err(Reject::Gap {
            offset: u64::from(cursor.offset),
        });
    }
    for map in &inputs {
        check_versions(map, &schema.input, version)?;
    }
    for map in &outputs {
        check_versions(map, &schema.output, version)?;
    }
    let parsed = Parsed {
        psbt,
        version,
        magic: head.span,
        global,
        inputs,
        outputs,
    };
    psbt_official::agrees(&parsed.psbt, &parsed.global, &parsed.inputs, &parsed.outputs)?;
    Ok(Some(parsed))
}

/// 总量上限由 `prepare` 检查.
fn sum(mut values: impl Iterator<Item = Amount>) -> Option<Amount> {
    values.try_fold(Amount::ZERO, Amount::checked_add)
}

impl Parsed<'_> {
    /// 第 `index` 个输入花掉的输出.前序交易和输入是否相符由 `prepare` 判定.
    fn spent(&self, index: usize) -> Option<TxOut> {
        let txin = self.psbt.unsigned_tx.input.get(index)?;
        let input = self.psbt.inputs.get(index)?;
        let declared = input.witness_utxo.clone();
        let prev = input.non_witness_utxo.as_ref();
        prev.and_then(|tx| tx.output.get(txin.previous_output.vout as usize).cloned())
            .or(declared)
    }

    /// 手续费和费率.有输入没有金额,或输出超过输入时没有这两行,后者由 `prepare` 拒绝.
    fn fee_nodes(&self) -> Vec<Node> {
        let tx = &self.psbt.unsigned_tx;
        let Some(spent) = (0..tx.input.len())
            .map(|index| self.spent(index))
            .collect::<Option<Vec<_>>>()
        else {
            return Vec::new();
        };
        let paid = sum(spent.iter().map(|out| out.value));
        let created = sum(tx.output.iter().map(|out| out.value));
        let Some(fee) = paid.zip(created).and_then(|(paid, created)| paid.checked_sub(created)) else {
            return Vec::new();
        };
        let vsize = estimated_vsize(tx, spent.iter().map(|out| out.script_pubkey.as_script()));
        let rate = u128::from(fee.to_sat()) * 100 / u128::from(vsize);
        vec![
            Node::new("fee", coin(fee.to_sat()), Source::Derived),
            Node::new(
                "fee_rate",
                Value::Amount(super::Amount {
                    raw: rate.to_be_bytes().to_vec(),
                    decimals: Some(2),
                    unit: Some("sat/vB".to_string()),
                }),
                Source::Derived,
            ),
        ]
    }

    fn record_node(&self, record: &Record<'_>, slot: Slot, mode: Option<Mode>) -> Result<Node, Reject> {
        let Some(entry) = record.entry else {
            return Ok(self.unknown_node(record));
        };
        let tx = &self.psbt.unsigned_tx;
        if entry.key == KeyCodec::None && !record.key.bytes.is_empty() {
            return Err(Reject::Malformed);
        }
        let spent_vout = slot
            .input()
            .and_then(|index| tx.input.get(index))
            .map(|txin| txin.previous_output.vout);
        let value = value_node(entry, &record.value, tx, spent_vout, mode)?;
        let node = if entry.key == KeyCodec::None {
            value
        } else {
            let key = key_node(entry.key, &record.key)?;
            Node::new(entry.name.as_str(), Value::Struct, entry.source.container()).children(vec![key, value])
        };
        // 标志位只有非 0 才是动作,取值时判断.
        let action = entry.action.as_deref().filter(|_| entry.value != ValueCodec::Flags);
        let node = node.framed(record.framing.clone());
        Ok(match action {
            Some(action) => node.action(action),
            None => node,
        })
    }

    fn unknown_node(&self, record: &Record<'_>) -> Node {
        let key = Node::new(
            "key",
            Value::Opaque(record.key.bytes.to_vec()),
            Origin::Context.own(record.key.span),
        );
        let value = Node::new(
            "value",
            Value::Opaque(record.value.bytes.to_vec()),
            Origin::Context.own(record.value.span),
        );
        let key_type = Node::new("key_type", uint(record.key_type), Source::Derived);
        Node::new("unknown_key", Value::Struct, Origin::Context.container())
            .framed(record.framing.clone())
            .action("psbt-key-unknown")
            .children(vec![key_type, key, value])
    }

    fn map_nodes(&self, map: &Map<'_>, slot: Slot, mode: Option<Mode>) -> Result<Vec<Node>, Reject> {
        map.records
            .iter()
            .map(|record| self.record_node(record, slot, mode))
            .collect()
    }

    /// v2 没有交易字节,这里补上由键算出的字段.
    fn global_extra(&self) -> Vec<Node> {
        let tx = &self.psbt.unsigned_tx;
        if self.version != V2 {
            return Vec::new();
        }
        let all_final = tx.input.iter().all(|txin| txin.sequence == Sequence::MAX);
        vec![lock_time_node(tx.lock_time, all_final, true, Source::Derived)]
    }

    fn input_node(&self, index: usize, map: &Map<'_>, mode: Option<Mode>) -> Result<Node, Reject> {
        let mut rows = self.map_nodes(map, Slot::Input(index), mode)?;
        let node = Node::new("psbt_input", Value::Struct, Origin::Signed.container()).framed(vec![map.end]);
        if self.version != V2 {
            return Ok(node.children(rows));
        }
        if map.value_of(ValueCodec::Sequence).is_none() {
            let version = self.psbt.unsigned_tx.version;
            rows.push(sequence_node(Sequence::MAX, version, true, Source::Derived));
        }
        Ok(node.action("input").children(rows))
    }

    fn output_node(&self, index: usize, map: &Map<'_>, mode: Option<Mode>) -> Result<Node, Reject> {
        let mut rows = self.map_nodes(map, Slot::Output, mode)?;
        let node = Node::new("psbt_output", Value::Struct, Origin::Signed.container()).framed(vec![map.end]);
        if self.version != V2 {
            return Ok(node.children(rows));
        }
        let output = self.psbt.unsigned_tx.output.get(index).ok_or(Reject::Malformed)?;
        let class = ScriptClass::of(&output.script_pubkey);
        rows.extend(address_node(&output.script_pubkey, mode));
        rows.push(script_type_node(&output.script_pubkey));
        Ok(node.action(output_action(class, output.value.to_sat())).children(rows))
    }

    fn tree(&self, mode: Option<Mode>) -> Result<Node, Reject> {
        let mut global = self.map_nodes(&self.global, Slot::Global, mode)?;
        global.extend(self.global_extra());
        let inputs = self
            .inputs
            .iter()
            .enumerate()
            .map(|(index, map)| self.input_node(index, map, mode))
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = self
            .outputs
            .iter()
            .enumerate()
            .map(|(index, map)| self.output_node(index, map, mode))
            .collect::<Result<Vec<_>, _>>()?;
        let group = Origin::Signed.container();
        let mut rows = self.fee_nodes();
        rows.push(
            Node::new("global", Value::Struct, group)
                .framed(vec![self.global.end])
                .children(global),
        );
        rows.push(Node::new("psbt_inputs", Value::List, group).children(inputs));
        rows.push(Node::new("psbt_outputs", Value::List, group).children(outputs));
        Ok(Node::new("psbt", Value::Struct, group)
            .framed(vec![self.magic])
            .children(rows))
    }

    /// C2.v0 比较未签名交易的共识编码,v2 没有这段字节,比较官方解码器和遍历器各自拼出的交易.
    fn round_trips(&self) -> Result<Vec<RoundTrip>, Reject> {
        let tx = &self.psbt.unsigned_tx;
        let original = match self.global.value_of(ValueCodec::UnsignedTx) {
            Some(bytes) => bytes.to_vec(),
            None => unsigned_bytes(&psbt_official::transaction_from_records(
                &self.global,
                &self.inputs,
                &self.outputs,
                tx.lock_time,
            )?),
        };
        let mut checks = vec![RoundTrip {
            original,
            rebuilt: unsigned_bytes(tx),
        }];
        for (map, input) in self.inputs.iter().zip(&self.psbt.inputs) {
            if let (Some(original), Some(prev)) = (map.value_of(ValueCodec::PrevTx), &input.non_witness_utxo) {
                checks.push(RoundTrip {
                    original: original.to_vec(),
                    rebuilt: serialize(prev),
                });
            }
            if let (Some(original), Some(out)) = (map.value_of(ValueCodec::TxOut), &input.witness_utxo) {
                checks.push(RoundTrip {
                    original: original.to_vec(),
                    rebuilt: serialize(out),
                });
            }
        }
        Ok(checks)
    }
}

/// 记录所在的映射.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Global,
    Input(usize),
    Output,
}

impl Slot {
    fn input(self) -> Option<usize> {
        match self {
            Self::Input(index) => Some(index),
            Self::Global | Self::Output => None,
        }
    }
}

/// PSBT 的解码器.
pub struct PsbtWalker;

impl Decoder for PsbtWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        let Some(parsed) = parse(input.payload)? else {
            return Ok(None);
        };
        Ok(Some(Walked {
            root: parsed.tree(input.mode)?,
            round_trips: parsed.round_trips()?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Decoded, Failure, Kind, decode};
    use super::*;
    use crate::Chain;
    use bitcoin::consensus::encode::serialize;
    use bitcoin::hashes::Hash;
    use bitcoin::script::{Builder, PushBytesBuf};
    use bitcoin::{OutPoint, ScriptBuf, Transaction, TxIn, Txid, Witness, absolute, opcodes, transaction};

    const UNSIGNED_TX: u8 = 0;
    const GLOBAL_VERSION: u8 = 0xfb;
    const TX_VERSION: u8 = 2;
    const INPUT_COUNT: u8 = 4;
    const OUTPUT_COUNT: u8 = 5;
    const SIGHASH_TYPE: u8 = 3;
    const REDEEM_SCRIPT: u8 = 4;
    const PREVIOUS_TXID: u8 = 0x0e;
    const OUTPUT_INDEX: u8 = 0x0f;
    const OUT_AMOUNT: u8 = 3;
    const OUT_SCRIPT: u8 = 4;

    fn record(key_type: u8, value: &[u8]) -> Vec<u8> {
        keyed(key_type, &[], value)
    }

    fn keyed(key_type: u8, key: &[u8], value: &[u8]) -> Vec<u8> {
        let mut bytes = serialize(&[&[key_type], key].concat());
        bytes.extend(serialize(&value.to_vec()));
        bytes
    }

    fn psbt(maps: &[&[Vec<u8>]]) -> Vec<u8> {
        let mut bytes = magic();
        for map in maps {
            bytes.extend(map.concat());
            bytes.push(0);
        }
        bytes
    }

    fn output(script: ScriptBuf, sats: u64) -> TxOut {
        TxOut {
            value: Amount::from_sat(sats),
            script_pubkey: script,
        }
    }

    fn unsigned(sequence: Sequence, outputs: Vec<TxOut>) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([7; 32]),
                    vout: 1,
                },
                script_sig: ScriptBuf::new(),
                sequence,
                witness: Witness::new(),
            }],
            output: outputs,
        }
    }

    fn v0(tx: &Transaction) -> Vec<u8> {
        let empty: &[Vec<u8>] = &[];
        let global = [record(UNSIGNED_TX, &unsigned_bytes(tx))];
        let mut maps: Vec<&[Vec<u8>]> = vec![&global, empty];
        maps.extend(tx.output.iter().map(|_| empty));
        psbt(&maps)
    }

    fn v2_global() -> Vec<Vec<u8>> {
        vec![
            record(GLOBAL_VERSION, &serialize(&2u32)),
            record(TX_VERSION, &serialize(&2i32)),
            record(INPUT_COUNT, &[1]),
            record(OUTPUT_COUNT, &[1]),
        ]
    }

    fn v2_input() -> Vec<Vec<u8>> {
        vec![record(PREVIOUS_TXID, &[7; 32]), record(OUTPUT_INDEX, &serialize(&1u32))]
    }

    fn v2_output(sats: u64, script: &[u8]) -> Vec<Vec<u8>> {
        vec![record(OUT_AMOUNT, &serialize(&sats)), record(OUT_SCRIPT, script)]
    }

    fn run(payload: &[u8]) -> Result<Decoded, Failure> {
        decode(
            &PsbtWalker,
            &Input {
                chain: Chain::Bitcoin,
                mode: Some(Mode::Mainnet),
                network: "bitcoin",
                kind: 0,
                paths: &[],
                payload,
            },
        )
    }

    fn values(decoded: &Decoded, name: &str) -> Vec<String> {
        let visits = decoded.visits();
        let named = visits.iter().filter(|visit| visit.node.name == name);
        named.map(|visit| super::super::display(&visit.node.value)).collect()
    }

    fn actions(decoded: &Decoded) -> Vec<String> {
        decoded.actions().iter().map(|action| action.2.to_string()).collect()
    }

    fn simple() -> Transaction {
        unsigned(
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            vec![output(ScriptBuf::new_p2wsh(&Hash::all_zeros()), 5000)],
        )
    }

    #[test]
    fn a_v0_psbt_covers_every_byte() {
        let decoded = run(&v0(&simple())).unwrap();
        assert_eq!(decoded.root().name, "psbt");
        assert_eq!(decoded.node_count() as usize, decoded.visits().len());
        assert!(decoded.check_rendered(decoded.visits().len()).is_ok());
        assert_eq!(values(&decoded, "version"), ["2"]);
        assert_eq!(values(&decoded, "sequence"), ["4294967293"]);
    }

    #[test]
    fn trailing_bytes_are_a_gap() {
        let mut payload = v0(&simple());
        payload.push(0);
        let offset = payload.len() as u64 - 1;
        assert_eq!(run(&payload).unwrap_err().reason, Reject::Gap { offset });
    }

    #[test]
    fn a_payload_that_is_not_a_psbt_is_opaque() {
        let decoded = run(&[1, 2, 3]).unwrap();
        assert_eq!(decoded.root().value, Value::Opaque(vec![1, 2, 3]));
    }

    #[test]
    fn a_truncated_map_is_refused() {
        let payload = v0(&simple());
        assert_eq!(
            run(&payload[..payload.len() - 1]).unwrap_err().reason,
            Reject::Malformed
        );
    }

    #[test]
    fn key_order_inside_a_map_is_free() {
        let tx = simple();
        let global = [keyed(0xfc, &[0, 0], &[9]), record(UNSIGNED_TX, &serialize(&tx))];
        let unordered = [record(REDEEM_SCRIPT, &[0x51]), record(SIGHASH_TYPE, &serialize(&1u32))];
        let payload = psbt(&[&global, &unordered, &[]]);
        assert_eq!(values(&run(&payload).unwrap(), "sighash_type"), ["SIGHASH_ALL"]);
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let tx = simple();
        let global = [record(UNSIGNED_TX, &serialize(&tx))];
        let unknown = keyed(0x70, &[1], &[2]);
        let payload = psbt(&[&global, &[unknown.clone(), unknown], &[]]);
        assert_eq!(run(&payload).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn a_length_that_is_not_minimal_is_refused() {
        let tx = simple();
        let global = [record(UNSIGNED_TX, &serialize(&tx))];
        // 键长 2 写成三字节的形式.
        let stretched = [0xfd, 2, 0, 0x70, 1, 1, 2].to_vec();
        let payload = psbt(&[&global, &[stretched], &[]]);
        assert_eq!(run(&payload).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn a_key_with_data_where_none_is_allowed_is_refused() {
        let tx = simple();
        let global = [record(UNSIGNED_TX, &serialize(&tx))];
        let payload = psbt(&[&global, &[keyed(SIGHASH_TYPE, &[1], &serialize(&1u32))], &[]]);
        assert_eq!(run(&payload).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn an_unknown_key_is_opaque_context_with_an_action() {
        let tx = simple();
        let global = [record(UNSIGNED_TX, &serialize(&tx))];
        let payload = psbt(&[&global, &[keyed(0x70, &[1], &[2])], &[]]);
        let decoded = run(&payload).unwrap();
        assert!(actions(&decoded).contains(&"psbt-key-unknown".to_string()));
        let visits = decoded.visits();
        let row = visits.iter().find(|visit| visit.node.name == "value").unwrap();
        assert_eq!(row.node.value.kind(), Kind::Opaque);
        assert_eq!(row.node.source.kind(), super::super::SourceKind::Context);
    }

    #[test]
    fn an_unimplemented_protocol_is_opaque_with_an_action() {
        let tx = simple();
        let global = [record(UNSIGNED_TX, &serialize(&tx))];
        let musig = keyed(0x1b, &[3; 66], &[4; 66]);
        let decoded = run(&psbt(&[&global, &[musig], &[]])).unwrap();
        assert!(actions(&decoded).contains(&"psbt-key-unimplemented".to_string()));
    }

    #[test]
    fn other_psbt_versions_are_refused() {
        let tx = simple();
        let global = [
            record(UNSIGNED_TX, &serialize(&tx)),
            record(GLOBAL_VERSION, &serialize(&1u32)),
        ];
        assert_eq!(run(&psbt(&[&global, &[], &[]])).unwrap_err().reason, Reject::Malformed);
    }

    #[test]
    fn v2_takes_the_transaction_from_the_maps() {
        let script = ScriptBuf::new_p2wsh(&Hash::all_zeros());
        let payload = psbt(&[&v2_global(), &v2_input(), &v2_output(5000, script.as_bytes())]);
        let decoded = run(&payload).unwrap();
        assert_eq!(values(&decoded, "output_index"), ["1"]);
        assert_eq!(values(&decoded, "lock_time"), ["0"]);
        assert!(actions(&decoded).contains(&"output-p2wsh".to_string()));
    }

    #[test]
    fn v2_without_a_sequence_is_final() {
        let payload = psbt(&[&v2_global(), &v2_input(), &v2_output(5000, &[0x51])]);
        let tx = parse(&payload).unwrap().unwrap().psbt.unsigned_tx;
        assert_eq!(tx.input[0].sequence, Sequence::MAX);
        assert_eq!(values(&run(&payload).unwrap(), "sequence"), ["4294967295"]);
    }

    #[test]
    fn v2_key_order_is_free_even_for_the_counts() {
        let mut global = v2_global();
        global.reverse();
        let payload = psbt(&[&global, &v2_input(), &v2_output(5000, &[0x51])]);
        assert!(run(&payload).is_ok());
    }

    #[test]
    fn v2_keeps_a_zero_amount_op_return_and_an_empty_script() {
        let burn = ScriptBuf::new_op_return(b"note");
        for script in [burn.as_bytes(), &[]] {
            let payload = psbt(&[&v2_global(), &v2_input(), &v2_output(0, script)]);
            let parsed = parse(&payload).unwrap().unwrap();
            assert_eq!(
                parsed.psbt.unsigned_tx.output[0],
                output(ScriptBuf::from_bytes(script.to_vec()), 0)
            );
            assert!(run(&payload).is_ok());
        }
    }

    #[test]
    fn v2_counts_above_the_maps_are_refused() {
        let mut global = v2_global();
        global[2] = record(INPUT_COUNT, &[9]);
        let payload = psbt(&[&global, &v2_input(), &v2_output(5000, &[0x51])]);
        assert_eq!(run(&payload).unwrap_err().reason, Reject::Malformed);
    }

    fn action_of(script: ScriptBuf, sats: u64) -> String {
        output_action(ScriptClass::of(&script), sats)
    }

    fn witness(version: opcodes::Opcode, program: &[u8]) -> ScriptBuf {
        let pushed = PushBytesBuf::try_from(program.to_vec()).unwrap();
        Builder::new().push_opcode(version).push_slice(pushed).into_script()
    }

    #[test]
    fn witness_versions_two_to_sixteen_are_future_programs() {
        for opcode in opcodes::all::OP_PUSHNUM_2.to_u8()..=opcodes::all::OP_PUSHNUM_16.to_u8() {
            let script = witness(opcodes::Opcode::from(opcode), &[1; 32]);
            assert_eq!(ScriptClass::of(&script), ScriptClass::FutureWitness, "{opcode}");
        }
    }

    #[test]
    fn a_future_program_shows_its_version_and_program_as_opcodes() {
        let script = witness(opcodes::all::OP_PUSHNUM_2, &[1; 32]);
        let decoded = run(&v0(&unsigned(Sequence::MAX, vec![output(script, 5000)]))).unwrap();
        assert_eq!(values(&decoded, "witness_version"), ["2"]);
        assert_eq!(values(&decoded, "witness_program"), ["01".repeat(32)]);
        assert!(values(&decoded, "address").is_empty());
        assert!(values(&decoded, "script")[0].starts_with("OP_PUSHNUM_2"));
    }

    #[test]
    fn bad_witness_lengths_and_other_scripts_are_unknown() {
        let v0_short = witness(opcodes::all::OP_PUSHBYTES_0, &[1; 19]);
        let v1_short = witness(opcodes::all::OP_PUSHNUM_1, &[1; 31]);
        let junk = ScriptBuf::from_bytes(vec![opcodes::all::OP_NOP.to_u8()]);
        for script in [v0_short, v1_short, junk, ScriptBuf::new()] {
            assert_eq!(ScriptClass::of(&script), ScriptClass::Other, "{script:?}");
        }
    }

    #[test]
    fn scripts_without_an_address_are_classified() {
        let key = PublicKeyBytes::compressed();
        assert_eq!(action_of(key.p2pk(), 1), "output-p2pk");
        assert_eq!(action_of(key.multisig(), 1), "output-bare-multisig");
        assert_eq!(
            action_of(witness(opcodes::all::OP_PUSHNUM_1, &[0x4e, 0x73]), 1),
            "output-p2a"
        );
    }

    /// `Script::is_multisig` 数到第 256 个推入时溢出,解码器不能 panic.
    #[test]
    fn a_long_list_of_pushes_is_not_a_multisig() {
        let mut bytes = vec![opcodes::all::OP_PUSHNUM_1.to_u8()];
        bytes.extend(std::iter::repeat_n(opcodes::all::OP_PUSHBYTES_0.to_u8(), 300));
        assert_eq!(ScriptClass::of(&ScriptBuf::from_bytes(bytes)), ScriptClass::Other);
    }

    #[test]
    fn op_return_shows_text_and_hex_and_hex_alone_for_other_bytes() {
        let text = ScriptBuf::new_op_return(b"hello");
        let decoded = run(&v0(&unsigned(Sequence::MAX, vec![output(text, 0)]))).unwrap();
        assert_eq!(values(&decoded, "text"), ["hello"]);
        assert_eq!(values(&decoded, "data"), ["68656c6c6f"]);
        let bytes = ScriptBuf::new_op_return([0xff, 0xfe]);
        let decoded = run(&v0(&unsigned(Sequence::MAX, vec![output(bytes, 0)]))).unwrap();
        assert!(values(&decoded, "text").is_empty());
        assert_eq!(values(&decoded, "data"), ["fffe"]);
    }

    #[test]
    fn an_op_return_with_money_is_a_burn() {
        assert_eq!(action_of(ScriptBuf::new_op_return(b"x"), 0), "output-op-return");
        assert_eq!(action_of(ScriptBuf::new_op_return(b"x"), 1), "output-op-return-burn");
    }

    #[test]
    fn sequence_meaning_follows_bip68_and_bip125() {
        let cases = [
            (Sequence::MAX, 2, "sequence-final", None),
            (Sequence::ENABLE_LOCKTIME_NO_RBF, 2, "sequence-no-rbf", None),
            (Sequence::ENABLE_RBF_NO_LOCKTIME, 2, "sequence-rbf", None),
            (
                Sequence::from_height(144),
                2,
                "sequence-rbf",
                Some("relative_lock_blocks"),
            ),
            (
                Sequence::from_512_second_intervals(3),
                2,
                "sequence-rbf",
                Some("relative_lock_intervals"),
            ),
            (Sequence::from_height(144), 1, "sequence-rbf", None),
        ];
        for (sequence, version, action, detail) in cases {
            let source = Source::Signed(Extent::Inherited);
            let node = sequence_node(sequence, transaction::Version(version), true, source);
            assert_eq!(node.action.as_deref(), Some(action), "{sequence}");
            let names: Vec<_> = node.children.iter().map(|child| child.name.as_str()).collect();
            assert_eq!(names, detail.into_iter().collect::<Vec<_>>(), "{sequence}");
        }
    }

    #[test]
    fn lock_time_meaning_follows_the_threshold() {
        let with = |lock_time: u32, sequence| {
            let mut tx = unsigned(sequence, vec![output(ScriptBuf::new_p2wsh(&Hash::all_zeros()), 1)]);
            tx.lock_time = absolute::LockTime::from_consensus(lock_time);
            run(&v0(&tx)).unwrap()
        };
        let rbf = Sequence::ENABLE_RBF_NO_LOCKTIME;
        assert!(actions(&with(0, rbf)).contains(&"lock-time-none".to_string()));
        let height = with(499_999_999, rbf);
        assert!(actions(&height).contains(&"lock-time-height".to_string()));
        assert_eq!(values(&height, "lock_time_height"), ["499999999"]);
        let time = with(500_000_000, rbf);
        assert!(actions(&time).contains(&"lock-time-utc".to_string()));
        assert_eq!(values(&time, "lock_time_utc"), ["1985-11-05T00:53:20Z"]);
        assert!(actions(&with(7, Sequence::MAX)).contains(&"lock-time-inactive".to_string()));
    }

    #[test]
    fn the_schema_names_are_unique_within_each_map() {
        for map in [&schema().global, &schema().input, &schema().output] {
            let names: BTreeSet<_> = map.iter().map(|entry| entry.name.as_str()).collect();
            let types: BTreeSet<_> = map.iter().map(|entry| entry.key_type).collect();
            assert_eq!((names.len(), types.len()), (map.len(), map.len()));
        }
    }

    #[test]
    fn every_schema_name_has_a_label() {
        let labels: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("../../assets/field-labels.json")).unwrap();
        for map in [&schema().global, &schema().input, &schema().output] {
            for entry in map {
                assert!(labels.contains_key(&entry.name), "{}", entry.name);
            }
        }
    }

    struct PublicKeyBytes(bitcoin::PublicKey);

    impl PublicKeyBytes {
        fn compressed() -> Self {
            let secp = bitcoin::secp256k1::Secp256k1::new();
            let secret = bitcoin::secp256k1::SecretKey::from_slice(&[7; 32]).unwrap();
            Self(bitcoin::PublicKey::new(secret.public_key(&secp)))
        }

        fn p2pk(&self) -> ScriptBuf {
            ScriptBuf::new_p2pk(&self.0)
        }

        fn multisig(&self) -> ScriptBuf {
            Builder::new()
                .push_opcode(opcodes::all::OP_PUSHNUM_1)
                .push_key(&self.0)
                .push_opcode(opcodes::all::OP_PUSHNUM_1)
                .push_opcode(opcodes::all::OP_CHECKMULTISIG)
                .into_script()
        }
    }
}

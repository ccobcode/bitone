//! Solana 解码.这里只认消息布局,指令的字段来自官方 Codama IDL,见 `idl`,没有 IDL 的原生程序见 `native`.
//! 本机不认识的版本,程序和指令都可以签,`\xffsolana offchain` 是已知的签名域,解不开时拒绝.

#![deny(clippy::wildcard_enum_match_arm, clippy::wildcard_in_or_patterns)]

use super::idl::{self as solana_idl, Account, Instruction as IdlInstruction, Program, TypeWalk};
use super::native::Native;
use crate::decode::{
    Amount, Coin, Cursor, Decoder, Extent, Field, Input, Node, Reject, RoundTrip, Source, Span, Value, Walked,
};
use bitcoin::base58;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const KEY_BYTES: u64 = 32;
const HEADER_BYTES: usize = 3;
const COMPACT_SHIFTS: [u32; 3] = [0, 7, 14];
const COMPACT_MORE: u8 = 0x80;
const COMPACT_VALUE: u8 = 0x7f;
const MAX_ACCOUNTS: usize = 256;
const PDA_MARKER: &str = "ProgramDerivedAddress";
const INSTRUCTION: &str = "instruction";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Roles {
    signer_writable: String,
    signer_readonly: String,
    writable: String,
    readonly: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Versions {
    prefix: u8,
    v1: u8,
    legacy: String,
    v0: String,
    v1_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeeInput {
    action: String,
    argument: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fees {
    lamports_per_signature: u64,
    units_per_instruction: u64,
    max_compute_units: u64,
    micro_lamports_per_lamport: u128,
    unit_limit: FeeInput,
    unit_price: FeeInput,
}

/// `TransactionConfigMask` 的位和 SIMD-0385 的上限.上限以后可能放宽,超过时只告警.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Schema {
    priority_fee: u32,
    compute_unit_limit: u32,
    loaded_accounts_data_size_limit: u32,
    heap_size: u32,
    max_size: usize,
    signature_bytes: usize,
    max_signatures: usize,
    max_addresses: usize,
    max_instructions: usize,
    min_heap: u32,
    max_heap: u32,
    heap_step: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OffchainLimits {
    limited: usize,
    extended: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OffchainSchema {
    domain_prefix: u8,
    domain: String,
    application_domain_bytes: u64,
    formats: Vec<String>,
    ledger: OffchainLimits,
    preamble: OffchainLimits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenTransfer {
    instructions: Vec<String>,
    destination: String,
    mint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    native_unit: String,
    item: String,
    variant: String,
    lookup_value: String,
    roles: Roles,
    versions: Versions,
    associated_token_program: String,
    token_programs: Vec<String>,
    fees: Fees,
    v1: V1Schema,
    offchain: OffchainSchema,
    token_transfer: TokenTransfer,
}

static SCHEMA: OnceLock<Schema> = OnceLock::new();

fn schema() -> &'static Schema {
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/solana/schema.json")).expect("内置 Solana 表有效")
    })
}

/// 官方 IDL 和官方 interface crate 里全部指令在规则表里的种类名.
pub fn instruction_actions() -> Vec<String> {
    let mut actions = idl_actions();
    actions.extend(Native::ALL.into_iter().flat_map(Native::actions));
    actions
}

/// 官方 IDL 里的指令.
pub fn idl_actions() -> Vec<String> {
    solana_idl::programs()
        .iter()
        .flat_map(|program| {
            program
                .instructions()
                .iter()
                .map(|instruction| action_name(program, instruction))
        })
        .collect()
}

/// 生成 IDL 表用的 schema 文件和哈希.
pub fn idl_sources() -> &'static std::collections::BTreeMap<String, String> {
    solana_idl::sources()
}

fn action_name(program: &Program, instruction: &IdlInstruction) -> String {
    [program.name.as_str(), instruction.name.as_str()].join(".")
}

/// 这个种类是 `batch` 本身,它的效果都在内部指令上.
pub fn is_batch(kind: &str) -> bool {
    solana_idl::programs().iter().any(|program| {
        let name = program.batch.as_ref().map(|batch| batch.instruction.as_str());
        name.is_some_and(|name| kind == [program.name.as_str(), name].join("."))
    })
}

/// 这个值是 option 写明的"没有".
/// v1 的配置.缺失的值节点按最小值处理,见 SIMD-0385.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct V1Config {
    pub priority_fee: Option<u64>,
    pub compute_unit_limit: Option<u32>,
    pub loaded_accounts_data_size_limit: Option<u32>,
    pub heap_size: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Legacy,
    V0,
    V1(V1Config),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// 程序在账户表里的下标.
    pub program: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

/// 地址查找表的引用.表的内容在链上,冷钱包看不到.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lookup {
    pub table: [u8; 32],
    pub writable: Vec<u8>,
    pub readonly: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub version: Version,
    pub required_signatures: u8,
    pub readonly_signed: u8,
    pub readonly_unsigned: u8,
    pub keys: Vec<[u8; 32]>,
    /// durable nonce 交易这里是 nonce 值.
    pub blockhash: [u8; 32],
    pub instructions: Vec<Instruction>,
    pub lookups: Vec<Lookup>,
}

fn put_le(out: &mut Vec<u8>, value: Option<impl Into<u64>>, width: usize) {
    if let Some(value) = value {
        out.extend_from_slice(&value.into().to_le_bytes()[..width]);
    }
}

impl V1Config {
    fn mask(&self) -> u32 {
        let Self {
            priority_fee,
            compute_unit_limit,
            loaded_accounts_data_size_limit,
            heap_size,
        } = self;
        let bits = &schema().v1;
        let set = |present: bool, mask: u32| if present { mask } else { 0 };
        set(priority_fee.is_some(), bits.priority_fee)
            | set(compute_unit_limit.is_some(), bits.compute_unit_limit)
            | set(
                loaded_accounts_data_size_limit.is_some(),
                bits.loaded_accounts_data_size_limit,
            )
            | set(heap_size.is_some(), bits.heap_size)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        put_le(out, self.priority_fee, 8);
        put_le(out, self.compute_unit_limit, 4);
        put_le(out, self.loaded_accounts_data_size_limit, 4);
        put_le(out, self.heap_size, 4);
    }
}

impl Message {
    /// 和节点的 sanitize 规则一致.节点不接受的消息,这里也不展示.
    fn check(&self) -> Result<(), Reject> {
        check_header(
            self.required_signatures,
            self.readonly_signed,
            self.readonly_unsigned,
            &self.keys,
        )?;
        let static_keys = self.keys.len();
        let total = static_keys + self.lookup_count();
        if total > MAX_ACCOUNTS {
            return Err(Reject::Malformed);
        }
        if self
            .lookups
            .iter()
            .any(|lookup| lookup.writable.is_empty() && lookup.readonly.is_empty())
        {
            return Err(Reject::Malformed);
        }
        for ix in &self.instructions {
            // 手续费支付者不能是程序,程序也不能来自查找表.
            let program = ix.program as usize;
            if program == 0 || program >= static_keys {
                return Err(Reject::Malformed);
            }
            if ix.accounts.iter().any(|&index| index as usize >= total) {
                return Err(Reject::Malformed);
            }
        }
        Ok(())
    }

    fn encode_header(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[self.required_signatures, self.readonly_signed, self.readonly_unsigned]);
    }

    fn encode_v1(&self, config: &V1Config) -> Vec<u8> {
        let mut out = vec![schema().versions.v1];
        self.encode_header(&mut out);
        out.extend_from_slice(&config.mask().to_le_bytes());
        out.extend_from_slice(&self.blockhash);
        out.extend_from_slice(&[self.instructions.len() as u8, self.keys.len() as u8]);
        self.keys.iter().for_each(|key| out.extend_from_slice(key));
        config.encode(&mut out);
        for ix in &self.instructions {
            out.extend_from_slice(&[ix.program, ix.accounts.len() as u8]);
            out.extend_from_slice(&(ix.data.len() as u16).to_le_bytes());
        }
        for ix in &self.instructions {
            out.extend_from_slice(&ix.accounts);
            out.extend_from_slice(&ix.data);
        }
        out
    }

    pub fn encode(&self) -> Vec<u8> {
        if let Version::V1(config) = &self.version {
            return self.encode_v1(config);
        }
        let mut out = Vec::new();
        if self.version == Version::V0 {
            out.push(schema().versions.prefix);
        }
        self.encode_header(&mut out);
        put_len(&mut out, self.keys.len());
        self.keys.iter().for_each(|key| out.extend_from_slice(key));
        out.extend_from_slice(&self.blockhash);
        put_len(&mut out, self.instructions.len());
        for ix in &self.instructions {
            out.push(ix.program);
            put_bytes(&mut out, &ix.accounts);
            put_bytes(&mut out, &ix.data);
        }
        if self.version == Version::V0 {
            put_len(&mut out, self.lookups.len());
            for lookup in &self.lookups {
                out.extend_from_slice(&lookup.table);
                put_bytes(&mut out, &lookup.writable);
                put_bytes(&mut out, &lookup.readonly);
            }
        }
        out
    }

    pub fn lookup_count(&self) -> usize {
        self.lookups
            .iter()
            .map(|lookup| lookup.writable.len() + lookup.readonly.len())
            .sum()
    }

    pub fn is_signer(&self, index: usize) -> bool {
        index < self.required_signatures as usize
    }

    pub fn is_writable(&self, index: usize) -> bool {
        let required = self.required_signatures as usize;
        let static_keys = self.keys.len();
        if index < required {
            index < required - self.readonly_signed as usize
        } else if index < static_keys {
            index < static_keys - self.readonly_unsigned as usize
        } else {
            let writable: usize = self.lookups.iter().map(|lookup| lookup.writable.len()).sum();
            index < static_keys + writable
        }
    }

    /// 查找表地址的来源:表和表内的下标.可写的在前,和节点加载的顺序一致.
    fn lookup_entry(&self, index: usize) -> Option<(&[u8; 32], u8)> {
        let position = index.checked_sub(self.keys.len())?;
        let writable = self
            .lookups
            .iter()
            .flat_map(|lookup| lookup.writable.iter().map(move |slot| (&lookup.table, *slot)));
        let readonly = self
            .lookups
            .iter()
            .flat_map(|lookup| lookup.readonly.iter().map(move |slot| (&lookup.table, *slot)));
        writable.chain(readonly).nth(position)
    }
}

fn check_header(required: u8, readonly_signed: u8, readonly_unsigned: u8, keys: &[[u8; 32]]) -> Result<(), Reject> {
    let fits =
        required >= 1 && readonly_signed < required && required as usize + readonly_unsigned as usize <= keys.len();
    if !fits {
        return Err(Reject::Malformed);
    }
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != keys.len() {
        return Err(Reject::Malformed);
    }
    Ok(())
}

/// compact-u16.非规范的编码由整条消息的 C2 发现.
pub(super) fn read_len(cursor: &mut Cursor<'_>) -> Result<(usize, Span), Reject> {
    let mut value = 0usize;
    let mut span: Option<Span> = None;
    for shift in COMPACT_SHIFTS {
        let field = cursor.take(1)?;
        let byte = field.bytes.first().copied().ok_or(Reject::Malformed)?;
        let start = span.map_or(field.span.offset, |span| span.offset);
        let covered = Span::new(start, field.span.end() as u32 - start);
        span = Some(covered);
        value |= ((byte & COMPACT_VALUE) as usize) << shift;
        if byte & COMPACT_MORE == 0 {
            return if value > u16::MAX as usize {
                Err(Reject::Malformed)
            } else {
                Ok((value, covered))
            };
        }
    }
    Err(Reject::Malformed)
}

pub fn put_len(out: &mut Vec<u8>, mut value: usize) {
    while value >= COMPACT_MORE as usize {
        out.push(value as u8 | COMPACT_MORE);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    put_len(out, value.len());
    out.extend_from_slice(value);
}

fn own(name: &str, value: Value, span: Span) -> Node {
    Node::new(name, value, Source::Signed(Extent::Own(span)))
}

fn group(name: &str, value: Value, children: Vec<Node>) -> Node {
    Node::new(name, value, Source::Signed(Extent::Container)).children(children)
}

fn key_of(field: &Field<'_>) -> Result<[u8; 32], Reject> {
    field.bytes.try_into().map_err(|_| Reject::Malformed)
}

fn byte(cursor: &mut Cursor<'_>) -> Result<(u8, Span), Reject> {
    let field = cursor.take(1)?;
    let value = field.bytes.first().copied().ok_or(Reject::Malformed)?;
    Ok((value, field.span))
}

struct IxSpans {
    program: Span,
    framing: Vec<Span>,
    accounts: Vec<Span>,
    /// 数据的长度前缀和数据本身分开,数据按 IDL 解码.
    data: Span,
    data_framing: Vec<Span>,
}

struct LookupSpans {
    table: Span,
    writable_framing: Span,
    writable: Vec<Span>,
    readonly_framing: Span,
    readonly: Vec<Span>,
}

struct ConfigSpans {
    mask: Span,
    priority_fee: Option<Span>,
    compute_unit_limit: Option<Span>,
    loaded_accounts_data_size_limit: Option<Span>,
    heap_size: Option<Span>,
}

/// 消息各部分的字节范围,和 `Message` 一起由同一次解析得到.
struct Layout {
    version: Option<Span>,
    header: [Span; HEADER_BYTES],
    config: Option<ConfigSpans>,
    key_framing: Span,
    keys: Vec<Span>,
    blockhash: Span,
    instruction_framing: Span,
    instructions: Vec<IxSpans>,
    lookup_framing: Option<Span>,
    lookups: Vec<LookupSpans>,
}

fn indexes(cursor: &mut Cursor<'_>) -> Result<(Vec<u8>, Span, Vec<Span>), Reject> {
    let (count, framing) = read_len(cursor)?;
    let mut values = Vec::new();
    let mut spans = Vec::new();
    for _ in 0..count {
        let (value, span) = byte(cursor)?;
        values.push(value);
        spans.push(span);
    }
    Ok((values, framing, spans))
}

fn header(cursor: &mut Cursor<'_>) -> Result<([u8; HEADER_BYTES], [Span; HEADER_BYTES]), Reject> {
    let (first, one) = byte(cursor)?;
    let (second, two) = byte(cursor)?;
    let (third, three) = byte(cursor)?;
    Ok(([first, second, third], [one, two, three]))
}

fn take_keys(cursor: &mut Cursor<'_>, count: usize) -> Result<(Vec<[u8; 32]>, Vec<Span>), Reject> {
    let mut keys = Vec::new();
    let mut spans = Vec::new();
    for _ in 0..count {
        let field = cursor.take(KEY_BYTES)?;
        keys.push(key_of(&field)?);
        spans.push(field.span);
    }
    Ok((keys, spans))
}

fn compact_instruction(cursor: &mut Cursor<'_>) -> Result<(Instruction, IxSpans), Reject> {
    let (program, program_span) = byte(cursor)?;
    let (accounts, account_framing, account_spans) = indexes(cursor)?;
    let (length, data_framing) = read_len(cursor)?;
    let data = cursor.take(length as u64)?;
    Ok((
        Instruction {
            program,
            accounts,
            data: data.bytes.to_vec(),
        },
        IxSpans {
            program: program_span,
            framing: vec![account_framing],
            accounts: account_spans,
            data: data.span,
            data_framing: vec![data_framing],
        },
    ))
}

fn parse_lookups(cursor: &mut Cursor<'_>) -> Result<(Vec<Lookup>, Span, Vec<LookupSpans>), Reject> {
    let (count, framing) = read_len(cursor)?;
    let mut lookups = Vec::new();
    let mut spans = Vec::new();
    for _ in 0..count {
        let table = cursor.take(KEY_BYTES)?;
        let (writable, writable_framing, writable_spans) = indexes(cursor)?;
        let (readonly, readonly_framing, readonly_spans) = indexes(cursor)?;
        lookups.push(Lookup {
            table: key_of(&table)?,
            writable,
            readonly,
        });
        spans.push(LookupSpans {
            table: table.span,
            writable_framing,
            writable: writable_spans,
            readonly_framing,
            readonly: readonly_spans,
        });
    }
    Ok((lookups, framing, spans))
}

fn parse_compact(
    cursor: &mut Cursor<'_>,
    version: Version,
    version_span: Option<Span>,
) -> Result<(Message, Layout), Reject> {
    let ([required_signatures, readonly_signed, readonly_unsigned], header_spans) = header(cursor)?;
    let (count, key_framing) = read_len(cursor)?;
    let (keys, key_spans) = take_keys(cursor, count)?;
    let blockhash_field = cursor.take(KEY_BYTES)?;
    let (count, instruction_framing) = read_len(cursor)?;
    let mut instructions = Vec::new();
    let mut instruction_spans = Vec::new();
    for _ in 0..count {
        let (instruction, spans) = compact_instruction(cursor)?;
        instructions.push(instruction);
        instruction_spans.push(spans);
    }
    let (lookups, lookup_framing, lookup_spans) = if version == Version::V0 {
        let (lookups, framing, spans) = parse_lookups(cursor)?;
        (lookups, Some(framing), spans)
    } else {
        (Vec::new(), None, Vec::new())
    };
    if !cursor.is_empty() {
        return Err(Reject::Malformed);
    }
    Ok((
        Message {
            version,
            required_signatures,
            readonly_signed,
            readonly_unsigned,
            keys,
            blockhash: key_of(&blockhash_field)?,
            instructions,
            lookups,
        },
        Layout {
            version: version_span,
            header: header_spans,
            config: None,
            key_framing,
            keys: key_spans,
            blockhash: blockhash_field.span,
            instruction_framing,
            instructions: instruction_spans,
            lookup_framing,
            lookups: lookup_spans,
        },
    ))
}

/// 掩码里没有定义的位.它们后面的内容宽度未知.
fn unknown_mask_bits(mask: u32) -> u32 {
    let bits = &schema().v1;
    mask & !(bits.priority_fee | bits.compute_unit_limit | bits.loaded_accounts_data_size_limit | bits.heap_size)
}

fn take_config<const N: usize>(cursor: &mut Cursor<'_>, present: bool) -> Result<Option<([u8; N], Span)>, Reject> {
    if !present {
        return Ok(None);
    }
    let field = cursor.take(N as u64)?;
    let bytes = field.bytes.try_into().map_err(|_| Reject::Malformed)?;
    Ok(Some((bytes, field.span)))
}

/// 配置值,顺序是 SIMD-0385 的位序.优先费占两位,只写一位时节点拒绝.
fn parse_config(cursor: &mut Cursor<'_>, mask: u32, mask_span: Span) -> Result<(V1Config, ConfigSpans), Reject> {
    let bits = &schema().v1;
    let fee_bits = mask & bits.priority_fee;
    if fee_bits != 0 && fee_bits != bits.priority_fee {
        return Err(Reject::Malformed);
    }
    let fee = take_config::<8>(cursor, fee_bits != 0)?;
    let units = take_config::<4>(cursor, mask & bits.compute_unit_limit != 0)?;
    let size = take_config::<4>(cursor, mask & bits.loaded_accounts_data_size_limit != 0)?;
    let heap = take_config::<4>(cursor, mask & bits.heap_size != 0)?;
    Ok((
        V1Config {
            priority_fee: fee.map(|(bytes, _)| u64::from_le_bytes(bytes)),
            compute_unit_limit: units.map(|(bytes, _)| u32::from_le_bytes(bytes)),
            loaded_accounts_data_size_limit: size.map(|(bytes, _)| u32::from_le_bytes(bytes)),
            heap_size: heap.map(|(bytes, _)| u32::from_le_bytes(bytes)),
        },
        ConfigSpans {
            mask: mask_span,
            priority_fee: fee.map(|(_, span)| span),
            compute_unit_limit: units.map(|(_, span)| span),
            loaded_accounts_data_size_limit: size.map(|(_, span)| span),
            heap_size: heap.map(|(_, span)| span),
        },
    ))
}

/// 指令头和指令体分开放,先读全部头.
fn parse_v1_instructions(cursor: &mut Cursor<'_>, count: usize) -> Result<(Vec<Instruction>, Vec<IxSpans>), Reject> {
    let mut heads = Vec::new();
    for _ in 0..count {
        let (program, program_span) = byte(cursor)?;
        let (accounts, account_span) = byte(cursor)?;
        let length = cursor.take(2)?;
        let size = u16::from_le_bytes(length.bytes.try_into().map_err(|_| Reject::Malformed)?);
        heads.push((program, program_span, accounts, account_span, size, length.span));
    }
    let mut instructions = Vec::new();
    let mut spans = Vec::new();
    for (program, program_span, accounts, account_span, size, length_span) in heads {
        let mut indexes = Vec::new();
        let mut index_spans = Vec::new();
        for _ in 0..accounts {
            let (value, span) = byte(cursor)?;
            indexes.push(value);
            index_spans.push(span);
        }
        let data = cursor.take(u64::from(size))?;
        instructions.push(Instruction {
            program,
            accounts: indexes,
            data: data.bytes.to_vec(),
        });
        spans.push(IxSpans {
            program: program_span,
            framing: vec![account_span],
            accounts: index_spans,
            data: data.span,
            data_framing: vec![length_span],
        });
    }
    Ok((instructions, spans))
}

/// 掩码有未知的位时,地址之后的内容宽度未知,只能到这里.
pub struct PartialMessage {
    pub required_signatures: u8,
    pub readonly_signed: u8,
    pub readonly_unsigned: u8,
    pub mask: u32,
    pub blockhash: [u8; 32],
    pub instruction_count: u8,
    pub keys: Vec<[u8; 32]>,
    version: Span,
    header: [Span; HEADER_BYTES],
    mask_span: Span,
    blockhash_span: Span,
    instruction_count_span: Span,
    key_framing: Span,
    key_spans: Vec<Span>,
    rest: Span,
}

enum V1Parsed {
    Whole(Box<(Message, Layout)>),
    Partial(PartialMessage),
}

fn parse_v1(cursor: &mut Cursor<'_>, version: Span) -> Result<V1Parsed, Reject> {
    let ([required_signatures, readonly_signed, readonly_unsigned], header_spans) = header(cursor)?;
    let mask_field = cursor.take(4)?;
    let mask = u32::from_le_bytes(mask_field.bytes.try_into().map_err(|_| Reject::Malformed)?);
    let blockhash_field = cursor.take(KEY_BYTES)?;
    let (instruction_count, instruction_framing) = byte(cursor)?;
    let (key_count, key_framing) = byte(cursor)?;
    let (keys, key_spans) = take_keys(cursor, key_count as usize)?;
    if unknown_mask_bits(mask) != 0 {
        check_header(required_signatures, readonly_signed, readonly_unsigned, &keys)?;
        return Ok(V1Parsed::Partial(PartialMessage {
            required_signatures,
            readonly_signed,
            readonly_unsigned,
            mask,
            blockhash: key_of(&blockhash_field)?,
            instruction_count,
            keys,
            version,
            header: header_spans,
            mask_span: mask_field.span,
            blockhash_span: blockhash_field.span,
            instruction_count_span: instruction_framing,
            key_framing,
            key_spans,
            rest: cursor.rest().span,
        }));
    }
    let (config, config_spans) = parse_config(cursor, mask, mask_field.span)?;
    let (instructions, instruction_spans) = parse_v1_instructions(cursor, instruction_count as usize)?;
    if !cursor.is_empty() {
        return Err(Reject::Malformed);
    }
    Ok(V1Parsed::Whole(Box::new((
        Message {
            version: Version::V1(config),
            required_signatures,
            readonly_signed,
            readonly_unsigned,
            keys,
            blockhash: key_of(&blockhash_field)?,
            instructions,
            lookups: Vec::new(),
        },
        Layout {
            version: Some(version),
            header: header_spans,
            config: Some(config_spans),
            key_framing,
            keys: key_spans,
            blockhash: blockhash_field.span,
            instruction_framing,
            instructions: instruction_spans,
            lookup_framing: None,
            lookups: Vec::new(),
        },
    ))))
}

/// 官方有两种 v0:`solana-offchain-message` 没有应用域和签名者,`@solana/offchain-messages` 有.
enum OffchainBody<'a> {
    Ledger {
        format: Field<'a>,
        length: Field<'a>,
        message: Field<'a>,
    },
    Preamble {
        application_domain: Field<'a>,
        format: Field<'a>,
        count: Field<'a>,
        signers: Vec<Field<'a>>,
        length: Field<'a>,
        message: Field<'a>,
    },
    /// 版本 1,sRFC 3.签名者排序,没有长度字段.
    Signers {
        count: Field<'a>,
        signers: Vec<Field<'a>>,
        message: Field<'a>,
    },
    /// 以后的版本.外层是已知的签名域,所以不走 `opaque` 根节点.
    Future { data: Field<'a> },
}

/// 链下消息.`signers` 是消息声明的签名者,空表示格式里没有这一项.
pub struct Offchain<'a> {
    domain: Field<'a>,
    version: Field<'a>,
    body: OffchainBody<'a>,
    pub signers: Option<Vec<[u8; 32]>>,
}

fn offchain_text(format: u8, message: &[u8], limits: &OffchainLimits) -> Result<(), Reject> {
    let text = std::str::from_utf8(message).map_err(|_| Reject::Malformed)?;
    let printable = text.bytes().all(|byte| (0x20..=0x7e).contains(&byte));
    let valid = !message.is_empty()
        && match format {
            0 => printable && message.len() <= limits.limited,
            1 => message.len() <= limits.limited,
            2 => message.len() <= limits.extended,
            3.. => false,
        };
    if valid { Ok(()) } else { Err(Reject::Malformed) }
}

fn format_byte(field: &Field<'_>) -> Result<u8, Reject> {
    field.bytes.first().copied().ok_or(Reject::Malformed)
}

fn declared_length(field: &Field<'_>) -> Result<usize, Reject> {
    let bytes: [u8; 2] = field.bytes.try_into().map_err(|_| Reject::Malformed)?;
    Ok(u16::from_le_bytes(bytes) as usize)
}

fn offchain_ledger<'a>(body: &Field<'a>) -> Result<OffchainBody<'a>, Reject> {
    let mut cursor = Cursor::new(body);
    let format = cursor.take(1)?;
    let length = cursor.take(2)?;
    let message = cursor.rest();
    if declared_length(&length)? != message.bytes.len() {
        return Err(Reject::Malformed);
    }
    offchain_text(format_byte(&format)?, message.bytes, &schema().offchain.ledger)?;
    Ok(OffchainBody::Ledger {
        format,
        length,
        message,
    })
}

fn signer_fields<'a>(cursor: &mut Cursor<'a>, count: usize) -> Result<Vec<Field<'a>>, Reject> {
    if count == 0 {
        return Err(Reject::Malformed);
    }
    (0..count).map(|_| cursor.take(KEY_BYTES)).collect()
}

fn offchain_preamble<'a>(body: &Field<'a>) -> Result<OffchainBody<'a>, Reject> {
    let mut cursor = Cursor::new(body);
    let application_domain = cursor.take(schema().offchain.application_domain_bytes)?;
    let format = cursor.take(1)?;
    let count = cursor.take(1)?;
    let signers = signer_fields(&mut cursor, format_byte(&count)? as usize)?;
    let length = cursor.take(2)?;
    let message = cursor.rest();
    if declared_length(&length)? != message.bytes.len() {
        return Err(Reject::Malformed);
    }
    offchain_text(format_byte(&format)?, message.bytes, &schema().offchain.preamble)?;
    Ok(OffchainBody::Preamble {
        application_domain,
        format,
        count,
        signers,
        length,
        message,
    })
}

fn offchain_signers<'a>(body: &Field<'a>) -> Result<OffchainBody<'a>, Reject> {
    let mut cursor = Cursor::new(body);
    let count = cursor.take(1)?;
    let signers = signer_fields(&mut cursor, format_byte(&count)? as usize)?;
    if signers
        .windows(2)
        .any(|pair| pair.first().map(|field| field.bytes) >= pair.get(1).map(|field| field.bytes))
    {
        return Err(Reject::Malformed);
    }
    let message = cursor.rest();
    if message.bytes.is_empty() || std::str::from_utf8(message.bytes).is_err() {
        return Err(Reject::Malformed);
    }
    Ok(OffchainBody::Signers {
        count,
        signers,
        message,
    })
}

/// 两种 v0 都成立的字节,不同的解析器结果不同,所以拒绝.
fn offchain_v0<'a>(body: &Field<'a>) -> Result<OffchainBody<'a>, Reject> {
    match (offchain_ledger(body), offchain_preamble(body)) {
        (Ok(ledger), Err(_)) => Ok(ledger),
        (Err(_), Ok(preamble)) => Ok(preamble),
        (Ok(_), Ok(_)) | (Err(_), Err(_)) => Err(Reject::Malformed),
    }
}

fn parse_offchain(payload: &[u8]) -> Result<Offchain<'_>, Reject> {
    let domain_text = &schema().offchain.domain;
    let mut cursor = Cursor::new(&Field::whole(payload));
    let domain = cursor.take(1 + domain_text.len() as u64)?;
    let version = cursor.take(1)?;
    let rest = cursor.rest();
    let body = match format_byte(&version)? {
        0 => offchain_v0(&rest)?,
        1 => offchain_signers(&rest)?,
        2.. => OffchainBody::Future { data: rest },
    };
    let declared = |fields: &[Field<'_>]| fields.iter().map(key_of).collect::<Result<Vec<_>, _>>().map(Some);
    let signers = match &body {
        OffchainBody::Ledger {
            format: _format,
            length: _length,
            message: _message,
        } => None,
        OffchainBody::Future { data: _data } => None,
        OffchainBody::Preamble {
            application_domain: _application_domain,
            format: _format,
            count: _count,
            signers,
            length: _length,
            message: _message,
        } => declared(signers)?,
        OffchainBody::Signers {
            count: _count,
            signers,
            message: _message,
        } => declared(signers)?,
    };
    Ok(Offchain {
        domain,
        version,
        body,
        signers,
    })
}

fn is_offchain(payload: &[u8]) -> bool {
    let offchain = &schema().offchain;
    payload.first() == Some(&offchain.domain_prefix)
        && payload
            .get(1..)
            .is_some_and(|rest| rest.starts_with(offchain.domain.as_bytes()))
}

/// 载荷的格式.`\xffsolana offchain` 解不开时拒绝,永远不算未知格式.
enum Parsed<'a> {
    Message(Box<(Message, Layout)>),
    Partial(PartialMessage),
    Offchain(Offchain<'a>),
    Unknown,
}

fn parse(payload: &[u8]) -> Result<Parsed<'_>, Reject> {
    let versions = &schema().versions;
    let first = *payload.first().ok_or(Reject::Malformed)?;
    if is_offchain(payload) {
        return parse_offchain(payload).map(Parsed::Offchain);
    }
    let mut cursor = Cursor::new(&Field::whole(payload));
    let parsed = if first < versions.prefix {
        Box::new(parse_compact(&mut cursor, Version::Legacy, None)?)
    } else if first == versions.prefix {
        let tag = cursor.take(1)?;
        Box::new(parse_compact(&mut cursor, Version::V0, Some(tag.span))?)
    } else if first == versions.v1 {
        let tag = cursor.take(1)?;
        match parse_v1(&mut cursor, tag.span)? {
            V1Parsed::Whole(parsed) => parsed,
            V1Parsed::Partial(partial) => return Ok(Parsed::Partial(partial)),
        }
    } else {
        return Ok(Parsed::Unknown);
    };
    let (message, _) = parsed.as_ref();
    message.check()?;
    if message.encode() != payload {
        return Err(Reject::RoundTrip);
    }
    Ok(Parsed::Message(parsed))
}

/// 对外的格式判断,`tx` 用它核对签名者.
pub enum SolanaPayload {
    Message(Box<Message>),
    /// 掩码有未知的位,地址之后的内容是 `opaque`.
    Partial {
        required: Vec<[u8; 32]>,
    },
    /// 链下消息.`signers` 是消息声明的签名者,空表示格式里没有这一项.
    Offchain {
        signers: Option<Vec<[u8; 32]>>,
    },
    Unknown,
}

impl SolanaPayload {
    /// 载荷要求的签名者.`None` 表示载荷没有说,比如整段未知的格式.
    pub fn signers(&self) -> Option<Vec<[u8; 32]>> {
        match self {
            Self::Message(message) => Some(message.keys[..message.required_signatures as usize].to_vec()),
            Self::Partial { required } => Some(required.clone()),
            Self::Offchain { signers } => signers.clone(),
            Self::Unknown => None,
        }
    }
}

pub fn parse_solana(payload: &[u8]) -> Result<SolanaPayload, Reject> {
    Ok(match parse(payload)? {
        Parsed::Message(parsed) => SolanaPayload::Message(Box::new(parsed.0)),
        Parsed::Partial(partial) => SolanaPayload::Partial {
            required: partial.keys[..partial.required_signatures as usize].to_vec(),
        },
        Parsed::Offchain(offchain) => SolanaPayload::Offchain {
            signers: offchain.signers,
        },
        Parsed::Unknown => SolanaPayload::Unknown,
    })
}

fn program_key(text: &str) -> Option<[u8; 32]> {
    base58::decode(text).ok()?.try_into().ok()
}

/// 程序派生地址不能在曲线上,否则会有对应的私钥.
fn find_program_address(seeds: &[&[u8]], program: &[u8; 32]) -> Option<[u8; 32]> {
    (1..=u8::MAX).rev().find_map(|bump| {
        let mut hasher = Sha256::new();
        seeds.iter().for_each(|seed| hasher.update(seed));
        let address: [u8; 32] = hasher
            .chain_update([bump])
            .chain_update(program)
            .chain_update(PDA_MARKER)
            .finalize()
            .into();
        VerifyingKey::from_bytes(&address).is_err().then_some(address)
    })
}

pub fn associated_token_address(owner: &[u8; 32], mint: &[u8; 32], program: &[u8; 32]) -> Option<[u8; 32]> {
    let associated = program_key(&schema().associated_token_program)?;
    find_program_address(&[owner, program, mint], &associated)
}

pub fn is_token_program(key: &[u8; 32]) -> bool {
    let text = base58::encode(key);
    schema().token_programs.contains(&text)
}

/// 一条指令用到的一个账户位.
#[derive(Clone, Copy)]
struct AccountSlot {
    index: u8,
    span: Span,
}

/// 一条指令解码的结果.
struct Body {
    name: Option<String>,
    action: String,
    accounts: Vec<Node>,
    data: Vec<Node>,
}

fn number_of(node: &Node) -> Option<u128> {
    let bytes = if let Value::Amount(Amount {
        raw,
        decimals: _decimals,
        unit: _unit,
    }) = &node.value
    {
        raw
    } else if let Value::Uint(raw) = &node.value {
        raw
    } else {
        return None;
    };
    let mut wide = [0u8; 16];
    let start = wide.len().checked_sub(bytes.len())?;
    wide[start..].copy_from_slice(bytes);
    Some(u128::from_be_bytes(wide))
}

fn child<'n>(nodes: &'n [Node], name: &str) -> Option<&'n Node> {
    nodes.iter().find(|node| node.name == name)
}

fn account_key(nodes: &[Node], name: &str) -> Option<[u8; 32]> {
    let Value::Address(text) = &child(nodes, name)?.value else {
        return None;
    };
    program_key(text)
}

/// 一次消息遍历.
struct Walk<'a> {
    message: &'a Message,
    coin: Option<Coin>,
    hint: Option<[u8; 32]>,
    checks: Vec<RoundTrip>,
    unit_limit: Option<u64>,
    unit_price: Option<u64>,
}

impl<'a> Walk<'a> {
    fn role(&self, index: usize) -> Node {
        let roles = &schema().roles;
        let label = match (self.message.is_signer(index), self.message.is_writable(index)) {
            (true, true) => &roles.signer_writable,
            (true, false) => &roles.signer_readonly,
            (false, true) => &roles.writable,
            (false, false) => &roles.readonly,
        };
        Node::new("role", Value::Enum(label.clone()), Source::Derived)
    }

    /// 账户的值和它的来源.查找表里的地址是链上状态,节点是表地址加索引,不猜测.
    fn account_parts(&self, slot: AccountSlot) -> (Value, Vec<Node>, bool) {
        let index = slot.index as usize;
        let role = self.role(index);
        match self.message.keys.get(index) {
            Some(key) => (Value::Address(base58::encode(key)), vec![role], false),
            None => {
                let mut children = vec![role];
                if let Some((table, position)) = self.message.lookup_entry(index) {
                    children.push(Node::new(
                        "lookup_table",
                        Value::Address(base58::encode(table)),
                        Source::Derived,
                    ));
                    children.push(Node::new("lookup_index", Value::Uint(vec![position]), Source::Derived));
                }
                (Value::Enum(schema().lookup_value.clone()), children, true)
            }
        }
    }

    fn account(&self, name: &str, slot: AccountSlot) -> Node {
        let (value, children, from_table) = self.account_parts(slot);
        let node = own(name, value, slot.span).children(children);
        if from_table {
            node.action("lookup-account")
        } else {
            node
        }
    }

    /// IDL 之外的账户.程序通常忽略,所以是 `opaque`,地址放在子节点.
    fn extra_account(&self, slot: AccountSlot) -> Node {
        let (value, mut children, _) = self.account_parts(slot);
        children.insert(0, Node::new("address", value, Source::Derived));
        own("extra_account", Value::Opaque(vec![slot.index]), slot.span).children(children)
    }

    /// 账户按哪种布局读,只看这个位置上的地址.`None` 表示地址在查找表里,看不出来.
    fn layout<'i>(&self, instruction: &'i IdlInstruction, slots: &[AccountSlot]) -> Option<&'i [Account]> {
        let older = instruction.older.as_ref();
        let marked = older.and_then(|older| Some((older, slots.get(older.slot)?)));
        let Some((older, slot)) = marked else {
            return Some(&instruction.accounts);
        };
        let key = self.message.keys.get(slot.index as usize)?;
        Some(if base58::encode(key) == older.sysvar {
            &older.accounts
        } else {
            &instruction.accounts
        })
    }

    /// 账户数少于 IDL 必需的数量时解码失败,程序本身不会执行.
    fn named_accounts(&self, instruction: &IdlInstruction, slots: &[AccountSlot]) -> Result<Vec<Node>, Reject> {
        let Some(accounts) = self.layout(instruction, slots) else {
            return Ok(self.opaque_accounts(slots));
        };
        let required = accounts.iter().filter(|account| !account.optional).count();
        let mut spare = slots.len().checked_sub(required).ok_or(Reject::Malformed)?;
        let mut rest = slots.iter().copied().peekable();
        let mut nodes = Vec::new();
        for account in accounts {
            if account.optional {
                if spare == 0 {
                    continue;
                }
                spare -= 1;
            }
            let slot = rest.next().ok_or(Reject::Malformed)?;
            nodes.push(self.account(&account.name, slot));
        }
        for remaining in &instruction.remaining {
            while let Some(slot) = rest.next_if(|slot| !remaining.signer || self.message.is_signer(slot.index as usize))
            {
                nodes.push(self.account(&remaining.name, slot));
            }
        }
        nodes.extend(rest.map(|slot| self.extra_account(slot)));
        Ok(nodes)
    }

    /// 官方 interface crate 解码的原生程序.账户没有官方的名字,按指令里的位置命名,和官方构建函数的顺序一致.
    fn native(&mut self, native: Native, slots: &[AccountSlot], data: &Field<'_>) -> Result<Body, Reject> {
        let Some(decoded) = native.decode(data)? else {
            return Ok(Body {
                name: None,
                action: "instruction-unknown".to_string(),
                accounts: self.opaque_accounts(slots),
                data: vec![own("data", Value::Opaque(data.bytes.to_vec()), data.span)],
            });
        };
        self.checks.push(decoded.check);
        let accounts = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| self.account(&index.to_string(), *slot))
            .collect();
        Ok(Body {
            action: native.action(decoded.name.as_deref()),
            name: decoded.name,
            accounts,
            data: decoded.data,
        })
    }

    fn opaque_accounts(&self, slots: &[AccountSlot]) -> Vec<Node> {
        slots.iter().map(|slot| self.account("account", *slot)).collect()
    }

    fn type_walk(&self, program: &'static Program) -> TypeWalk<'static> {
        let names = schema();
        TypeWalk {
            program,
            coin: self.coin,
            native_unit: &names.native_unit,
            none: crate::decode::ABSENT_TEXT,
            item: &names.item,
            variant: &names.variant,
            rebuilt: Vec::new(),
        }
    }

    /// 读不完时整段是 `opaque`:程序以后可能改变格式,文本不合法才是确定的错误.
    /// 尾部字节程序本身会忽略,链接受,所以也是 `opaque`.
    fn arguments(
        &mut self,
        program: &'static Program,
        instruction: &IdlInstruction,
        data: &Field<'_>,
    ) -> Result<(Vec<Node>, bool), Reject> {
        // 程序把空数据当成这条指令时,判别值也不在数据里.
        if data.bytes.is_empty() {
            return Ok((Vec::new(), false));
        }
        let mut walk = self.type_walk(program);
        let mut cursor = Cursor::new(data);
        let mut nodes = Vec::new();
        for argument in &instruction.arguments {
            match walk.read(&argument.name, &argument.node, &mut cursor) {
                Ok(node) => nodes.push(node),
                Err(Reject::Malformed) => {
                    return Ok((vec![own("data", Value::Opaque(data.bytes.to_vec()), data.span)], false));
                }
                Err(reason) => return Err(reason),
            }
        }
        let trailing = cursor.rest();
        walk.rebuilt.extend_from_slice(trailing.bytes);
        if !trailing.bytes.is_empty() {
            nodes.push(own(
                "trailing_data",
                Value::Opaque(trailing.bytes.to_vec()),
                trailing.span,
            ));
        }
        self.checks.push(RoundTrip {
            original: data.bytes.to_vec(),
            rebuilt: walk.rebuilt,
        });
        Ok((nodes, true))
    }

    /// `batch` 的内部指令按同一份 IDL 解码.账户按顺序分给内部指令,嵌套的 `batch` 解码失败.
    fn batch(
        &mut self,
        program: &'static Program,
        program_id: &[u8; 32],
        slots: &[AccountSlot],
        data: &Field<'_>,
        nodes: Vec<Node>,
    ) -> Result<(Vec<Node>, Vec<Node>), Reject> {
        let shape = program.batch.as_ref().ok_or(Reject::Malformed)?;
        let mut pool = slots.iter().copied();
        let mut rebuilt = Vec::new();
        for mut node in nodes {
            if node.name == shape.argument {
                let items = std::mem::take(&mut node.children);
                let mut inner = Vec::new();
                for item in items {
                    inner.push(self.batch_item(shape, program_id, &mut pool, data, item)?);
                }
                node.children = inner;
            }
            rebuilt.push(node);
        }
        let leftover: Vec<Node> = pool.map(|slot| self.extra_account(slot)).collect();
        Ok((rebuilt, leftover))
    }

    fn batch_item(
        &mut self,
        shape: &solana_idl::Batch,
        program_id: &[u8; 32],
        pool: &mut impl Iterator<Item = AccountSlot>,
        data: &Field<'_>,
        item: Node,
    ) -> Result<Node, Reject> {
        let count = child(&item.children, &shape.accounts)
            .and_then(number_of)
            .ok_or(Reject::Malformed)?;
        let payload = child(&item.children, &shape.payload).ok_or(Reject::Malformed)?;
        let Source::Signed(Extent::Own(span)) = payload.source else {
            return Err(Reject::Malformed);
        };
        let start = span.offset.checked_sub(data.span.offset).ok_or(Reject::Malformed)? as usize;
        let bytes = data
            .bytes
            .get(start..start + span.len as usize)
            .ok_or(Reject::Malformed)?;
        let accounts: Vec<AccountSlot> = pool.take(count as usize).collect();
        if accounts.len() as u128 != count {
            return Err(Reject::Malformed);
        }
        let inner = self.body(program_id, &accounts, &Field { bytes, span }, true)?;
        let framed = instruction_node(inner, payload.framing.clone(), Vec::new(), Vec::new(), None);
        let kept: Vec<Node> = item
            .children
            .into_iter()
            .filter(|node| node.name == shape.accounts)
            .chain([framed])
            .collect();
        Ok(group(&item.name, Value::Struct, kept))
    }

    fn body(
        &mut self,
        program_id: &[u8; 32],
        slots: &[AccountSlot],
        data: &Field<'_>,
        nested: bool,
    ) -> Result<Body, Reject> {
        let Some(program) = solana_idl::program(program_id) else {
            if let Some(native) = Native::of(program_id) {
                return self.native(native, slots, data);
            }
            return Ok(Body {
                name: None,
                action: "program-unknown".to_string(),
                accounts: self.opaque_accounts(slots),
                data: vec![own("data", Value::Opaque(data.bytes.to_vec()), data.span)],
            });
        };
        let Some(instruction) = program.identify(data.bytes) else {
            return Ok(Body {
                name: None,
                action: "instruction-unknown".to_string(),
                accounts: self.opaque_accounts(slots),
                data: vec![own("data", Value::Opaque(data.bytes.to_vec()), data.span)],
            });
        };
        let batching = program
            .batch
            .as_ref()
            .is_some_and(|batch| batch.instruction == instruction.name);
        if batching && nested {
            return Err(Reject::Malformed);
        }
        let (mut nodes, decoded) = self.arguments(program, instruction, data)?;
        let accounts = if batching && decoded {
            let (inner, leftover) = self.batch(program, program_id, slots, data, nodes)?;
            nodes = inner;
            leftover
        } else {
            self.named_accounts(instruction, slots)?
        };
        if decoded && let Some(owner) = self.confirmed_owner(program_id, instruction, &accounts) {
            nodes.push(Node::new(
                "recipient_owner",
                Value::Address(base58::encode(&owner)),
                Source::Derived,
            ));
        }
        Ok(Body {
            name: Some(instruction.name.clone()),
            action: action_name(program, instruction),
            accounts,
            data: nodes,
        })
    }

    /// 网页端声明的收款方所有者推导得出目标账户,才变成 `derived`.
    fn confirmed_owner(
        &self,
        program_id: &[u8; 32],
        instruction: &IdlInstruction,
        accounts: &[Node],
    ) -> Option<[u8; 32]> {
        let shape = &schema().token_transfer;
        if !is_token_program(program_id) || !shape.instructions.contains(&instruction.name) {
            return None;
        }
        let owner = self.hint?;
        let mint = account_key(accounts, &shape.mint)?;
        let destination = account_key(accounts, &shape.destination)?;
        (associated_token_address(&owner, &mint, program_id)? == destination).then_some(owner)
    }

    fn note_fee_inputs(&mut self, action: &str, data: &[Node]) {
        let fees = &schema().fees;
        let read = |input: &FeeInput| {
            (input.action == action)
                .then(|| child(data, &input.argument).and_then(number_of))
                .flatten()
                .and_then(|value| u64::try_from(value).ok())
        };
        if let Some(units) = read(&fees.unit_limit) {
            self.unit_limit = Some(self.unit_limit.unwrap_or(0).max(units));
        }
        if let Some(price) = read(&fees.unit_price) {
            self.unit_price = Some(self.unit_price.unwrap_or(0).max(price));
        }
    }

    fn instruction(&mut self, ix: &Instruction, spans: &IxSpans, payload: &'a [u8]) -> Result<Node, Reject> {
        let Instruction {
            program,
            accounts,
            data,
        } = ix;
        let key = self.message.keys.get(*program as usize).ok_or(Reject::Malformed)?;
        let slots: Vec<AccountSlot> = accounts
            .iter()
            .zip(&spans.accounts)
            .map(|(index, span)| AccountSlot {
                index: *index,
                span: *span,
            })
            .collect();
        let field = Field {
            bytes: payload
                .get(spans.data.offset as usize..spans.data.end() as usize)
                .ok_or(Reject::Malformed)?,
            span: spans.data,
        };
        if field.bytes != data.as_slice() {
            return Err(Reject::RoundTrip);
        }
        let body = self.body(key, &slots, &field, false)?;
        self.note_fee_inputs(&body.action, &body.data);
        let program_row = own("program_id", Value::Address(base58::encode(key)), spans.program);
        Ok(instruction_node(
            body,
            Vec::new(),
            spans.framing.clone(),
            spans.data_framing.clone(),
            Some(program_row),
        ))
    }

    fn coin_amount(&self, lamports: u64) -> Value {
        Value::Amount(Amount {
            raw: lamports
                .to_be_bytes()
                .iter()
                .copied()
                .skip_while(|byte| *byte == 0)
                .collect(),
            decimals: self.coin.map(|coin| coin.decimals),
            unit: self.coin.map(|coin| coin.unit.to_string()),
        })
    }

    /// 微 lamports 单价乘单元数,向上取整到 lamports.结果超过 64 位时取上限,节点也是饱和计算.
    fn priority_fee(&self) -> u64 {
        let fees = &schema().fees;
        if let Version::V1(config) = &self.message.version {
            return config.priority_fee.unwrap_or(0);
        }
        let default_units = self.message.instructions.len() as u64 * fees.units_per_instruction;
        let units = self.unit_limit.unwrap_or(default_units).min(fees.max_compute_units);
        let price = u128::from(self.unit_price.unwrap_or(0));
        let total = (price * u128::from(units)).div_ceil(fees.micro_lamports_per_lamport);
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    fn base_fee(&self) -> u64 {
        schema().fees.lamports_per_signature * u64::from(self.message.required_signatures)
    }
}

/// `program_row` 只有消息里的指令才有,`batch` 的内部指令用同一个程序.
fn instruction_node(
    body: Body,
    framing: Vec<Span>,
    account_framing: Vec<Span>,
    data_framing: Vec<Span>,
    program_row: Option<Node>,
) -> Node {
    let Body {
        name,
        action,
        accounts,
        data,
    } = body;
    let label = name.map(|name| Node::new("instruction_name", Value::Enum(name), Source::Derived));
    let accounts = group("accounts", Value::List, accounts).framed(account_framing);
    let data = group("data", Value::Struct, data).framed(data_framing);
    let children = program_row.into_iter().chain(label).chain([accounts, data]).collect();
    group(INSTRUCTION, Value::Struct, children)
        .framed(framing)
        .action(action)
}

fn uint(value: u64) -> Value {
    Value::Uint(
        value
            .to_be_bytes()
            .iter()
            .copied()
            .skip_while(|byte| *byte == 0)
            .collect(),
    )
}

fn header_nodes(values: [u8; HEADER_BYTES], spans: [Span; HEADER_BYTES]) -> Vec<Node> {
    let [one, two, three] = values;
    let [first, second, third] = spans;
    vec![
        own("num_required_signatures", uint(u64::from(one)), first),
        own("num_readonly_signed_accounts", uint(u64::from(two)), second),
        own("num_readonly_unsigned_accounts", uint(u64::from(three)), third),
    ]
}

fn key_rows(keys: &[[u8; 32]], spans: &[Span], role: impl Fn(usize) -> Node) -> Vec<Node> {
    keys.iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (key, span))| {
            own("account_key", Value::Address(base58::encode(key)), *span).children(vec![role(index)])
        })
        .collect()
}

fn config_rows(config: &V1Config, spans: &ConfigSpans) -> Vec<Node> {
    let v1 = &schema().v1;
    let heap_fits = |size: u32| size >= v1.min_heap && size <= v1.max_heap && size.is_multiple_of(v1.heap_step);
    let mut rows = vec![own(
        "transaction_config_mask",
        uint(u64::from(config.mask())),
        spans.mask,
    )];
    rows.extend(
        config
            .priority_fee
            .zip(spans.priority_fee)
            .map(|(value, span)| own("config_priority_fee", uint(value), span)),
    );
    rows.extend(
        config
            .compute_unit_limit
            .zip(spans.compute_unit_limit)
            .map(|(value, span)| own("compute_unit_limit", uint(u64::from(value)), span)),
    );
    rows.extend(
        config
            .loaded_accounts_data_size_limit
            .zip(spans.loaded_accounts_data_size_limit)
            .map(|(value, span)| own("loaded_accounts_data_size_limit", uint(u64::from(value)), span)),
    );
    rows.extend(config.heap_size.zip(spans.heap_size).map(|(value, span)| {
        let row = own("heap_size", uint(u64::from(value)), span);
        if heap_fits(value) {
            row
        } else {
            row.action("v1-heap-size")
        }
    }));
    rows
}

fn lookup_rows(lookup: &Lookup, spans: &LookupSpans) -> Node {
    let list = |name: &str, values: &[u8], framing: Span, slots: &[Span]| {
        let rows = values
            .iter()
            .zip(slots)
            .map(|(value, span)| own("index", uint(u64::from(*value)), *span))
            .collect();
        group(name, Value::List, rows).framed(vec![framing])
    };
    group(
        "address_table_lookup",
        Value::Struct,
        vec![
            own("table", Value::Address(base58::encode(&lookup.table)), spans.table),
            list(
                "writable_indexes",
                &lookup.writable,
                spans.writable_framing,
                &spans.writable,
            ),
            list(
                "readonly_indexes",
                &lookup.readonly,
                spans.readonly_framing,
                &spans.readonly,
            ),
        ],
    )
}

fn version_row(message: &Message, span: Option<Span>) -> Node {
    let versions = &schema().versions;
    let label = match &message.version {
        Version::Legacy => &versions.legacy,
        Version::V0 => &versions.v0,
        Version::V1(_) => &versions.v1_name,
    };
    let value = Value::Enum(label.clone());
    span.map_or_else(
        || Node::new("message_version", value.clone(), Source::Derived),
        |span| own("message_version", value.clone(), span),
    )
}

/// v1 的上限以后可能放宽,超过时只告警.
fn over_v1_limit(message: &Message, payload_len: usize) -> Vec<&'static str> {
    let v1 = &schema().v1;
    if !matches!(message.version, Version::V1(_)) {
        return Vec::new();
    }
    let signatures = message.required_signatures as usize;
    let checks = [
        (
            payload_len + signatures * v1.signature_bytes > v1.max_size,
            "v1-transaction-size",
        ),
        (signatures > v1.max_signatures, "v1-signatures"),
        (message.keys.len() > v1.max_addresses, "v1-addresses"),
        (message.instructions.len() > v1.max_instructions, "v1-instructions"),
    ];
    checks
        .into_iter()
        .filter(|(over, _)| *over)
        .map(|(_, kind)| kind)
        .collect()
}

fn walk_message(parsed: &(Message, Layout), payload: &[u8], walker: &SolanaWalker) -> Result<Walked, Reject> {
    let (message, layout) = parsed;
    let Layout {
        version,
        header,
        config,
        key_framing,
        keys,
        blockhash,
        instruction_framing,
        instructions,
        lookup_framing,
        lookups,
    } = layout;
    let mut walk = Walk {
        message,
        coin: walker.coin,
        hint: walker.hint,
        checks: vec![RoundTrip {
            original: payload.to_vec(),
            rebuilt: message.encode(),
        }],
        unit_limit: None,
        unit_price: None,
    };
    let instruction_rows = message
        .instructions
        .iter()
        .zip(instructions)
        .map(|(ix, spans)| walk.instruction(ix, spans, payload))
        .collect::<Result<Vec<_>, _>>()?;
    let over = over_v1_limit(message, payload.len());
    let marked = |node: Node, kind: &str| {
        if over.contains(&kind) { node.action(kind) } else { node }
    };
    let mut children = vec![
        Node::new("base_fee", walk.coin_amount(walk.base_fee()), Source::Derived),
        Node::new("priority_fee", walk.coin_amount(walk.priority_fee()), Source::Derived),
        marked(version_row(message, *version), "v1-transaction-size"),
    ];
    let mut head = header_nodes(
        [
            message.required_signatures,
            message.readonly_signed,
            message.readonly_unsigned,
        ],
        *header,
    );
    if let Some(first) = head.first_mut() {
        *first = marked(first.clone(), "v1-signatures");
    }
    children.extend(head);
    if let (Version::V1(values), Some(spans)) = (&message.version, config) {
        children.extend(config_rows(values, spans));
    }
    children.push(own(
        "recent_blockhash",
        Value::Hash(message.blockhash.to_vec()),
        *blockhash,
    ));
    let key_list = group(
        "account_keys",
        Value::List,
        key_rows(&message.keys, keys, |index| walk.role(index)),
    )
    .framed(vec![*key_framing]);
    children.push(marked(key_list, "v1-addresses"));
    let instruction_list = group("instructions", Value::List, instruction_rows).framed(vec![*instruction_framing]);
    children.push(marked(instruction_list, "v1-instructions"));
    if let Some(framing) = lookup_framing {
        let rows = message
            .lookups
            .iter()
            .zip(lookups)
            .map(|(lookup, spans)| lookup_rows(lookup, spans));
        let list = group("address_table_lookups", Value::List, rows.collect()).framed(vec![*framing]);
        children.push(if message.lookups.is_empty() {
            list
        } else {
            list.action("lookup-table")
        });
    }
    Ok(Walked {
        root: group("solana_message", Value::Struct, children),
        round_trips: walk.checks,
    })
}

fn walk_partial(partial: &PartialMessage, payload: &[u8]) -> Result<Walked, Reject> {
    let versions = &schema().versions;
    let PartialMessage {
        required_signatures,
        readonly_signed,
        readonly_unsigned,
        mask,
        blockhash,
        instruction_count,
        keys,
        version,
        header,
        mask_span,
        blockhash_span,
        instruction_count_span,
        key_framing,
        key_spans,
        rest,
    } = partial;
    let body = payload
        .get(rest.offset as usize..rest.end() as usize)
        .ok_or(Reject::Malformed)?;
    let mut rebuilt = vec![versions.v1, *required_signatures, *readonly_signed, *readonly_unsigned];
    rebuilt.extend_from_slice(&mask.to_le_bytes());
    rebuilt.extend_from_slice(blockhash);
    rebuilt.extend_from_slice(&[*instruction_count, keys.len() as u8]);
    keys.iter().for_each(|key| rebuilt.extend_from_slice(key));
    rebuilt.extend_from_slice(body);
    let signed = *required_signatures as usize;
    let unlocked = signed - *readonly_signed as usize;
    let role = |index: usize| {
        let roles = &schema().roles;
        let writable = if index < signed {
            index < unlocked
        } else {
            index < keys.len() - *readonly_unsigned as usize
        };
        let label = match (index < signed, writable) {
            (true, true) => &roles.signer_writable,
            (true, false) => &roles.signer_readonly,
            (false, true) => &roles.writable,
            (false, false) => &roles.readonly,
        };
        Node::new("role", Value::Enum(label.clone()), Source::Derived)
    };
    let mut children = vec![own("message_version", Value::Enum(versions.v1_name.clone()), *version)];
    children.extend(header_nodes(
        [*required_signatures, *readonly_signed, *readonly_unsigned],
        *header,
    ));
    children.push(own("transaction_config_mask", uint(u64::from(*mask)), *mask_span).action("v1-config-unknown"));
    children.push(own(
        "recent_blockhash",
        Value::Hash(blockhash.to_vec()),
        *blockhash_span,
    ));
    children.push(own(
        "num_instructions",
        uint(u64::from(*instruction_count)),
        *instruction_count_span,
    ));
    children.push(group("account_keys", Value::List, key_rows(keys, key_spans, role)).framed(vec![*key_framing]));
    children.push(own("rest", Value::Opaque(body.to_vec()), *rest));
    Ok(Walked {
        root: group("solana_message", Value::Struct, children),
        round_trips: vec![RoundTrip {
            original: payload.to_vec(),
            rebuilt,
        }],
    })
}

fn format_row(field: &Field<'_>) -> Result<Node, Reject> {
    let label = schema()
        .offchain
        .formats
        .get(format_byte(field)? as usize)
        .ok_or(Reject::Malformed)?;
    Ok(own("message_format", Value::Enum(label.clone()), field.span))
}

fn text_row(field: &Field<'_>) -> Result<Node, Reject> {
    let text = std::str::from_utf8(field.bytes).map_err(|_| Reject::InvalidText)?;
    Ok(own("message", Value::Text(text.to_string()), field.span))
}

fn signer_rows(fields: &[Field<'_>]) -> Vec<Node> {
    fields
        .iter()
        .map(|field| own("signer", Value::Address(base58::encode(field.bytes)), field.span))
        .collect()
}

/// 每一段都从读出的结果重新编码.长度由文本算出,不抄原字节.
fn rebuilt_length(message: &Field<'_>) -> [u8; 2] {
    (message.bytes.len() as u16).to_le_bytes()
}

fn walk_offchain(offchain: &Offchain<'_>, payload: &[u8]) -> Result<Walked, Reject> {
    let Offchain {
        domain,
        version,
        body,
        signers: _signers,
    } = offchain;
    let mut rebuilt = [domain.bytes, version.bytes].concat();
    let version_row = own("offchain_version", uint(u64::from(format_byte(version)?)), version.span);
    let mut framing = vec![domain.span];
    let mut children = vec![version_row];
    let action = match body {
        OffchainBody::Ledger {
            format,
            length,
            message,
        } => {
            framing.push(length.span);
            rebuilt.extend_from_slice(format.bytes);
            rebuilt.extend_from_slice(&rebuilt_length(message));
            rebuilt.extend_from_slice(message.bytes);
            children.push(format_row(format)?);
            children.push(text_row(message)?);
            "offchain-message"
        }
        OffchainBody::Preamble {
            application_domain,
            format,
            count,
            signers,
            length,
            message,
        } => {
            framing.extend([count.span, length.span]);
            rebuilt.extend_from_slice(application_domain.bytes);
            rebuilt.extend_from_slice(format.bytes);
            rebuilt.push(signers.len() as u8);
            signers.iter().for_each(|field| rebuilt.extend_from_slice(field.bytes));
            rebuilt.extend_from_slice(&rebuilt_length(message));
            rebuilt.extend_from_slice(message.bytes);
            children.push(own(
                "application_domain",
                Value::Address(base58::encode(application_domain.bytes)),
                application_domain.span,
            ));
            children.push(format_row(format)?);
            children.push(group("signers", Value::List, signer_rows(signers)));
            children.push(text_row(message)?);
            "offchain-message"
        }
        OffchainBody::Signers {
            count,
            signers,
            message,
        } => {
            framing.push(count.span);
            rebuilt.push(signers.len() as u8);
            signers.iter().for_each(|field| rebuilt.extend_from_slice(field.bytes));
            rebuilt.extend_from_slice(message.bytes);
            children.push(group("signers", Value::List, signer_rows(signers)));
            children.push(text_row(message)?);
            "offchain-message"
        }
        OffchainBody::Future { data } => {
            rebuilt.extend_from_slice(data.bytes);
            children.push(own("data", Value::Opaque(data.bytes.to_vec()), data.span));
            "offchain-unknown"
        }
    };
    let root = group("offchain_message", Value::Struct, children).framed(framing);
    Ok(Walked {
        root: root.action(action),
        round_trips: vec![RoundTrip {
            original: payload.to_vec(),
            rebuilt,
        }],
    })
}

/// `coin` 是原生币,本机不认识的网络没有.`hint` 是网页端声明的收款方所有者.
#[derive(Clone, Copy, Debug)]
pub struct SolanaWalker {
    pub coin: Option<Coin>,
    pub hint: Option<[u8; 32]>,
}

impl Decoder for SolanaWalker {
    fn walk(&self, input: &Input<'_>) -> Result<Option<Walked>, Reject> {
        match parse(input.payload)? {
            Parsed::Message(parsed) => walk_message(&parsed, input.payload, self).map(Some),
            Parsed::Partial(partial) => walk_partial(&partial, input.payload).map(Some),
            Parsed::Offchain(offchain) => walk_offchain(&offchain, input.payload).map(Some),
            Parsed::Unknown => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{Decoded, Failure, Kind, decode, display};
    use crate::{Chain, Mode};

    const WALKER: SolanaWalker = SolanaWalker {
        coin: Some(Coin {
            unit: "SOL",
            decimals: 9,
        }),
        hint: None,
    };
    const PAYER: [u8; 32] = [0x11; 32];
    const OTHER: [u8; 32] = [0x22; 32];
    const THIRD: [u8; 32] = [0x33; 32];
    const SYSTEM: [u8; 32] = [0; 32];
    const UNKNOWN_PROGRAM: [u8; 32] = [0x55; 32];

    fn run(payload: &[u8]) -> Result<Decoded, Failure> {
        decode(
            &WALKER,
            &Input {
                chain: Chain::Solana,
                mode: Some(Mode::Mainnet),
                network: "solana",
                kind: 0,
                paths: &[],
                payload,
                signer: None,
            },
        )
    }

    fn shown(decoded: &Decoded, name: &str) -> Vec<String> {
        let visits = decoded.visits();
        let rows = visits.iter().filter(|visit| visit.node.name == name);
        rows.map(|visit| display(&visit.node.value)).collect()
    }

    fn kinds(decoded: &Decoded) -> Vec<String> {
        decoded.actions().iter().map(|(_, _, kind)| kind.to_string()).collect()
    }

    fn message(version: Version, program: [u8; 32], accounts: Vec<u8>, data: Vec<u8>) -> Message {
        Message {
            version,
            required_signatures: 1,
            readonly_signed: 0,
            readonly_unsigned: 1,
            keys: vec![PAYER, OTHER, program],
            blockhash: [7; 32],
            instructions: vec![Instruction {
                program: 2,
                accounts,
                data,
            }],
            lookups: Vec::new(),
        }
    }

    fn transfer_data(lamports: u64) -> Vec<u8> {
        [&2u32.to_le_bytes()[..], &lamports.to_le_bytes()].concat()
    }

    fn transfer(version: Version) -> Message {
        message(version, SYSTEM, vec![0, 1], transfer_data(1_500_000_000))
    }

    #[test]
    fn compact_u16_edges() {
        let cases: [(usize, &[u8]); 5] = [
            (0, &[0]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (16383, &[0xff, 0x7f]),
            (65535, &[0xff, 0xff, 0x03]),
        ];
        for (value, bytes) in cases {
            let mut out = Vec::new();
            put_len(&mut out, value);
            assert_eq!(out, bytes);
            let field = Field::whole(&out);
            assert_eq!(read_len(&mut Cursor::new(&field)).unwrap().0, value);
        }
        // 超过 16 位和 4 字节编码都不合法.
        for bad in [&[0xff, 0xff, 0x04][..], &[0x80, 0x80, 0x80, 0x01]] {
            let field = Field::whole(bad);
            assert_eq!(read_len(&mut Cursor::new(&field)), Err(Reject::Malformed));
        }
    }

    #[test]
    fn every_version_decodes_the_same_instruction() {
        let config = V1Config {
            priority_fee: Some(9),
            compute_unit_limit: Some(1000),
            loaded_accounts_data_size_limit: Some(4096),
            heap_size: Some(32768),
        };
        for version in [
            Version::Legacy,
            Version::V0,
            Version::V1(config),
            Version::V1(V1Config::default()),
        ] {
            let decoded = run(&transfer(version).encode()).unwrap();
            assert_eq!(kinds(&decoded), ["system.transferSol"], "{version:?}");
            assert_eq!(shown(&decoded, "instruction_name"), ["transferSol"]);
            assert_eq!(shown(&decoded, "amount"), ["1.5 SOL"]);
            assert_eq!(shown(&decoded, "source"), [base58::encode(&PAYER)]);
            assert_eq!(shown(&decoded, "destination"), [base58::encode(&OTHER)]);
            assert_eq!(decoded.node_count() as usize, decoded.visits().len());
        }
    }

    /// 字段的顺序来自 SIMD-0385,字节是手写的,不经过 `encode`.
    #[test]
    fn v1_follows_the_wire_layout() {
        let mut bytes = vec![0x81, 1, 0, 1];
        bytes.extend_from_slice(&0b1_0111u32.to_le_bytes());
        bytes.extend_from_slice(&[7; 32]);
        bytes.extend_from_slice(&[1, 3]);
        for key in [PAYER, OTHER, SYSTEM] {
            bytes.extend_from_slice(&key);
        }
        bytes.extend_from_slice(&5000u64.to_le_bytes());
        bytes.extend_from_slice(&200_000u32.to_le_bytes());
        bytes.extend_from_slice(&65536u32.to_le_bytes());
        bytes.extend_from_slice(&[2, 2, 12, 0]);
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&transfer_data(1));
        let decoded = run(&bytes).unwrap();
        assert_eq!(shown(&decoded, "message_version"), ["v1"]);
        assert_eq!(shown(&decoded, "config_priority_fee"), ["5000"]);
        assert_eq!(shown(&decoded, "compute_unit_limit"), ["200000"]);
        assert_eq!(shown(&decoded, "heap_size"), ["65536"]);
        assert_eq!(shown(&decoded, "priority_fee"), ["0.000005 SOL"]);
        assert_eq!(kinds(&decoded), ["system.transferSol"]);
    }

    #[test]
    fn v1_ignores_compute_budget_instructions_for_the_fee() {
        let budget = program_key("ComputeBudget111111111111111111111111111111").unwrap();
        let price = [&[3u8][..], &1_000_000u64.to_le_bytes()].concat();
        let config = V1Config {
            priority_fee: Some(7),
            ..V1Config::default()
        };
        let legacy = run(&message(Version::Legacy, budget, Vec::new(), price.clone()).encode()).unwrap();
        assert_eq!(shown(&legacy, "priority_fee"), ["0.0002 SOL"]);
        let v1 = run(&message(Version::V1(config), budget, Vec::new(), price).encode()).unwrap();
        assert_eq!(shown(&v1, "priority_fee"), ["0.000000007 SOL"]);
    }

    #[test]
    fn a_half_priority_fee_mask_is_refused() {
        let mut bytes = transfer(Version::V1(V1Config::default())).encode();
        bytes[4] = 0b01;
        assert!(run(&bytes).is_err());
    }

    #[test]
    fn unknown_mask_bits_keep_the_prefix_and_show_the_rest_as_opaque() {
        let mut bytes = transfer(Version::V1(V1Config::default())).encode();
        bytes[4] = 0b10_0000;
        let decoded = run(&bytes).unwrap();
        assert_eq!(decoded.root().name, "solana_message");
        assert_ne!(decoded.root().value.kind(), Kind::Opaque);
        assert_eq!(kinds(&decoded), ["v1-config-unknown"]);
        assert_eq!(shown(&decoded, "account_key").len(), 3);
        assert_eq!(shown(&decoded, "recent_blockhash"), ["07".repeat(32)]);
        assert_eq!(shown(&decoded, "rest").len(), 1);
        assert!(matches!(parse_solana(&bytes), Ok(SolanaPayload::Partial { .. })));
    }

    #[test]
    fn v1_limits_warn_and_do_not_refuse() {
        let mut message = transfer(Version::V1(V1Config::default()));
        message.instructions[0].data.extend(vec![0; 5000]);
        let decoded = run(&message.encode()).unwrap();
        assert!(kinds(&decoded).iter().any(|kind| kind == "v1-transaction-size"));
        let heap = V1Config {
            heap_size: Some(1000),
            ..V1Config::default()
        };
        let decoded = run(&transfer(Version::V1(heap)).encode()).unwrap();
        assert!(kinds(&decoded).iter().any(|kind| kind == "v1-heap-size"));
    }

    #[test]
    fn unknown_versions_are_a_single_opaque_root() {
        for first in [0x82, 0xa0, 0xfe, 0xff] {
            let decoded = run(&[first, 1, 2]).unwrap();
            assert_eq!(decoded.root().value.kind(), Kind::Opaque, "{first:#x}");
        }
    }

    /// `test_offchain_message_ascii`,`test_offchain_message_utf8`,agave 的 `solana-offchain-message`.
    const LEDGER_ASCII: [u8; 32] = [
        255, 115, 111, 108, 97, 110, 97, 32, 111, 102, 102, 99, 104, 97, 105, 110, 0, 0, 12, 0, 84, 101, 115, 116, 32,
        77, 101, 115, 115, 97, 103, 101,
    ];
    const LEDGER_UTF8: [u8; 55] = [
        255, 115, 111, 108, 97, 110, 97, 32, 111, 102, 102, 99, 104, 97, 105, 110, 0, 1, 35, 0, 208, 162, 208, 181,
        209, 129, 209, 130, 208, 190, 208, 178, 208, 190, 208, 181, 32, 209, 129, 208, 190, 208, 190, 208, 177, 209,
        137, 208, 181, 208, 189, 208, 184, 208, 181,
    ];

    fn domain() -> Vec<u8> {
        LEDGER_ASCII[..16].to_vec()
    }

    fn preamble(application: [u8; 32], format: u8, signers: &[[u8; 32]], message: &[u8]) -> Vec<u8> {
        let mut bytes = domain();
        bytes.push(0);
        bytes.extend_from_slice(&application);
        bytes.extend_from_slice(&[format, signers.len() as u8]);
        signers.iter().for_each(|key| bytes.extend_from_slice(key));
        bytes.extend_from_slice(&(message.len() as u16).to_le_bytes());
        bytes.extend_from_slice(message);
        bytes
    }

    fn sorted_signers(signers: &[[u8; 32]], message: &[u8]) -> Vec<u8> {
        let mut bytes = domain();
        bytes.extend_from_slice(&[1, signers.len() as u8]);
        signers.iter().for_each(|key| bytes.extend_from_slice(key));
        bytes.extend_from_slice(message);
        bytes
    }

    #[test]
    fn the_ledger_offchain_format_matches_the_agave_vectors() {
        let decoded = run(&LEDGER_ASCII).unwrap();
        assert_eq!(decoded.root().name, "offchain_message");
        assert_eq!(shown(&decoded, "message"), ["Test Message"]);
        assert_eq!(shown(&decoded, "message_format"), ["restricted-ascii"]);
        assert_eq!(kinds(&decoded), ["offchain-message"]);
        let decoded = run(&LEDGER_UTF8).unwrap();
        assert_eq!(shown(&decoded, "message"), ["Тестовое сообщение"]);
        assert_eq!(shown(&decoded, "message_format"), ["limited-utf8"]);
    }

    #[test]
    fn the_preamble_offchain_format_names_its_signers() {
        let payload = preamble([0xaa; 32], 0, &[PAYER, OTHER], b"hello");
        let decoded = run(&payload).unwrap();
        assert_eq!(shown(&decoded, "message"), ["hello"]);
        assert_eq!(
            shown(&decoded, "signer"),
            [base58::encode(&PAYER), base58::encode(&OTHER)]
        );
        assert_eq!(shown(&decoded, "application_domain").len(), 1);
        let SolanaPayload::Offchain { signers } = parse_solana(&payload).unwrap() else {
            panic!("不是链下消息");
        };
        assert_eq!(signers, Some(vec![PAYER, OTHER]));
    }

    #[test]
    fn version_one_offchain_messages_sort_their_signers() {
        let decoded = run(&sorted_signers(&[PAYER, OTHER], b"hello")).unwrap();
        assert_eq!(shown(&decoded, "message"), ["hello"]);
        assert!(run(&sorted_signers(&[OTHER, PAYER], b"hello")).is_err());
        assert!(run(&sorted_signers(&[PAYER, PAYER], b"hello")).is_err());
        assert!(run(&sorted_signers(&[], b"hello")).is_err());
        assert!(run(&sorted_signers(&[PAYER], b"")).is_err());
    }

    #[test]
    fn a_later_offchain_version_keeps_its_domain() {
        let mut bytes = domain();
        bytes.extend_from_slice(&[9, 1, 2, 3]);
        let decoded = run(&bytes).unwrap();
        assert_eq!(decoded.root().name, "offchain_message");
        assert_ne!(decoded.root().value.kind(), Kind::Opaque);
        assert_eq!(kinds(&decoded), ["offchain-unknown"]);
        assert_eq!(shown(&decoded, "data"), ["010203"]);
    }

    #[test]
    fn a_malformed_offchain_message_never_becomes_opaque() {
        let bad_length = {
            let mut bytes = LEDGER_ASCII.to_vec();
            bytes[18] = 13;
            bytes
        };
        let bad_format = {
            let mut bytes = LEDGER_ASCII.to_vec();
            bytes[17] = 3;
            bytes
        };
        let not_ascii = {
            let mut bytes = LEDGER_ASCII.to_vec();
            bytes[20] = 0x07;
            bytes
        };
        let not_utf8 = preamble([0xaa; 32], 1, &[PAYER], &[0xff, 0xfe]);
        let no_signer = preamble([0xaa; 32], 0, &[], b"hello");
        for payload in [domain(), bad_length, bad_format, not_ascii, not_utf8, no_signer] {
            assert!(run(&payload).is_err(), "{payload:02x?}");
        }
    }

    /// 只有完整的 `\xffsolana offchain` 才是这个签名域.
    #[test]
    fn a_partial_domain_is_an_unknown_format() {
        let partial = &LEDGER_ASCII[..10];
        assert_eq!(run(partial).unwrap().root().value.kind(), Kind::Opaque);
        let mut altered = LEDGER_ASCII;
        altered[3] ^= 1;
        assert_eq!(run(&altered).unwrap().root().value.kind(), Kind::Opaque);
    }

    /// 同时符合两种 v0 的字节,不同的解析器结果不同.
    #[test]
    fn an_offchain_message_valid_in_both_layouts_is_refused() {
        // 应用域的开头是 Ledger 格式的头部,声明的长度正好到消息末尾.
        let body_len = 32 + 2 + 32 + 2 + 5;
        let mut application = [0x41; 32];
        application[0] = 1;
        application[1..3].copy_from_slice(&((body_len - 3) as u16).to_le_bytes());
        let payload = preamble(application, 1, &[[0x41; 32]], b"hello");
        assert!(run(&payload).is_err());
    }

    #[test]
    fn unknown_programs_keep_every_account_and_show_the_data() {
        let decoded = run(&message(Version::Legacy, UNKNOWN_PROGRAM, vec![1, 0], vec![0xde, 0xad]).encode()).unwrap();
        assert_eq!(kinds(&decoded), ["program-unknown"]);
        assert_eq!(shown(&decoded, "account").len(), 2);
        assert_eq!(shown(&decoded, "data"), ["", "dead"]);
    }

    #[test]
    fn an_unknown_instruction_of_a_known_program_is_opaque() {
        let decoded = run(&message(Version::Legacy, SYSTEM, vec![0], vec![99, 0, 0, 0]).encode()).unwrap();
        assert_eq!(kinds(&decoded), ["instruction-unknown"]);
    }

    #[test]
    fn trailing_bytes_and_extra_accounts_are_opaque_and_c1_holds() {
        let mut data = transfer_data(1);
        data.push(0xaa);
        let decoded = run(&message(Version::Legacy, SYSTEM, vec![0, 1, 1], data).encode()).unwrap();
        assert_eq!(shown(&decoded, "trailing_data"), ["aa"]);
        assert_eq!(shown(&decoded, "extra_account"), ["01"]);
        assert_eq!(shown(&decoded, "amount"), ["0.000000001 SOL"]);
    }

    #[test]
    fn fewer_accounts_than_the_idl_requires_is_a_decode_failure() {
        assert!(run(&message(Version::Legacy, SYSTEM, vec![0], transfer_data(1)).encode()).is_err());
    }

    #[test]
    fn data_shorter_than_the_idl_is_opaque_not_guessed() {
        let decoded = run(&message(Version::Legacy, SYSTEM, vec![0, 1], vec![2, 0, 0, 0, 1]).encode()).unwrap();
        assert_eq!(kinds(&decoded), ["system.transferSol"]);
        assert_eq!(shown(&decoded, "data"), ["", "0200000001"]);
        assert!(shown(&decoded, "amount").is_empty());
    }

    #[test]
    fn lookup_accounts_show_the_table_and_the_index_and_no_address() {
        let mut lookup = transfer(Version::V0);
        lookup.lookups = vec![Lookup {
            table: THIRD,
            writable: vec![4, 5],
            readonly: vec![6],
        }];
        lookup.instructions[0].accounts = vec![0, 5];
        let decoded = run(&lookup.encode()).unwrap();
        assert_eq!(shown(&decoded, "destination"), ["查找表地址"]);
        assert_eq!(shown(&decoded, "lookup_table"), [base58::encode(&THIRD)]);
        assert_eq!(shown(&decoded, "lookup_index"), ["6"]);
        assert_eq!(shown(&decoded, "table"), [base58::encode(&THIRD)]);
        assert_eq!(shown(&decoded, "index"), ["4", "5", "6"]);
        assert!(kinds(&decoded).iter().any(|kind| kind == "lookup-table"));
        assert!(kinds(&decoded).iter().any(|kind| kind == "lookup-account"));
    }

    #[test]
    fn accounts_carry_their_marks() {
        let decoded = run(&transfer(Version::Legacy).encode()).unwrap();
        assert_eq!(
            shown(&decoded, "role"),
            ["签名者,可写", "可写", "只读", "签名者,可写", "可写"]
        );
    }

    fn token_program() -> [u8; 32] {
        program_key(&schema().token_programs[0]).unwrap()
    }

    fn revoke(accounts: u8) -> Vec<u8> {
        vec![accounts, 1, 5]
    }

    #[test]
    fn batch_decodes_its_inner_instructions_with_their_accounts() {
        let data = [&[255][..], &revoke(2), &revoke(2)].concat();
        let mut batch = message(Version::Legacy, token_program(), vec![1, 0, 1, 0], data);
        batch.keys.push(THIRD);
        let decoded = run(&batch.encode()).unwrap();
        assert_eq!(kinds(&decoded), ["token.batch", "token.revoke", "token.revoke"]);
        assert_eq!(shown(&decoded, "instruction_name"), ["batch", "revoke", "revoke"]);
        assert_eq!(shown(&decoded, "source").len(), 2);
    }

    #[test]
    fn a_batch_cannot_nest_or_run_out_of_accounts() {
        let nested = [&[255][..], &[0, 1, 255]].concat();
        assert!(run(&message(Version::Legacy, token_program(), vec![], nested).encode()).is_err());
        let short = [&[255][..], &revoke(2)].concat();
        assert!(run(&message(Version::Legacy, token_program(), vec![1], short).encode()).is_err());
    }

    #[test]
    fn leftover_batch_accounts_are_opaque() {
        let data = [&[255][..], &revoke(2)].concat();
        let decoded = run(&message(Version::Legacy, token_program(), vec![1, 0, 1], data).encode()).unwrap();
        assert_eq!(shown(&decoded, "extra_account"), ["01"]);
    }

    #[test]
    fn an_empty_associated_token_instruction_is_create() {
        let associated = program_key(&schema().associated_token_program).unwrap();
        let decoded = run(&message(Version::Legacy, associated, vec![0, 1, 0, 1, 0, 1], Vec::new()).encode()).unwrap();
        assert_eq!(kinds(&decoded), ["associatedTokenAccount.create"]);
    }

    #[test]
    fn set_authority_shows_the_enum_and_the_option() {
        let mut data = vec![6, 2, 1];
        data.extend_from_slice(&THIRD);
        let decoded = run(&message(Version::Legacy, token_program(), vec![1, 0], data).encode()).unwrap();
        assert_eq!(shown(&decoded, "authorityType"), ["accountOwner"]);
        assert_eq!(shown(&decoded, "newAuthority"), [base58::encode(&THIRD)]);
        let decoded = run(&message(Version::Legacy, token_program(), vec![1, 0], vec![6, 3, 0]).encode()).unwrap();
        assert_eq!(shown(&decoded, "newAuthority"), ["无"]);
    }

    #[test]
    fn a_bad_option_tag_is_opaque() {
        let decoded = run(&message(Version::Legacy, token_program(), vec![1, 0], vec![6, 3, 2]).encode()).unwrap();
        assert_eq!(shown(&decoded, "data"), ["", "060302"]);
    }

    #[test]
    fn the_hint_becomes_derived_only_when_it_derives_the_destination() {
        let mint = THIRD;
        let program = token_program();
        let destination = associated_token_address(&OTHER, &mint, &program).unwrap();
        let transfer = Message {
            version: Version::Legacy,
            required_signatures: 1,
            readonly_signed: 0,
            readonly_unsigned: 2,
            keys: vec![PAYER, destination, mint, program],
            blockhash: [7; 32],
            instructions: vec![Instruction {
                program: 3,
                accounts: vec![1, 2, 1, 0],
                data: [&[12][..], &5u64.to_le_bytes(), &[6]].concat(),
            }],
            lookups: Vec::new(),
        };
        let walked = |hint| {
            decode(
                &SolanaWalker { hint, ..WALKER },
                &Input {
                    chain: Chain::Solana,
                    mode: Some(Mode::Mainnet),
                    network: "solana",
                    kind: 0,
                    paths: &[],
                    payload: &transfer.encode(),
                    signer: None,
                },
            )
            .unwrap()
        };
        assert_eq!(shown(&walked(Some(OTHER)), "recipient_owner"), [base58::encode(&OTHER)]);
        assert!(shown(&walked(Some(PAYER)), "recipient_owner").is_empty());
        assert!(shown(&walked(None), "recipient_owner").is_empty());
    }

    #[test]
    fn invalid_utf8_in_a_memo_is_a_decode_failure() {
        let memo = program_key("Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo").unwrap();
        let decoded = run(&message(Version::Legacy, memo, Vec::new(), b"hello".to_vec()).encode()).unwrap();
        assert_eq!(shown(&decoded, "memo"), ["hello"]);
        let failure = run(&message(Version::Legacy, memo, Vec::new(), vec![0xff]).encode()).unwrap_err();
        assert_eq!(failure.reason, Reject::InvalidText);
    }

    #[test]
    fn the_token_2022_authority_enum_lists_every_type() {
        let program = solana_idl::programs()
            .iter()
            .find(|program| program.name == "token2022")
            .unwrap();
        let types: Vec<usize> = (0..18)
            .map(|tag| {
                let data = vec![6, tag, 0];
                let decoded = run(&message(
                    Version::Legacy,
                    program_key("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap(),
                    vec![1, 0],
                    data,
                )
                .encode())
                .unwrap();
                shown(&decoded, "authorityType").len()
            })
            .collect();
        assert_eq!(program.name, "token2022");
        assert_eq!(types, [1; 18]);
    }

    /// 每个字节都有归属.改一个字节,要么拒绝,要么有一个值变了.
    #[test]
    fn the_schema_tables_parse_and_cover_the_standard() {
        let count = |name: &str| {
            solana_idl::programs()
                .iter()
                .find(|program| program.name == name)
                .map(|program| program.instructions().len())
        };
        assert_eq!(count("system"), Some(14));
        assert_eq!(count("token"), Some(28));
        assert_eq!(count("token2022"), Some(100));
        assert_eq!(count("associatedTokenAccount"), Some(4));
        assert_eq!(count("computeBudget"), Some(5));
        assert_eq!(count("memo"), Some(1));
        assert_eq!(count("stake"), Some(17));
        assert_eq!(count("addressLookupTable"), Some(5));
        assert_eq!(count("config"), Some(1));
        assert_eq!(count("loaderV3"), Some(8));
        // Vote 没有官方 IDL,按官方 interface crate 解码.
        assert_eq!(count("vote"), None);
        // 10 个程序的 IDL,Stake 的旧布局,Loader v3 官方仓库的 Codama 配置.
        assert_eq!(idl_sources().len(), 12);
    }

    #[test]
    fn discriminators_do_not_collide() {
        for program in solana_idl::programs() {
            let mut names: Vec<&str> = program.instructions().iter().map(|ix| ix.name.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), program.instructions().len(), "{}", program.name);
        }
    }
}

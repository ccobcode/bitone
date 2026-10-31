//! Solana 交易:严格解析,分类,构建和核验.
//!
//! 载荷是消息字节,签名是消息的 ed25519 签名.解析后必须能重新编码回完全相同的字节,
//! 这样冷钱包展示的内容和实际签名的内容不可能不一致.

use crate::address::parse_solana_address;
use crate::manifest::{Asset, Manifest};
use crate::policy;
use crate::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use crate::review::{ActionView, AmountView, Forbid, RawLine, Review, ReviewBuilder, Risk, Row};
use crate::tx::{ReviewContext, Verified, raw_lines, signer_path};
use crate::units::format_units;
use crate::{Chain, Error, Network};
use bitcoin::hex::DisplayHex;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

pub const SYSTEM_PROGRAM: [u8; 32] = [0; 32];
pub const TOKEN_PROGRAM: [u8; 32] = [
    0x06, 0xdd, 0xf6, 0xe1, 0xd7, 0x65, 0xa1, 0x93, 0xd9, 0xcb, 0xe1, 0x46, 0xce, 0xeb, 0x79, 0xac, 0x1c, 0xb4, 0x85,
    0xed, 0x5f, 0x5b, 0x37, 0x91, 0x3a, 0x8c, 0xf5, 0x85, 0x7e, 0xff, 0x00, 0xa9,
];
pub const TOKEN_2022_PROGRAM: [u8; 32] = [
    0x06, 0xdd, 0xf6, 0xe1, 0xee, 0x75, 0x8f, 0xde, 0x18, 0x42, 0x5d, 0xbc, 0xe4, 0x6c, 0xcd, 0xda, 0xb6, 0x1a, 0xfc,
    0x4d, 0x83, 0xb9, 0x0d, 0x27, 0xfe, 0xbd, 0xf9, 0x28, 0xd8, 0xa1, 0x8b, 0xfc,
];
pub const ASSOCIATED_TOKEN_PROGRAM: [u8; 32] = [
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83, 0x0b, 0x5a, 0x13,
    0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
];
pub const COMPUTE_BUDGET_PROGRAM: [u8; 32] = [
    0x03, 0x06, 0x46, 0x6f, 0xe5, 0x21, 0x17, 0x32, 0xff, 0xec, 0xad, 0xba, 0x72, 0xc3, 0x9b, 0xe7, 0xbc, 0x8c, 0xe5,
    0xbb, 0xc5, 0xf7, 0x12, 0x6b, 0x2c, 0x43, 0x9b, 0x3a, 0x40, 0x00, 0x00, 0x00,
];
pub const RECENT_BLOCKHASHES_SYSVAR: [u8; 32] = [
    0x06, 0xa7, 0xd5, 0x17, 0x19, 0x2c, 0x56, 0x8e, 0xe0, 0x8a, 0x84, 0x5f, 0x73, 0xd2, 0x97, 0x88, 0xcf, 0x03, 0x5c,
    0x31, 0x45, 0xb2, 0x1a, 0xb3, 0x44, 0xd8, 0x06, 0x2e, 0xa9, 0x40, 0x00, 0x00,
];
pub const RENT_SYSVAR: [u8; 32] = [
    0x06, 0xa7, 0xd5, 0x17, 0x19, 0x2c, 0x5c, 0x51, 0x21, 0x8c, 0xc9, 0x4c, 0x3d, 0x4a, 0xf1, 0x7f, 0x58, 0xda, 0xee,
    0x08, 0x9b, 0xa1, 0xfd, 0x44, 0xe3, 0xdb, 0xd9, 0x8a, 0x00, 0x00, 0x00, 0x00,
];

const NONCE_SEED: &str = "bitone-nonce";
const CREATE_NONCE_TITLE: &str = "创建 nonce 账户";
const ADVANCE_NONCE_TITLE: &str = "作废已签名的交易";
const CREATE_TOKEN_ACCOUNT_TITLE: &str = "创建代币账户";
/// nonce 账户的数据长度.
const NONCE_SPACE: u64 = 80;
const LAMPORTS_PER_SIGNATURE: u64 = 5000;
/// 没写 SetComputeUnitLimit 时每条指令的上限,算优先费时取这个偏高的值.
const DEFAULT_UNITS_PER_INSTRUCTION: u64 = 200_000;
const MAX_COMPUTE_UNITS: u64 = 1_400_000;
const MICRO_LAMPORTS_PER_LAMPORT: u128 = 1_000_000;

fn base58(key: &[u8; 32]) -> String {
    bitcoin::base58::encode(key)
}

fn take<const N: usize>(buf: &mut &[u8]) -> Result<[u8; N], Forbid> {
    let (head, rest) = buf.split_first_chunk::<N>().ok_or(Forbid::ParseFailed)?;
    *buf = rest;
    Ok(*head)
}

fn read_len(buf: &mut &[u8]) -> Result<usize, Forbid> {
    let mut value = 0usize;
    for shift in [0, 7, 14] {
        let [byte] = take::<1>(buf)?;
        value |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return if value > u16::MAX as usize {
                Err(Forbid::ParseFailed)
            } else {
                Ok(value)
            };
        }
    }
    Err(Forbid::ParseFailed)
}

fn put_len(out: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_bytes(buf: &mut &[u8]) -> Result<Vec<u8>, Forbid> {
    let len = read_len(buf)?;
    if len > buf.len() {
        return Err(Forbid::ParseFailed);
    }
    let (value, rest) = buf.split_at(len);
    *buf = rest;
    Ok(value.to_vec())
}

fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    put_len(out, value.len());
    out.extend_from_slice(value);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Legacy,
    V0,
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

impl Message {
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        let mut buf = payload;
        let first = *buf.first().ok_or(Forbid::ParseFailed)?;
        let version = match first {
            0x80 => {
                buf = &buf[1..];
                Version::V0
            }
            0x81..=0xff => return Err(Forbid::Unsupported),
            _ => Version::Legacy,
        };
        let [required_signatures, readonly_signed, readonly_unsigned] = take(&mut buf)?;
        let keys = (0..read_len(&mut buf)?)
            .map(|_| take(&mut buf))
            .collect::<Result<_, _>>()?;
        let blockhash = take(&mut buf)?;
        let instructions = (0..read_len(&mut buf)?)
            .map(|_| {
                let [program] = take(&mut buf)?;
                Ok(Instruction {
                    program,
                    accounts: read_bytes(&mut buf)?,
                    data: read_bytes(&mut buf)?,
                })
            })
            .collect::<Result<_, Forbid>>()?;
        let lookups = match version {
            Version::Legacy => Vec::new(),
            Version::V0 => (0..read_len(&mut buf)?)
                .map(|_| {
                    Ok(Lookup {
                        table: take(&mut buf)?,
                        writable: read_bytes(&mut buf)?,
                        readonly: read_bytes(&mut buf)?,
                    })
                })
                .collect::<Result<_, Forbid>>()?,
        };
        if !buf.is_empty() {
            return Err(Forbid::ParseFailed);
        }
        let message = Self {
            version,
            required_signatures,
            readonly_signed,
            readonly_unsigned,
            keys,
            blockhash,
            instructions,
            lookups,
        };
        message.check()?;
        if message.encode() != payload {
            return Err(Forbid::ParseFailed);
        }
        Ok(message)
    }

    /// 和节点的 sanitize 规则一致.节点不接受的消息,这里也不展示.
    fn check(&self) -> Result<(), Forbid> {
        let required = self.required_signatures as usize;
        let static_keys = self.keys.len();
        let total = static_keys + self.lookup_count();
        let header_fits = required >= 1
            && self.readonly_signed < self.required_signatures
            && required + self.readonly_unsigned as usize <= static_keys;
        if !header_fits || total > 256 {
            return Err(Forbid::ParseFailed);
        }
        let mut sorted = self.keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != static_keys {
            return Err(Forbid::ParseFailed);
        }
        if self
            .lookups
            .iter()
            .any(|l| l.writable.is_empty() && l.readonly.is_empty())
        {
            return Err(Forbid::ParseFailed);
        }
        for ix in &self.instructions {
            // 手续费支付者不能是程序,程序也不能来自查找表.
            let program = ix.program as usize;
            if program == 0 || program >= static_keys {
                return Err(Forbid::ParseFailed);
            }
            if ix.accounts.iter().any(|&index| index as usize >= total) {
                return Err(Forbid::ParseFailed);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.version == Version::V0 {
            out.push(0x80);
        }
        out.extend_from_slice(&[self.required_signatures, self.readonly_signed, self.readonly_unsigned]);
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

    /// 来自查找表的地址数量.
    pub fn lookup_count(&self) -> usize {
        self.lookups.iter().map(|l| l.writable.len() + l.readonly.len()).sum()
    }

    fn is_signer(&self, index: usize) -> bool {
        index < self.required_signatures as usize
    }

    fn is_writable(&self, index: usize) -> bool {
        let required = self.required_signatures as usize;
        let static_keys = self.keys.len();
        if index < required {
            index < required - self.readonly_signed as usize
        } else if index < static_keys {
            index < static_keys - self.readonly_unsigned as usize
        } else {
            let writable: usize = self.lookups.iter().map(|l| l.writable.len()).sum();
            index < static_keys + writable
        }
    }
}

pub fn nonce_account_address(payer: &[u8; 32]) -> Result<[u8; 32], Error> {
    Ok(Sha256::new()
        .chain_update(payer)
        .chain_update(NONCE_SEED)
        .chain_update(SYSTEM_PROGRAM)
        .finalize()
        .into())
}

/// 程序派生地址不能在曲线上,否则会有对应的私钥.
fn find_program_address(seeds: &[&[u8]], program: &[u8; 32]) -> Result<[u8; 32], Error> {
    for bump in (1..=u8::MAX).rev() {
        let mut hasher = Sha256::new();
        seeds.iter().for_each(|seed| hasher.update(seed));
        let address: [u8; 32] = hasher
            .chain_update([bump])
            .chain_update(program)
            .chain_update("ProgramDerivedAddress")
            .finalize()
            .into();
        if VerifyingKey::from_bytes(&address).is_err() {
            return Ok(address);
        }
    }
    Err(Error::InvalidParams)
}

pub fn associated_token_address(owner: &[u8; 32], mint: &[u8; 32], program: &[u8; 32]) -> Result<[u8; 32], Error> {
    find_program_address(&[owner, program, mint], &ASSOCIATED_TOKEN_PROGRAM)
}

fn le_u64(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(*bytes.first_chunk::<8>()?))
}

fn key_at(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.first_chunk::<32>().copied()
}

/// AllocateWithSeed.派生地址是公开的,任何人都能往里转一点钱,那样 CreateAccountWithSeed 就会失败,
/// 这个钱包再也发不出交易.所以分三步:先转入租金,再分配空间,最后初始化.
/// 分配要付款账户签名,攻击者只能存钱,抢不走这个地址.
fn allocate_nonce_data(payer: &[u8; 32]) -> Vec<u8> {
    let mut data = 9u32.to_le_bytes().to_vec();
    data.extend_from_slice(payer);
    data.extend_from_slice(&(NONCE_SEED.len() as u64).to_le_bytes());
    data.extend_from_slice(NONCE_SEED.as_bytes());
    data.extend_from_slice(&NONCE_SPACE.to_le_bytes());
    data.extend_from_slice(&SYSTEM_PROGRAM);
    data
}

fn initialize_nonce_data(authority: &[u8; 32]) -> Vec<u8> {
    let mut data = 6u32.to_le_bytes().to_vec();
    data.extend_from_slice(authority);
    data
}

/// 一条指令分析出来的结果.
struct Effect {
    risk: Risk,
    title: String,
    rows: Vec<Row>,
    amount: Option<AmountView>,
    counterparty: Option<(&'static str, String)>,
    /// 代币收款方的所有者没有确认.
    unconfirmed_owner: bool,
}

impl Effect {
    fn new(risk: Risk, title: &str) -> Self {
        Self {
            risk,
            title: title.to_string(),
            rows: Vec::new(),
            amount: None,
            counterparty: None,
            unconfirmed_owner: false,
        }
    }

    fn unknown(title: &str) -> Self {
        Self::new(Risk::Unknown, title)
    }
}

/// 一遍扫完全部指令.ComputeBudget 和本钱包的 AdvanceNonceAccount 不算动作,其余每条都有一个 `Effect`.
struct Scan<'a> {
    message: &'a Message,
    network: Network,
    manifest: &'a Manifest,
    native: &'a Asset,
    payer: [u8; 32],
    nonce: [u8; 32],
    /// 网页端声明的代币收款方所有者.只有推导得出目标账户时才采信.
    hint: Option<[u8; 32]>,
    unit_limit: Option<u32>,
    unit_price: Option<u64>,
    alerts: Vec<String>,
}

impl Scan<'_> {
    fn program(&self, ix: &Instruction) -> [u8; 32] {
        self.message.keys[ix.program as usize]
    }

    /// 查找表里的地址没有结果.
    fn account(&self, ix: &Instruction, position: usize) -> Option<[u8; 32]> {
        ix.accounts
            .get(position)
            .and_then(|&index| self.message.keys.get(index as usize))
            .copied()
    }

    fn shown(&self, ix: &Instruction, position: usize) -> String {
        match ix.accounts.get(position) {
            None => "缺失".to_string(),
            Some(&index) => self
                .message
                .keys
                .get(index as usize)
                .map_or_else(|| "查找表地址".to_string(), base58),
        }
    }

    fn uses_lookup(&self, ix: &Instruction) -> bool {
        ix.accounts
            .iter()
            .any(|&index| index as usize >= self.message.keys.len())
    }

    fn accounts_are(&self, ix: &Instruction, expected: &[[u8; 32]]) -> bool {
        ix.accounts.len() == expected.len()
            && expected
                .iter()
                .enumerate()
                .all(|(i, key)| self.account(ix, i).as_ref() == Some(key))
    }

    fn sol(&self, lamports: u64) -> String {
        format!(
            "{} {}",
            format_units(&lamports.to_string(), self.native.decimals),
            self.native.symbol
        )
    }

    fn sol_view(&self, lamports: u64) -> AmountView {
        AmountView {
            text: format_units(&lamports.to_string(), self.native.decimals),
            symbol: Some(self.native.symbol.clone()),
            asset: Some(self.native.key()),
            unlimited: false,
        }
    }

    /// 不在清单或小数位对不上时显示原始整数,不显示符号.
    fn token_view(&self, mint: Option<&[u8; 32]>, decimals: Option<u8>, amount: u64) -> AmountView {
        let asset = mint
            .and_then(|mint| self.manifest.lookup(self.network.id(), &base58(mint)))
            .filter(|asset| Some(asset.decimals) == decimals);
        match asset {
            Some(asset) => AmountView {
                text: format_units(&amount.to_string(), asset.decimals),
                symbol: Some(asset.symbol.clone()),
                asset: Some(asset.key()),
                unlimited: false,
            },
            None => AmountView {
                text: amount.to_string(),
                symbol: None,
                asset: None,
                unlimited: false,
            },
        }
    }

    fn details(&self, ix: &Instruction) -> Vec<Row> {
        let mut rows = vec![Row::mono("程序 ID", base58(&self.program(ix)))];
        for (position, &index) in ix.accounts.iter().enumerate() {
            let index = index as usize;
            let mut marks = Vec::new();
            if self.message.is_signer(index) {
                marks.push("签名者");
            }
            marks.push(if self.message.is_writable(index) {
                "可写"
            } else {
                "只读"
            });
            let label = format!("账户 {}({})", position + 1, marks.join(","));
            rows.push(Row::mono(&label, self.shown(ix, position)));
        }
        rows.push(Row::text("数据长度", format!("{} 字节", ix.data.len())));
        rows.push(Row::mono("数据", ix.data.to_lower_hex_string()));
        rows
    }

    fn permission_change(&self, title: &str, current: String, new: String) -> Effect {
        let mut effect = Effect::new(Risk::PermissionChange, title);
        effect.rows = vec![Row::mono("当前控制方", current), Row::mono("新控制方", new.clone())];
        effect.counterparty = Some(("新控制方", new));
        effect
    }

    /// 只有规范的形式才算普通转账.账户多余或数据有尾部字节时节点照样转账,所以金额和收款方都要显示.
    fn transfer_sol(&self, ix: &Instruction) -> Option<Effect> {
        let lamports = le_u64(ix.data.get(4..)?)?;
        let plain = ix.data.len() == 12 && ix.accounts.len() == 2 && !self.uses_lookup(ix);
        let to = self.shown(ix, 1);
        let mut effect = if plain {
            Effect::new(Risk::Transfer, Risk::Transfer.label())
        } else {
            Effect::unknown("不规范的转账")
        };
        effect.rows = vec![
            Row::text("金额", self.sol(lamports)),
            Row::mono("付款账户", self.shown(ix, 0)),
            Row::mono("收款地址", to.clone()),
        ];
        effect.amount = Some(self.sol_view(lamports));
        effect.counterparty = Some(("收款地址", to));
        Some(effect)
    }

    /// 系统程序用 bincode 解码,多余的尾部字节会被忽略.权限类指令按前缀判断,不能因为尾部字节降级.
    fn system(&self, ix: &Instruction) -> Effect {
        let Some((tag, rest)) = ix.data.split_first_chunk::<4>() else {
            return Effect::unknown("未知系统指令");
        };
        match (u32::from_le_bytes(*tag), key_at(rest)) {
            (1, Some(owner)) => self.permission_change("Assign", self.shown(ix, 0), base58(&owner)),
            (2, _) => self.transfer_sol(ix).unwrap_or_else(|| Effect::unknown("系统转账")),
            (7, Some(authority)) => {
                let mut effect = self.permission_change("AuthorizeNonceAccount", self.shown(ix, 1), base58(&authority));
                effect.rows.push(Row::mono("nonce 账户", self.shown(ix, 0)));
                effect
            }
            _ => Effect::unknown("其他系统指令"),
        }
    }

    /// 转入租金,分配空间,初始化三步,地址和 authority 都必须是本钱包的.
    /// 这个标题是点按确认,所以交易里除了优先费不能有别的指令,否则转账会藏在标题后面.
    fn nonce_creation(&mut self, index: usize) -> Option<Effect> {
        let instructions = &self.message.instructions;
        let fund = instructions.get(index)?;
        let allocate = instructions.get(index + 1)?;
        let initialize = instructions.get(index + 2)?;
        let amount = fund.data.strip_prefix(&2u32.to_le_bytes()[..])?;
        let lamports = le_u64(amount).filter(|_| amount.len() == 8)?;
        let alone = instructions
            .iter()
            .enumerate()
            .all(|(i, ix)| (index..index + 3).contains(&i) || self.program(ix) == COMPUTE_BUDGET_PROGRAM);
        let own = alone
            && self.program(fund) == SYSTEM_PROGRAM
            && self.program(allocate) == SYSTEM_PROGRAM
            && self.program(initialize) == SYSTEM_PROGRAM
            && allocate.data == allocate_nonce_data(&self.payer)
            && initialize.data == initialize_nonce_data(&self.payer)
            && self.accounts_are(fund, &[self.payer, self.nonce])
            && self.accounts_are(allocate, &[self.nonce, self.payer])
            && self.accounts_are(initialize, &[self.nonce, RECENT_BLOCKHASHES_SYSVAR, RENT_SYSVAR]);
        if !own {
            return None;
        }
        if lamports as u128 > policy::fee_ceiling(self.network) {
            self.alerts.push("存入 nonce 账户的金额明显偏高".to_string());
        }
        let mut effect = Effect::new(Risk::Recognized, CREATE_NONCE_TITLE);
        effect.rows = vec![
            Row::mono("nonce 账户", base58(&self.nonce)),
            Row::text("存入金额", self.sol(lamports)),
        ];
        effect.amount = Some(self.sol_view(lamports));
        effect.counterparty = Some(("nonce 账户", base58(&self.nonce)));
        Some(effect)
    }

    fn own_advance(&self, index: usize, ix: &Instruction) -> bool {
        index == 0 && ix.data.len() == 4 && self.accounts_are(ix, &[self.nonce, RECENT_BLOCKHASHES_SYSVAR, self.payer])
    }

    fn recipient_owner(&self, program: &[u8; 32], mint: &[u8; 32], destination: &[u8; 32]) -> Option<[u8; 32]> {
        self.hint
            .filter(|owner| associated_token_address(owner, mint, program).as_ref() == Ok(destination))
    }

    /// 只有规范的形式才可能算普通转账.账户多余,数据有尾部字节或地址来自查找表时节点照样转账,
    /// 所以金额和目标都要显示,金额用原始整数.
    fn transfer_checked(&self, ix: &Instruction, program: &[u8; 32], rest: &[u8]) -> Option<Effect> {
        let amount = le_u64(rest)?;
        let decimals = *rest.get(8)?;
        let exact = rest.len() == 9 && ix.accounts.len() == 4 && !self.uses_lookup(ix);
        let (mint, destination) = (self.account(ix, 1), self.account(ix, 2));
        let view = self.token_view(mint.as_ref().filter(|_| exact), Some(decimals), amount);
        let listed = view.symbol.is_some();
        let mut effect = if listed {
            Effect::new(Risk::Transfer, Risk::Transfer.label())
        } else {
            Effect::unknown("未知代币的转账")
        };
        let owner = mint
            .zip(destination)
            .and_then(|(mint, destination)| self.recipient_owner(program, &mint, &destination));
        let shown = owner.map_or_else(|| self.shown(ix, 2), |owner| base58(&owner));
        let symbol = view.symbol.as_deref().map(|s| format!(" {s}")).unwrap_or_default();
        effect.rows = vec![
            Row::text("金额", format!("{}{symbol}", view.text)),
            Row::mono("代币", self.shown(ix, 1)),
            Row::text(
                "合约清单",
                if listed {
                    "在内置清单"
                } else {
                    "不在内置清单"
                },
            ),
            Row::mono("付款账户", self.shown(ix, 3)),
            Row::mono("来源代币账户", self.shown(ix, 0)),
            Row::mono("目标代币账户", self.shown(ix, 2)),
        ];
        match owner {
            Some(owner) => effect.rows.push(Row::mono("收款地址", base58(&owner))),
            None => {
                effect.rows.push(Row::text("收款地址", "无法确认所有者"));
                effect.unconfirmed_owner = true;
            }
        }
        effect.amount = Some(view);
        effect.counterparty = Some(("收款地址", shown));
        Some(effect)
    }

    fn approval(
        &self,
        ix: &Instruction,
        title: &str,
        mint: Option<[u8; 32]>,
        decimals: Option<u8>,
        amount: u64,
    ) -> Effect {
        // ApproveChecked 的账户表比 Approve 多一个 mint.
        let shift = usize::from(mint.is_some() || decimals.is_some());
        let delegate = self.shown(ix, 1 + shift);
        let mut view = self.token_view(mint.as_ref(), decimals, amount);
        view.unlimited = amount == u64::MAX;
        let symbol = view.symbol.as_deref().map(|s| format!(" {s}")).unwrap_or_default();
        let mut effect = Effect::new(Risk::Approval, title);
        effect.rows = vec![
            Row::mono("被授权方", delegate.clone()),
            Row::text("额度", format!("{}{symbol}", view.text)),
            Row::mono("代币账户", self.shown(ix, 0)),
            Row::mono("授权人", self.shown(ix, 2 + shift)),
        ];
        if shift == 1 {
            effect.rows.push(Row::mono("代币", self.shown(ix, 1)));
        }
        effect.amount = Some(view);
        effect.counterparty = Some(("被授权方", delegate));
        effect
    }

    fn set_authority(&self, ix: &Instruction, rest: &[u8]) -> Option<Effect> {
        let (&kind, rest) = rest.split_first()?;
        let new = match rest.split_first()? {
            (0, _) => "无,权限被永久移除".to_string(),
            (1, key) => base58(&key_at(key)?),
            _ => return None,
        };
        let kind = match kind {
            0 => "MintTokens".to_string(),
            1 => "FreezeAccount".to_string(),
            2 => "AccountOwner".to_string(),
            3 => "CloseAccount".to_string(),
            other => format!("类型 {other}"),
        };
        let mut effect = self.permission_change("SetAuthority", self.shown(ix, 1), new);
        effect.rows.push(Row::text("权限类型", kind));
        effect.rows.push(Row::mono("账户", self.shown(ix, 0)));
        Some(effect)
    }

    /// 代币程序解码时也忽略尾部字节,规则同 `system`.低风险的类别才要求长度完全一致.
    fn token(&self, ix: &Instruction, program: &[u8; 32]) -> Effect {
        let Some((&tag, rest)) = ix.data.split_first() else {
            return Effect::unknown("未知代币指令");
        };
        let approve_checked = || {
            let amount = le_u64(rest)?;
            Some(self.approval(ix, "ApproveChecked", self.account(ix, 1), Some(*rest.get(8)?), amount))
        };
        let effect = match tag {
            3 => le_u64(rest).map(|amount| {
                let mut effect = Effect::unknown("旧版代币转账");
                let view = self.token_view(None, None, amount);
                let destination = self.shown(ix, 1);
                effect.rows = vec![
                    Row::text("原始数额", view.text.clone()),
                    Row::mono("目标代币账户", destination.clone()),
                ];
                effect.amount = Some(view);
                effect.counterparty = Some(("目标代币账户", destination));
                effect
            }),
            4 => le_u64(rest).map(|amount| self.approval(ix, "Approve", None, None, amount)),
            5 => Some(Effect::unknown("Revoke")),
            6 => self.set_authority(ix, rest),
            9 => Some(Effect::unknown("CloseAccount")),
            12 => self.transfer_checked(ix, program, rest),
            13 => approve_checked(),
            _ => None,
        };
        effect.unwrap_or_else(|| Effect::unknown("其他代币指令"))
    }

    fn associated_token(&self, ix: &Instruction) -> Effect {
        let recognized = || {
            if !(ix.data.is_empty() || ix.data == [0] || ix.data == [1]) || ix.accounts.len() != 6 {
                return None;
            }
            let (account, owner, mint) = (self.account(ix, 1)?, self.account(ix, 2)?, self.account(ix, 3)?);
            let program = self.account(ix, 5)?;
            let valid = self.account(ix, 4)? == SYSTEM_PROGRAM
                && (program == TOKEN_PROGRAM || program == TOKEN_2022_PROGRAM)
                && associated_token_address(&owner, &mint, &program).as_ref() == Ok(&account);
            valid.then(|| {
                let mut effect = Effect::new(Risk::Recognized, CREATE_TOKEN_ACCOUNT_TITLE);
                effect.rows = vec![
                    Row::mono("所有者", base58(&owner)),
                    Row::mono("代币", base58(&mint)),
                    Row::mono("代币账户", base58(&account)),
                ];
                effect
            })
        };
        recognized().unwrap_or_else(|| Effect::unknown("其他关联代币账户指令"))
    }

    /// 节点解码时同样忽略尾部字节,所以手续费按前缀算.形式不规范的指令按无法识别展示.
    fn compute_budget(&mut self, ix: &Instruction) -> Option<Effect> {
        let plain = ix.accounts.is_empty();
        match ix.data.split_first() {
            Some((2, rest)) if rest.len() >= 4 => {
                let units = u32::from_le_bytes(*rest.first_chunk::<4>()?);
                let exact = plain && rest.len() == 4 && self.unit_limit.is_none();
                self.unit_limit = Some(self.unit_limit.unwrap_or(0).max(units));
                (!exact).then(|| Effect::unknown("SetComputeUnitLimit"))
            }
            Some((3, rest)) if rest.len() >= 8 => {
                let price = le_u64(rest)?;
                let exact = plain && rest.len() == 8 && self.unit_price.is_none();
                self.unit_price = Some(self.unit_price.unwrap_or(0).max(price));
                (!exact).then(|| Effect::unknown("SetComputeUnitPrice"))
            }
            _ => Some(Effect::unknown("其他 ComputeBudget 指令")),
        }
    }

    fn run(&mut self) -> Vec<Effect> {
        let message = self.message;
        let mut effects = Vec::new();
        let mut index = 0;
        while let Some(ix) = message.instructions.get(index) {
            let program = self.program(ix);
            let effect = if program == SYSTEM_PROGRAM {
                if ix.data.starts_with(&4u32.to_le_bytes()) {
                    if self.own_advance(index, ix) {
                        None
                    } else {
                        self.alerts
                            .push("AdvanceNonceAccount 不在第一条,或 nonce 账户不属于本钱包".to_string());
                        Some(Effect::unknown("AdvanceNonceAccount"))
                    }
                } else if let Some(effect) = self.nonce_creation(index) {
                    index += 2;
                    Some(effect)
                } else {
                    Some(self.system(ix))
                }
            } else if program == TOKEN_PROGRAM || program == TOKEN_2022_PROGRAM {
                Some(self.token(ix, &program))
            } else if program == ASSOCIATED_TOKEN_PROGRAM {
                Some(self.associated_token(ix))
            } else if program == COMPUTE_BUDGET_PROGRAM {
                self.compute_budget(ix)
            } else {
                Some(Effect::unknown(Risk::Unknown.label()))
            };
            if let Some(mut effect) = effect {
                if self.uses_lookup(ix) {
                    effect.risk = effect.risk.max(Risk::Unknown);
                }
                if effect.risk >= Risk::Unknown {
                    effect.rows.extend(self.details(ix));
                }
                effects.push(effect);
            }
            index += 1;
        }
        effects
    }

    fn base_fee(&self) -> u64 {
        LAMPORTS_PER_SIGNATURE * self.message.required_signatures as u64
    }

    /// 微 lamports 单价乘单元数,向上取整到 lamports.结果超过 64 位时取上限,节点也是饱和计算.
    fn priority_fee(&self) -> u64 {
        let default_units = self.message.instructions.len() as u64 * DEFAULT_UNITS_PER_INSTRUCTION;
        let units = self.unit_limit.map_or(default_units, u64::from).min(MAX_COMPUTE_UNITS);
        let total = (self.unit_price.unwrap_or(0) as u128 * units as u128).div_ceil(MICRO_LAMPORTS_PER_LAMPORT);
        u64::try_from(total).unwrap_or(u64::MAX)
    }
}

/// 消息不是 ABI 编码,方法选择器和疑似地址这些提示不适用.
fn plain_lines(bytes: &[u8]) -> Vec<RawLine> {
    let mut lines = raw_lines(bytes);
    lines.iter_mut().for_each(|line| line.note = None);
    lines
}

pub fn review(network: Network, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = signer_path(request, network)?;
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::MessageSigning);
    }
    let payer = ctx.keys.ed25519(&path)?;
    let message = Message::decode(&request.payload)?;
    if message.keys[0] != payer {
        return Err(Forbid::AddressMismatch);
    }

    let native = ctx.manifest.native(network.id()).ok_or(Forbid::UnknownNetwork)?;
    let mut scan = Scan {
        message: &message,
        network,
        manifest: ctx.manifest,
        native,
        payer,
        nonce: nonce_account_address(&payer).map_err(|_| Forbid::AddressMismatch)?,
        hint: request.hint.as_deref().and_then(|hint| parse_solana_address(hint).ok()),
        unit_limit: None,
        unit_price: None,
        alerts: Vec::new(),
    };
    let mut effects = scan.run();
    // 只推进 nonce:之前用这个 nonce 值签名的交易全部作废.这是 Solana 上取消交易的写法.
    let cancels = effects.is_empty()
        && message.instructions.first().is_some_and(|ix| {
            scan.program(ix) == SYSTEM_PROGRAM && ix.data.starts_with(&4u32.to_le_bytes()) && scan.own_advance(0, ix)
        });
    if cancels {
        let mut effect = Effect::new(Risk::Recognized, ADVANCE_NONCE_TITLE);
        effect.rows = vec![Row::mono("nonce 账户", base58(&scan.nonce))];
        effects.push(effect);
        // 这一笔会让之前签过的交易失效,不能点一下就过去.
        scan.alerts.push("用当前 nonce 值签过名的交易都会作废".to_string());
    }

    let mut builder = ReviewBuilder::default();
    let mut unconfirmed = false;
    let nonce_creation = effects.iter().any(|e| e.title == CREATE_NONCE_TITLE);
    let token_accounts = effects.iter().filter(|e| e.title == CREATE_TOKEN_ACCOUNT_TITLE).count();

    // 页面顶部只显示一个动作,用户看的就是它,所以它必须是唯一风险最高的动作.
    // 最高的有多个,或者无法识别的指令没有金额和对方时,顶部留空,不能让小额动作盖住真正的动作.
    // 全部动作都在 `actions` 里.
    let named: Vec<&Effect> = effects.iter().filter(|e| e.counterparty.is_some()).collect();
    let named_count = named.len();
    let top = named.iter().map(|e| e.risk).max();
    let tied = named.iter().filter(|e| Some(e.risk) == top).count();
    let unnamed = effects
        .iter()
        .filter(|e| e.counterparty.is_none() && e.risk >= Risk::Unknown)
        .map(|e| e.risk)
        .max();
    let headline = tied == 1 && unnamed.is_none_or(|risk| Some(risk) < top);

    for effect in effects {
        if headline && effect.counterparty.is_some() && Some(effect.risk) == top {
            builder.amount = effect.amount;
            builder.counterparty_label = effect.counterparty.as_ref().map(|(label, _)| label.to_string());
            builder.counterparty = effect.counterparty.map(|(_, value)| value);
        }
        unconfirmed |= effect.unconfirmed_owner;
        builder.actions.push(ActionView {
            risk: effect.risk,
            title: effect.title,
            rows: effect.rows,
        });
    }

    let lookup_row = (!message.lookups.is_empty())
        .then(|| Row::text("地址查找表", format!("{} 个地址无法离线解析", message.lookup_count())));
    if let Some(row) = &lookup_row {
        builder.actions.push(ActionView {
            risk: Risk::Unknown,
            title: "地址查找表".to_string(),
            rows: vec![row.clone()],
        });
    }

    if message.required_signatures > 1 {
        scan.alerts.push("这笔交易还需要其他账户签名".to_string());
    }
    if named_count > 1 {
        scan.alerts
            .push(format!("这笔交易包含 {named_count} 个转账,授权或权限变更,请逐个核对"));
    }
    if unconfirmed {
        scan.alerts.push("无法确认代币收款方的所有者".to_string());
    }
    if token_accounts > 1 {
        scan.alerts
            .push(format!("这笔交易创建 {token_accounts} 个代币账户,每个都要付租金"));
    }
    let (base_fee, priority_fee) = (scan.base_fee(), scan.priority_fee());
    if (base_fee as u128 + priority_fee as u128) > policy::fee_ceiling(network) {
        scan.alerts.push("手续费明显偏高".to_string());
    }

    let signers: Vec<String> = message.keys[..message.required_signatures as usize]
        .iter()
        .map(base58)
        .collect();
    let nonce_account = message
        .instructions
        .first()
        .filter(|ix| scan.program(ix) == SYSTEM_PROGRAM && ix.data.starts_with(&4u32.to_le_bytes()))
        .map_or_else(|| "无".to_string(), |ix| scan.shown(ix, 0));
    builder.rows.push(Row::text("网络", network.label()));
    builder.rows.push(Row::mono("手续费支付者", base58(&payer)));
    builder.rows.push(Row::mono("派生路径", path.to_string()));
    builder.rows.push(Row::mono("全部签名者", signers.join("\n")));
    builder.rows.push(Row::mono("nonce 账户", nonce_account));
    builder.rows.push(Row::text(
        "消息版本",
        match message.version {
            Version::Legacy => "legacy",
            Version::V0 => "v0",
        },
    ));
    builder
        .rows
        .push(Row::text("指令数", message.instructions.len().to_string()));
    builder.rows.push(Row::text("基础费", scan.sol(base_fee)));
    builder.rows.push(Row::text("优先费", scan.sol(priority_fee)));
    builder.rows.extend(lookup_row);
    if unconfirmed {
        builder.rows.push(Row::text("收款地址", "无法确认所有者"));
    }
    builder.alerts = std::mem::take(&mut scan.alerts);
    builder.raw = plain_lines(&request.payload);

    let mut review = builder.finish(network.id());
    if review.risk == Risk::Recognized && nonce_creation {
        review.title = CREATE_NONCE_TITLE.to_string();
    }
    if cancels {
        review.title = ADVANCE_NONCE_TITLE.to_string();
    }
    Ok(review)
}

/// 构建指令时用的账户.
struct Meta {
    key: [u8; 32],
    signer: bool,
    writable: bool,
}

fn signer(key: [u8; 32]) -> Meta {
    Meta {
        key,
        signer: true,
        writable: true,
    }
}

fn writable(key: [u8; 32]) -> Meta {
    Meta {
        key,
        signer: false,
        writable: true,
    }
}

fn readonly(key: [u8; 32]) -> Meta {
    Meta {
        key,
        signer: false,
        writable: false,
    }
}

struct Call {
    program: [u8; 32],
    accounts: Vec<Meta>,
    data: Vec<u8>,
}

/// 账户表依次是可写签名者,只读签名者,可写非签名者,只读非签名者,同组内保持首次出现的顺序.
fn compile(payer: [u8; 32], blockhash: [u8; 32], calls: &[Call]) -> Message {
    fn merge(metas: &mut Vec<Meta>, meta: &Meta) {
        match metas.iter_mut().find(|m| m.key == meta.key) {
            Some(known) => {
                known.signer |= meta.signer;
                known.writable |= meta.writable;
            }
            None => metas.push(Meta {
                key: meta.key,
                signer: meta.signer,
                writable: meta.writable,
            }),
        }
    }
    let mut metas = vec![signer(payer)];
    for call in calls {
        merge(&mut metas, &readonly(call.program));
        call.accounts.iter().for_each(|meta| merge(&mut metas, meta));
    }
    metas.sort_by_key(|m| (!m.signer, !m.writable));
    let index = |key: &[u8; 32]| metas.iter().position(|m| m.key == *key).expect("账户已写入账户表") as u8;
    Message {
        version: Version::Legacy,
        required_signatures: metas.iter().filter(|m| m.signer).count() as u8,
        readonly_signed: metas.iter().filter(|m| m.signer && !m.writable).count() as u8,
        readonly_unsigned: metas.iter().filter(|m| !m.signer && !m.writable).count() as u8,
        keys: metas.iter().map(|m| m.key).collect(),
        blockhash,
        instructions: calls
            .iter()
            .map(|call| Instruction {
                program: index(&call.program),
                accounts: call.accounts.iter().map(|m| index(&m.key)).collect(),
                data: call.data.clone(),
            })
            .collect(),
        lookups: Vec::new(),
    }
}

fn advance_nonce(nonce: [u8; 32], authority: [u8; 32]) -> Call {
    Call {
        program: SYSTEM_PROGRAM,
        accounts: vec![writable(nonce), readonly(RECENT_BLOCKHASHES_SYSVAR), signer(authority)],
        data: 4u32.to_le_bytes().to_vec(),
    }
}

fn compute_unit_limit(units: u32) -> Call {
    let mut data = vec![2];
    data.extend_from_slice(&units.to_le_bytes());
    Call {
        program: COMPUTE_BUDGET_PROGRAM,
        accounts: Vec::new(),
        data,
    }
}

fn compute_unit_price(micro_lamports: u64) -> Call {
    let mut data = vec![3];
    data.extend_from_slice(&micro_lamports.to_le_bytes());
    Call {
        program: COMPUTE_BUDGET_PROGRAM,
        accounts: Vec::new(),
        data,
    }
}

fn system_transfer(from: [u8; 32], to: [u8; 32], lamports: u64) -> Call {
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    Call {
        program: SYSTEM_PROGRAM,
        accounts: vec![signer(from), writable(to)],
        data,
    }
}

fn create_associated_token_account(payer: [u8; 32], account: [u8; 32], owner: [u8; 32], mint: [u8; 32]) -> Call {
    Call {
        program: ASSOCIATED_TOKEN_PROGRAM,
        accounts: vec![
            signer(payer),
            writable(account),
            readonly(owner),
            readonly(mint),
            readonly(SYSTEM_PROGRAM),
            readonly(TOKEN_PROGRAM),
        ],
        // CreateIdempotent:签名等待期间账户被别人建好时,这笔交易不会失败.
        data: vec![1],
    }
}

fn transfer_checked(
    source: [u8; 32],
    mint: [u8; 32],
    destination: [u8; 32],
    owner: [u8; 32],
    amount: u64,
    decimals: u8,
) -> Call {
    let mut data = vec![12];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Call {
        program: TOKEN_PROGRAM,
        accounts: vec![writable(source), readonly(mint), writable(destination), signer(owner)],
        data,
    }
}

pub struct TransferParams<'a> {
    pub network: Network,
    pub from: &'a str,
    /// 收款方的所有者地址,代币转账时用它算出代币账户.
    pub to: &'a str,
    /// 最小单位.
    pub amount: u64,
    /// 代币的 mint.留空是 SOL 转账.
    pub mint: Option<&'a str>,
    pub decimals: u8,
    /// nonce 账户里当前的值,放在 blockhash 的位置.
    pub nonce_value: [u8; 32],
    pub priority_micro_lamports: u64,
    pub compute_units: u32,
    pub create_recipient_ata: bool,
}

/// 构建转账,返回 legacy 消息字节.第一条指令是 AdvanceNonceAccount,代币只用 SPL Token 程序.
pub fn build_transfer(params: &TransferParams) -> Result<Vec<u8>, Error> {
    if params.network.chain() != Chain::Solana {
        return Err(Error::InvalidParams);
    }
    let payer = parse_solana_address(params.from)?;
    let to = parse_solana_address(params.to)?;
    let mut calls = vec![advance_nonce(nonce_account_address(&payer)?, payer)];
    if params.compute_units > 0 {
        calls.push(compute_unit_limit(params.compute_units));
    }
    if params.priority_micro_lamports > 0 {
        calls.push(compute_unit_price(params.priority_micro_lamports));
    }
    match params.mint {
        None => calls.push(system_transfer(payer, to, params.amount)),
        Some(mint) => {
            let mint = parse_solana_address(mint)?;
            let source = associated_token_address(&payer, &mint, &TOKEN_PROGRAM)?;
            let destination = associated_token_address(&to, &mint, &TOKEN_PROGRAM)?;
            if params.create_recipient_ata {
                calls.push(create_associated_token_account(payer, destination, to, mint));
            }
            calls.push(transfer_checked(
                source,
                mint,
                destination,
                payer,
                params.amount,
                params.decimals,
            ));
        }
    }
    Ok(compile(payer, params.nonce_value, &calls).encode())
}

/// 只推进 nonce,用同一个 nonce 值签过名的交易随之作废.
pub fn build_advance_nonce(payer: &str, nonce_value: [u8; 32]) -> Result<Vec<u8>, Error> {
    let payer = parse_solana_address(payer)?;
    let nonce = nonce_account_address(&payer)?;
    Ok(compile(payer, nonce_value, &[advance_nonce(nonce, payer)]).encode())
}

/// 创建本钱包的 nonce 账户.这一笔只能用 blockhash,有效期约 1 分钟.
pub fn build_create_nonce(payer: &str, recent_blockhash: [u8; 32], lamports: u64) -> Result<Vec<u8>, Error> {
    let payer = parse_solana_address(payer)?;
    let nonce = nonce_account_address(&payer)?;
    let calls = [
        system_transfer(payer, nonce, lamports),
        Call {
            program: SYSTEM_PROGRAM,
            accounts: vec![writable(nonce), signer(payer)],
            data: allocate_nonce_data(&payer),
        },
        Call {
            program: SYSTEM_PROGRAM,
            accounts: vec![
                writable(nonce),
                readonly(RECENT_BLOCKHASHES_SYSVAR),
                readonly(RENT_SYSVAR),
            ],
            data: initialize_nonce_data(&payer),
        },
    ];
    Ok(compile(payer, recent_blockhash, &calls).encode())
}

/// `signer` 是手续费支付者的公钥,必须和消息里的账户 0 一致.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 64] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    let message = Message::decode(&request.payload).map_err(|_| Error::InvalidParams)?;
    // 还缺其他人的签名时,这笔交易不能广播.
    if message.required_signatures != 1 {
        return Err(Error::InvalidParams);
    }
    let signer: &[u8; 32] = signer.try_into().map_err(|_| Error::InvalidKey)?;
    if message.keys[0] != *signer {
        return Err(Error::BadSignature);
    }
    let key = VerifyingKey::from_bytes(signer).map_err(|_| Error::InvalidKey)?;
    key.verify_strict(&request.payload, &Signature::from_bytes(signature))
        .map_err(|_| Error::BadSignature)?;
    let mut raw = Vec::with_capacity(1 + 64 + request.payload.len());
    put_len(&mut raw, 1);
    raw.extend_from_slice(signature);
    raw.extend_from_slice(&request.payload);
    Ok(Verified {
        raw,
        hash: bitcoin::base58::encode(signature),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYER: [u8; 32] = [0x11; 32];
    const OTHER: [u8; 32] = [0x22; 32];
    const PROGRAM: [u8; 32] = [0x33; 32];

    fn message() -> Message {
        Message {
            version: Version::Legacy,
            required_signatures: 1,
            readonly_signed: 0,
            readonly_unsigned: 1,
            keys: vec![PAYER, OTHER, PROGRAM],
            blockhash: [7; 32],
            instructions: vec![Instruction {
                program: 2,
                accounts: vec![0, 1],
                data: vec![1, 2, 3],
            }],
            lookups: Vec::new(),
        }
    }

    fn v0() -> Message {
        let mut message = message();
        message.version = Version::V0;
        message.lookups = vec![Lookup {
            table: [9; 32],
            writable: vec![1, 2],
            readonly: vec![3],
        }];
        message.instructions[0].accounts = vec![0, 3, 4, 5];
        message
    }

    #[test]
    fn program_ids_match_their_base58_text() {
        for (text, bytes) in [
            ("11111111111111111111111111111111", SYSTEM_PROGRAM),
            ("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", TOKEN_PROGRAM),
            ("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb", TOKEN_2022_PROGRAM),
            ("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", ASSOCIATED_TOKEN_PROGRAM),
            ("ComputeBudget111111111111111111111111111111", COMPUTE_BUDGET_PROGRAM),
            ("SysvarRecentB1ockHashes11111111111111111111", RECENT_BLOCKHASHES_SYSVAR),
            ("SysvarRent111111111111111111111111111111111", RENT_SYSVAR),
        ] {
            assert_eq!(parse_solana_address(text).unwrap(), bytes, "{text}");
        }
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
            assert_eq!(read_len(&mut &*out), Ok(value));
        }
        // 超过 16 位和 4 字节编码都不合法.
        let too_big: &[u8] = &[0xff, 0xff, 0x04];
        assert_eq!(read_len(&mut &*too_big), Err(Forbid::ParseFailed));
        let too_long: &[u8] = &[0x80, 0x80, 0x80, 0x01];
        assert_eq!(read_len(&mut &*too_long), Err(Forbid::ParseFailed));
    }

    #[test]
    fn round_trip_legacy_and_v0() {
        for message in [message(), v0()] {
            assert_eq!(Message::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn non_canonical_lengths_are_refused() {
        // 账户数 3 写成两字节的 0x83 0x00.
        let mut bytes = message().encode();
        bytes.splice(3..4, [0x83, 0x00]);
        assert_eq!(Message::decode(&bytes), Err(Forbid::ParseFailed));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        for message in [message(), v0()] {
            let mut bytes = message.encode();
            bytes.push(0);
            assert_eq!(Message::decode(&bytes), Err(Forbid::ParseFailed));
        }
    }

    #[test]
    fn truncation_is_refused_at_every_length() {
        for message in [message(), v0()] {
            let bytes = message.encode();
            for len in 0..bytes.len() {
                assert_eq!(Message::decode(&bytes[..len]), Err(Forbid::ParseFailed), "{len}");
            }
        }
    }

    #[test]
    fn unsupported_versions_are_refused() {
        let mut bytes = v0().encode();
        bytes[0] = 0x81;
        assert_eq!(Message::decode(&bytes), Err(Forbid::Unsupported));
        bytes[0] = 0xff;
        assert_eq!(Message::decode(&bytes), Err(Forbid::Unsupported));
    }

    #[test]
    fn header_must_fit_the_account_table() {
        let bad = |change: fn(&mut Message)| {
            let mut m = message();
            change(&mut m);
            assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
        };
        bad(|m| m.required_signatures = 0);
        bad(|m| m.readonly_signed = 1);
        bad(|m| m.required_signatures = 3);
        bad(|m| m.readonly_unsigned = 3);
    }

    #[test]
    fn indexes_must_be_in_range() {
        let bad = |change: fn(&mut Message)| {
            let mut m = message();
            change(&mut m);
            assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
        };
        bad(|m| m.instructions[0].program = 3);
        bad(|m| m.instructions[0].program = 0);
        bad(|m| m.instructions[0].accounts = vec![3]);
        bad(|m| m.instructions[0].accounts = vec![0, 255]);
    }

    #[test]
    fn indexes_can_reach_the_lookups_but_not_past_them() {
        let mut m = v0();
        m.instructions[0].accounts = vec![6];
        assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
        // 程序不能来自查找表.
        let mut m = v0();
        m.instructions[0].program = 3;
        assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
    }

    #[test]
    fn empty_lookup_is_refused() {
        let mut m = v0();
        m.lookups[0].writable.clear();
        m.lookups[0].readonly.clear();
        m.instructions[0].accounts = vec![0];
        assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
    }

    #[test]
    fn duplicate_accounts_are_refused() {
        let mut m = message();
        m.keys[1] = PAYER;
        assert_eq!(Message::decode(&m.encode()), Err(Forbid::ParseFailed));
    }

    #[test]
    fn empty_payload_is_refused() {
        assert_eq!(Message::decode(&[]), Err(Forbid::ParseFailed));
    }

    #[test]
    fn signer_and_writable_marks() {
        let m = Message {
            required_signatures: 3,
            readonly_signed: 1,
            readonly_unsigned: 1,
            keys: vec![[1; 32], [2; 32], [3; 32], [4; 32], [5; 32]],
            ..v0()
        };
        let signer: Vec<bool> = (0..8).map(|i| m.is_signer(i)).collect();
        assert_eq!(signer, [true, true, true, false, false, false, false, false]);
        let writable: Vec<bool> = (0..8).map(|i| m.is_writable(i)).collect();
        assert_eq!(writable, [true, true, false, true, false, true, true, false]);
    }

    /// 期望值由 @solana/web3.js 的 `PublicKey.createWithSeed` 算出.
    #[test]
    fn nonce_address_matches_web3js() {
        let payer = parse_solana_address("HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk").unwrap();
        assert_eq!(
            base58(&nonce_account_address(&payer).unwrap()),
            "DR8vVnEzE8JD2JXdqmNXZxo3WneEEdxdxoAEfycoq7XT"
        );
    }

    #[test]
    fn wrong_network_is_refused() {
        let payer = base58(&PAYER);
        let params = TransferParams {
            network: Network::Tron,
            from: &payer,
            to: &payer,
            amount: 1,
            mint: None,
            decimals: 0,
            nonce_value: [0; 32],
            priority_micro_lamports: 0,
            compute_units: 0,
            create_recipient_ata: false,
        };
        assert_eq!(build_transfer(&params), Err(Error::InvalidParams));
    }
}

//! Solana 交易:核对,构建和核验.解码在 `decode::solana`,这里从解码树算出策略要的事实和页面顶部.
//! 载荷的 ed25519 签名原文就是载荷本身.

use crate::address::parse_solana_address;
use crate::decode::solana::{
    self as format, SolanaPayload, SolanaWalker, is_batch, is_token_program, parse_solana, put_len,
};
use crate::decode::{self, Coin, Decoded, Node, NodeId, Value, display};
use crate::manifest::{Asset, Manifest};
use crate::path::{DerivationPath, check_solana_path, is_usual_solana_path, solana_path_mode};
use crate::policy::{self, Finding, SolanaFacts, SolanaFinding};
use crate::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use crate::review::{Forbid, Review, Risk};
use crate::tx::evm::MAX_PATH_DEPTH;
use crate::tx::summary::{self, Effect, number_of};
use crate::tx::{ReviewContext, Verified};
use crate::units::format_units;
use crate::{Chain, Error, Mode, Network};
use bitcoin::hex::DisplayHex;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

pub use crate::decode::solana::{Instruction, Lookup, Message, V1Config, Version};

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
const NONCE_ACCOUNT_LABEL: &str = "nonce 账户";
/// nonce 账户的数据长度.
const NONCE_SPACE: u64 = 80;
/// 规格化的 `transferChecked` 的账户数.
const PLAIN_TOKEN_ACCOUNTS: usize = 4;
const PROGRAM_ROW: &str = "program_id";
const LOOKUP_TABLE_ROW: &str = "lookup_table";
const BASE_FEE_ROW: &str = "base_fee";
const PRIORITY_FEE_ROW: &str = "priority_fee";
const INSTRUCTION_ROW: &str = "instruction";
const ACCOUNTS_ROW: &str = "accounts";
const DATA_ROW: &str = "data";
const AMOUNT_ROW: &str = "amount";
const DECIMALS_ROW: &str = "decimals";
const MINT_ROW: &str = "mint";
const TOKEN_ACTIONS: &str = "token.";
const TOKEN_2022_ACTIONS: &str = "token2022.";
const ADVANCE_ACTION: &str = "system.advanceNonceAccount";
const TRANSFER_SOL_ACTION: &str = "system.transferSol";
const TRANSFER: &str = "transfer";
const CHECKED_TRANSFERS: [&str; 2] = ["transferChecked", "transferCheckedWithFee"];
const ASSOCIATED_ACTIONS: [&str; 3] = [
    "associatedTokenAccount.create",
    "associatedTokenAccount.createIdempotent",
    "associatedTokenAccount.createWithArgs",
];
/// 空数据和判别值 0 都是 `create`.`createWithArgs` 不算规范形式.
const PLAIN_ASSOCIATED_ACTIONS: [&str; 2] = [
    "associatedTokenAccount.create",
    "associatedTokenAccount.createIdempotent",
];
const ASSOCIATED_ACCOUNT_ROW: &str = "associatedTokenAccount";
const WALLET_ROW: &str = "wallet";
const SYSTEM_PROGRAM_ROW: &str = "systemProgram";
const TOKEN_PROGRAM_ROW: &str = "tokenProgram";
const ALLOCATE_ACTION: &str = "system.allocateWithSeed";
const ALLOCATED_ROW: &str = "newAccount";
const INITIALIZE_ACTION: &str = "system.initializeNonceAccount";
const COMPUTE_BUDGET_ACTIONS: &str = "computeBudget.";

fn base58(key: &[u8; 32]) -> String {
    bitcoin::base58::encode(key)
}

impl Message {
    /// 只认交易消息,载荷的其他格式走 `decode`.解析器保证重新编码回完全相同的字节.
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        match parse_solana(payload).map_err(|_| Forbid::ParseFailed)? {
            SolanaPayload::Message(message) => Ok(*message),
            SolanaPayload::Partial { .. } | SolanaPayload::Offchain { .. } | SolanaPayload::Unknown => {
                Err(Forbid::Unsupported)
            }
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

pub fn associated_token_address(owner: &[u8; 32], mint: &[u8; 32], program: &[u8; 32]) -> Result<[u8; 32], Error> {
    format::associated_token_address(owner, mint, program).ok_or(Error::InvalidParams)
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

/// 消息里的一条指令.`batch` 的内部指令和消息里的指令一样算,它们的 `id` 和规则都是自己的.
#[derive(Clone, Copy)]
struct Step<'a> {
    id: NodeId,
    node: &'a Node,
    kind: &'a str,
}

fn steps(decoded: &Decoded) -> Vec<Step<'_>> {
    decoded
        .visits()
        .into_iter()
        .filter(|visit| visit.node.name == INSTRUCTION_ROW)
        .map(|visit| Step {
            id: visit.id,
            node: visit.node,
            kind: visit.node.action.as_deref().unwrap_or_default(),
        })
        .collect()
}

fn argument_text(node: &Node, name: &str) -> Option<String> {
    let found = section(node, DATA_ROW).iter().find(|child| child.name == name)?;
    Some(display(&found.value))
}

/// 没有多余账户和多余字节,数据都读懂了.
fn is_exact(node: &Node) -> bool {
    let rows = section(node, DATA_ROW).iter().chain(section(node, ACCOUNTS_ROW));
    rows.into_iter().all(|row| !matches!(row.value, Value::Opaque(_)))
}

fn uses_lookup(node: &Node) -> bool {
    section(node, ACCOUNTS_ROW)
        .iter()
        .any(|account| account.children.iter().any(|child| child.name == LOOKUP_TABLE_ROW))
}

fn accounts_are(node: &Node, expected: &[&str]) -> bool {
    let accounts = section(node, ACCOUNTS_ROW);
    accounts.len() == expected.len()
        && accounts
            .iter()
            .zip(expected)
            .all(|(account, key)| matches!(&account.value, Value::Address(text) if text == key))
}

fn program_of(node: &Node) -> Option<&str> {
    let found = node.children.iter().find(|child| child.name == PROGRAM_ROW)?;
    if let Value::Address(text) = &found.value {
        Some(text)
    } else {
        None
    }
}

/// 从解码树读出页面顶部和 nonce 的事实.不算动作的指令和本钱包的 AdvanceNonceAccount 没有 `Effect`,其余每条都有一个.
struct Reader<'a> {
    /// 请求里的网络,本机不一定认识.
    network: &'a str,
    known: Option<Network>,
    manifest: &'a Manifest,
    native: Option<&'a Asset>,
    /// 本账户和它的 nonce 账户.路径没有导出时公钥算不出来,要解锁后才能核对.
    own: Option<(String, String)>,
    /// 本钱包的 nonce 创建占用的指令.
    creation: Option<Range<usize>>,
    /// 不在第一条,或者不属于本钱包的 AdvanceNonceAccount.
    foreign_advances: Vec<usize>,
    deposit_high: bool,
}

impl<'a> Reader<'a> {
    /// 只有规范形式才显示清单里的符号.Token-2022 的扩展能改变实际效果,清单声明过才算,
    /// 账户多余,数据有尾部字节或地址来自查找表时节点照样执行,金额用原始整数.
    fn listed(&self, step: Step<'_>, mint: &str, decimals: Option<u64>) -> Option<&'a Asset> {
        let asset = self.manifest.lookup(self.network, mint)?;
        let declared = step.kind.starts_with(TOKEN_ACTIONS) || asset.extensions.is_some();
        let plain = declared && is_exact(step.node) && !uses_lookup(step.node);
        (plain && decimals == Some(u64::from(asset.decimals))).then_some(asset)
    }

    fn read(&self, step: Step<'_>) -> Effect {
        summary::effect(
            Chain::Solana,
            step.kind,
            step.id,
            step.node,
            self.native,
            |mint, decimals| self.listed(step, mint, decimals),
        )
    }

    fn own_advance(&self, index: usize, step: Step<'_>) -> bool {
        self.own.as_ref().is_some_and(|(payer, nonce)| {
            let sysvar = base58(&RECENT_BLOCKHASHES_SYSVAR);
            index == 0
                && step.kind == ADVANCE_ACTION
                && is_exact(step.node)
                && accounts_are(step.node, &[nonce, &sysvar, payer])
        })
    }

    /// 转入租金,分配空间,初始化三步,地址和 authority 都必须是本钱包的.
    /// 这个标题是点按确认,所以交易里除了优先费不能有别的指令,否则转账会藏在标题后面.
    /// `batch` 本身没有效果,它的内部指令都是单独的一条.
    fn nonce_creation(&mut self, steps: &[Step<'_>], index: usize) -> Option<Effect> {
        let (payer, nonce) = self.own.clone()?;
        let [fund, allocate, initialize] = *steps.get(index..index + 3)? else {
            return None;
        };
        let lamports = argument_of(fund.node, AMOUNT_ROW)?;
        let budget = base58(&COMPUTE_BUDGET_PROGRAM);
        let system = base58(&SYSTEM_PROGRAM);
        let alone = steps.iter().enumerate().all(|(i, step)| {
            (index..index + 3).contains(&i) || program_of(step.node) == Some(budget.as_str()) || is_batch(step.kind)
        });
        let blockhashes = base58(&RECENT_BLOCKHASHES_SYSVAR);
        let rent = base58(&RENT_SYSVAR);
        let argument =
            |step: Step<'_>, name: &str, wanted: &str| argument_text(step.node, name).as_deref() == Some(wanted);
        let own = alone
            && fund.kind == TRANSFER_SOL_ACTION
            && allocate.kind == ALLOCATE_ACTION
            && initialize.kind == INITIALIZE_ACTION
            && [fund, allocate, initialize].iter().all(|step| is_exact(step.node))
            && argument(allocate, "base", &payer)
            && argument(allocate, "seed", NONCE_SEED)
            && argument(allocate, "programAddress", &system)
            && argument_of(allocate.node, "space") == Some(NONCE_SPACE)
            && argument(initialize, "nonceAuthority", &payer)
            && accounts_are(fund.node, &[&payer, &nonce])
            && accounts_are(allocate.node, &[&nonce, &payer])
            && accounts_are(initialize.node, &[&nonce, &blockhashes, &rent]);
        if !own {
            return None;
        }
        self.creation = Some(index..index + 3);
        self.deposit_high = self
            .known
            .is_some_and(|network| lamports as u128 > policy::fee_ceiling(network));
        Some(Effect {
            counterparty: Some((NONCE_ACCOUNT_LABEL.to_string(), nonce)),
            parties: Vec::new(),
            ..self.read(fund)
        })
    }

    fn effect(&mut self, steps: &[Step<'_>], index: usize) -> (Option<Effect>, usize) {
        let step = steps[index];
        if step.kind == ADVANCE_ACTION {
            if self.own_advance(index, step) {
                return (None, 1);
            }
            self.foreign_advances.push(index);
            return (Some(self.read(step)), 1);
        }
        if let Some(effect) = self.nonce_creation(steps, index) {
            return (Some(effect), 3);
        }
        // 形式不规范的指令按无法识别展示,手续费由解码树算出.
        let passive = policy::is_passive(Chain::Solana, step.kind) && is_exact(step.node);
        ((!passive).then(|| self.read(step)), 1)
    }

    fn run(&mut self, steps: &[Step<'_>]) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut index = 0;
        while index < steps.len() {
            let (effect, used) = self.effect(steps, index);
            effects.extend(effect);
            index += used;
        }
        effects
    }
}

/// 解码之后算好的内容.类别和告警都来自策略.
#[derive(Default)]
struct Prepared {
    effects: Vec<Effect>,
    title: Option<&'static str>,
    facts: SolanaFacts,
}

/// 按名字取指令节点的子节点.名字来自 IDL.
fn section<'a>(node: &'a Node, name: &str) -> &'a [Node] {
    node.children
        .iter()
        .find(|child| child.name == name)
        .map_or(&[], |child| child.children.as_slice())
}

fn account_of<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    let account = section(node, ACCOUNTS_ROW).iter().find(|child| child.name == name)?;
    if let Value::Address(text) = &account.value {
        Some(text)
    } else {
        None
    }
}

fn account_key(node: &Node, name: &str) -> Option<[u8; 32]> {
    parse_solana_address(account_of(node, name)?).ok()
}

fn argument_of(node: &Node, name: &str) -> Option<u64> {
    let found = section(node, DATA_ROW).iter().find(|child| child.name == name)?;
    number_of(&found.value)
}

fn row_id(decoded: &Decoded, name: &str) -> NodeId {
    let visits = decoded.visits();
    visits
        .iter()
        .find(|visit| visit.node.name == name)
        .map_or(0, |visit| visit.id)
}

/// 这棵子树里第一个叫这个名字的节点.
fn descendant(decoded: &Decoded, root: NodeId, name: &str) -> Option<NodeId> {
    let visits = decoded.visits();
    let start = visits.get(root as usize)?;
    visits
        .iter()
        .skip(root as usize + 1)
        .take_while(|visit| visit.depth > start.depth)
        .find(|visit| visit.node.name == name)
        .map(|visit| visit.id)
}

fn lamports_row(decoded: &Decoded, name: &str) -> u64 {
    let visits = decoded.visits();
    visits
        .iter()
        .find(|visit| visit.node.name == name)
        .and_then(|visit| number_of(&visit.node.value))
        .unwrap_or(0)
}

fn finding(node: NodeId, kind: &'static str) -> SolanaFinding {
    SolanaFinding {
        node,
        kind,
        args: Vec::new(),
    }
}

/// 代币转账的类别要看清单:mint 在清单内,小数位一致,账户恰好 4 个才是普通转账.
/// Token-2022 的扩展是链上状态,只有清单声明过扩展的 mint 才可能是普通转账.
fn token_facts(decoded: &Decoded, manifest: &Manifest, network: &str, facts: &mut SolanaFacts) {
    for (id, node, kind) in decoded.actions() {
        let (classic, name) = match (kind.strip_prefix(TOKEN_ACTIONS), kind.strip_prefix(TOKEN_2022_ACTIONS)) {
            (Some(name), _) => (true, name),
            (None, Some(name)) => (false, name),
            (None, None) => continue,
        };
        let checked = CHECKED_TRANSFERS.contains(&name);
        if !checked && name != TRANSFER {
            continue;
        }
        let asset = account_of(node, MINT_ROW).and_then(|mint| manifest.lookup(network, mint));
        if !classic && asset.is_none_or(|asset| asset.extensions.is_none()) {
            facts.findings.push(finding(id, "token-2022-undeclared"));
        }
        if !checked {
            continue;
        }
        let decimals = argument_of(node, DECIMALS_ROW);
        match asset {
            Some(asset) if decimals != Some(u64::from(asset.decimals)) => {
                facts.findings.push(finding(id, "token-decimals-mismatch"));
            }
            Some(asset) if section(node, ACCOUNTS_ROW).len() == PLAIN_TOKEN_ACCOUNTS => {
                let shown = argument_of(node, AMOUNT_ROW)
                    .map(|amount| format!("{} {}", format_units(&amount.to_string(), asset.decimals), asset.symbol));
                let row = descendant(decoded, id, AMOUNT_ROW);
                facts.displays.extend(row.zip(shown));
            }
            Some(_) | None => facts.findings.push(finding(id, "token-unlisted")),
        }
    }
}

/// 地址由所有者,mint,程序推导得出.
fn ata_is_derived(node: &Node) -> bool {
    let derived = || {
        let program = account_key(node, TOKEN_PROGRAM_ROW)?;
        let owner = account_key(node, WALLET_ROW)?;
        let mint = account_key(node, MINT_ROW)?;
        let derived = format::associated_token_address(&owner, &mint, &program)?;
        let system = account_key(node, SYSTEM_PROGRAM_ROW)? == SYSTEM_PROGRAM;
        Some(system && is_token_program(&program) && account_key(node, ASSOCIATED_ACCOUNT_ROW)? == derived)
    };
    derived() == Some(true)
}

/// 规范的 `create` 和 `createIdempotent`,地址由所有者,mint,程序推导得出.
fn creates_token_account(step: Step<'_>) -> bool {
    PLAIN_ASSOCIATED_ACTIONS.contains(&step.kind) && is_exact(step.node) && ata_is_derived(step.node)
}

/// `create` 系列的地址必须由所有者,mint,程序推导得出.
fn associated_facts(decoded: &Decoded, facts: &mut SolanaFacts) {
    for (id, node, kind) in decoded.actions() {
        if ASSOCIATED_ACTIONS.contains(&kind) && !ata_is_derived(node) {
            facts.findings.push(finding(id, "ata-underived"));
        }
    }
}

/// 相同的 ComputeBudget 指令节点只接受一条.
fn budget_facts(decoded: &Decoded, facts: &mut SolanaFacts) {
    let budgets: Vec<(NodeId, &str)> = decoded
        .actions()
        .into_iter()
        .filter(|(_, _, kind)| kind.starts_with(COMPUTE_BUDGET_ACTIONS))
        .map(|(id, _, kind)| (id, kind))
        .collect();
    for (position, (id, kind)) in budgets.iter().enumerate() {
        if budgets[..position].iter().any(|(_, earlier)| earlier == kind) {
            facts.findings.push(finding(*id, "compute-budget-repeated"));
        }
    }
}

/// 本钱包的 nonce 创建之外,初始化 nonce 账户或者给本钱包的 nonce 地址分配空间都不算已识别.
fn nonce_facts(decoded: &Decoded, reader: &Reader, facts: &mut SolanaFacts) {
    let instructions = steps(decoded);
    facts.findings.extend(
        reader
            .foreign_advances
            .iter()
            .filter_map(|index| instructions.get(*index))
            .map(|step| finding(step.id, "nonce-advance-foreign")),
    );
    let own_nonce = reader.own.as_ref().map(|(_, nonce)| nonce.as_str());
    let in_creation = |id: NodeId| {
        let position = instructions.iter().position(|step| step.id == id);
        position.is_some_and(|position| reader.creation.as_ref().is_some_and(|range| range.contains(&position)))
    };
    for (id, node, kind) in decoded.actions() {
        let step =
            kind == INITIALIZE_ACTION || (kind == ALLOCATE_ACTION && account_of(node, ALLOCATED_ROW) == own_nonce);
        if step && !in_creation(id) {
            facts.findings.push(finding(id, "nonce-step-foreign"));
        }
    }
}

fn signer_facts(payload: &SolanaPayload, signer: Option<&[u8; 32]>, decoded: &Decoded, facts: &mut SolanaFacts) {
    let Some(required) = payload.signers() else {
        return;
    };
    let keys = row_id(decoded, "account_keys");
    if required.len() > 1 {
        facts.findings.push(finding(keys, "other-signers"));
    }
    match (signer, payload) {
        (None, _) => facts.findings.push(finding(keys, "signer-unverified")),
        (Some(key), SolanaPayload::Message(_) | SolanaPayload::Partial { .. }) if required.first() != Some(key) => {
            facts.findings.push(finding(keys, "fee-payer-foreign"));
        }
        (Some(_), _) => {}
    }
}

/// 每条指令的类别,页面顶部按它选动作.指令的字段上的发现都算,比如查找表里的账户和多余的字节,
/// `batch` 的内部指令是单独的一条.
fn ranks(decoded: &Decoded, findings: &BTreeMap<NodeId, Finding>) -> BTreeMap<NodeId, u8> {
    let visits = decoded.visits();
    let mut ranks = BTreeMap::new();
    for (id, finding) in findings {
        let mut at = Some(*id);
        while let Some(visit) = at.and_then(|id| visits.get(id as usize)) {
            if visit.node.name == INSTRUCTION_ROW {
                let rank = finding.risk.map_or(0, |risk| risk.headline_rank());
                ranks
                    .entry(visit.id)
                    .and_modify(|top: &mut u8| *top = rank.max(*top))
                    .or_insert(rank);
                break;
            }
            at = visit.parent;
        }
    }
    ranks
}

fn prepared(reader: &mut Reader, decoded: &Decoded) -> Prepared {
    let instructions = steps(decoded);
    let effects = reader.run(&instructions);
    // 只推进 nonce:之前用这个 nonce 值签名的交易全部作废.这是 Solana 上取消交易的写法.
    let cancels = effects.is_empty() && instructions.first().is_some_and(|step| reader.own_advance(0, *step));

    let mut facts = SolanaFacts::default();
    let root = 0;
    if cancels {
        // 这一笔会让之前签过的交易失效,不能点一下就过去.
        facts
            .findings
            .push(finding(row_id(decoded, INSTRUCTION_ROW), "nonce-cancel"));
    }
    if reader.deposit_high {
        facts.findings.push(finding(root, "nonce-deposit-high"));
    }
    let named = effects.iter().filter(|effect| effect.is_named()).count();
    if named > 1 {
        facts.findings.push(SolanaFinding {
            node: root,
            kind: "actions-many",
            args: vec![named.to_string()],
        });
    }
    if effects.iter().any(|effect| effect.owner_unconfirmed) {
        facts.findings.push(finding(root, "owner-unconfirmed"));
    }
    let token_accounts = instructions.iter().filter(|step| creates_token_account(**step)).count();
    if token_accounts > 1 {
        facts.findings.push(SolanaFinding {
            node: root,
            kind: "token-accounts-many",
            args: vec![token_accounts.to_string()],
        });
    }

    let (base_fee, priority_fee) = (
        lamports_row(decoded, BASE_FEE_ROW),
        lamports_row(decoded, PRIORITY_FEE_ROW),
    );
    facts.fee_high = reader
        .known
        .is_some_and(|network| (base_fee as u128 + priority_fee as u128) > policy::fee_ceiling(network));
    nonce_facts(decoded, reader, &mut facts);
    token_facts(decoded, reader.manifest, reader.network, &mut facts);
    associated_facts(decoded, &mut facts);
    budget_facts(decoded, &mut facts);

    if let Some(range) = &reader.creation {
        facts.proven = instructions[range.clone()].iter().map(|step| step.id).collect();
    }
    Prepared {
        effects,
        title: if cancels {
            Some(ADVANCE_NONCE_TITLE)
        } else if reader.creation.is_some() {
            Some(CREATE_NONCE_TITLE)
        } else {
            None
        },
        facts,
    }
}

/// 路径不设白名单,只有全部硬化才能派生 ed25519 私钥.
fn request_path(request: &SignRequest, declared: Option<Network>) -> Result<DerivationPath, Forbid> {
    let [path] = request.paths.as_slice() else {
        return Err(Forbid::PathNotAllowed);
    };
    let path = DerivationPath::parse(path).map_err(|_| Forbid::PathNotAllowed)?;
    if path.0.len() > MAX_PATH_DEPTH {
        return Err(Forbid::PathNotAllowed);
    }
    check_solana_path(&path, declared).map_err(|error| match error {
        Error::PathNetworkMismatch => Forbid::PathNetworkMismatch,
        _ => Forbid::PathNotAllowed,
    })?;
    Ok(path)
}

/// 没有导出的路径保存的公钥算不出来,解锁后用私钥核对.
fn signer_key(ctx: &ReviewContext, path: &DerivationPath) -> Result<Option<[u8; 32]>, Forbid> {
    match ctx.keys.ed25519(path) {
        Ok(key) => Ok(Some(key)),
        Err(Forbid::PathNotAllowed) => Ok(None),
        Err(reason) => Err(reason),
    }
}

fn coin_of(network: Network) -> Coin {
    let (unit, decimals) = network.native_coin();
    Coin { unit, decimals }
}

pub fn review(declared: Option<Network>, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = request_path(request, declared)?;
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::MessageSigning);
    }
    let signer = signer_key(ctx, &path)?;
    let parsed = parse_solana(&request.payload);
    if let (Some(key), Ok(payload)) = (&signer, &parsed)
        && payload.signers().is_some_and(|required| !required.contains(key))
    {
        return Err(Forbid::AddressMismatch);
    }
    let network = request.network.as_str();
    let hint = request.hint.as_deref().and_then(|hint| parse_solana_address(hint).ok());
    let walker = SolanaWalker {
        coin: declared.map(coin_of),
        hint,
    };
    let mode = declared.map_or_else(|| solana_path_mode(&path).unwrap_or(Mode::Mainnet), Network::mode);
    let address = signer.as_ref().map(base58);
    let decoded = decode::decode(
        &walker,
        &decode::Input {
            chain: Chain::Solana,
            mode: Some(mode),
            network,
            kind: request.kind,
            paths: &request.paths,
            payload: &request.payload,
            signer: address.as_deref(),
        },
    );
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(failure) => return Ok(Review::undecodable(&request.network, &failure)),
    };
    let payload = parsed.map_err(|_| Forbid::ParseFailed)?;
    let native: Option<Cow<'_, Asset>> = declared.map(|network| ctx.native(network));
    let own = signer
        .map(|key| Ok((base58(&key), base58(&nonce_account_address(&key)?))))
        .transpose()
        .map_err(|_: Error| Forbid::AddressMismatch)?;
    let mut prepared = match &payload {
        SolanaPayload::Message(_) => {
            let mut reader = Reader {
                network,
                known: declared,
                manifest: ctx.manifest,
                native: native.as_deref(),
                own,
                creation: None,
                foreign_advances: Vec::new(),
                deposit_high: false,
            };
            prepared(&mut reader, &decoded)
        }
        SolanaPayload::Partial { .. } | SolanaPayload::Offchain { .. } | SolanaPayload::Unknown => Prepared::default(),
    };
    signer_facts(&payload, signer.as_ref(), &decoded, &mut prepared.facts);
    prepared.facts.signer = address;
    prepared.facts.unusual_path = (!is_usual_solana_path(&path)).then(|| path.to_string());
    let Prepared { effects, title, facts } = prepared;
    let assessment = policy::assess_solana(&decoded, &facts);
    let ranks = ranks(&decoded, &assessment.findings);
    let ranked = effects.into_iter().map(|effect| {
        let rank = ranks.get(&effect.node).copied().unwrap_or(0);
        (effect, rank)
    });
    let mut summary = summary::headline(ranked.collect());
    summary.payer = facts.signer;
    let assessed = Review::assessed(&decoded, assessment);
    if let Some(title) = title.filter(|_| assessed.risk == Risk::Recognized) {
        summary.title = title.to_string();
    }
    // `tx::review` 只给本机认识的网络检查清单.
    Ok(assessed.summarized(summary))
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

fn check_message_signature(payload: &[u8], signature: &[u8; 64], signer: &[u8; 32]) -> Result<(), Error> {
    let key = VerifyingKey::from_bytes(signer).map_err(|_| Error::InvalidKey)?;
    key.verify_strict(payload, &Signature::from_bytes(signature))
        .map_err(|_| Error::BadSignature)
}

/// 消息和未知的格式没有可以广播的形式,结果只有核验过的签名.
fn signature_only(payload: &[u8], signature: &[u8; 64], signer: &[u8]) -> Result<Verified, Error> {
    let signer: &[u8; 32] = signer.try_into().map_err(|_| Error::InvalidKey)?;
    check_message_signature(payload, signature, signer)?;
    Ok(Verified {
        raw: signature.to_vec(),
        hash: Sha256::digest(payload).to_lower_hex_string(),
        fee_unconfirmed: false,
        signature_only: true,
    })
}

/// `signer` 是手续费支付者的公钥,必须和消息里的账户 0 一致.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 64] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    let parsed = parse_solana(&request.payload).map_err(|_| Error::InvalidParams)?;
    let message = match parsed {
        SolanaPayload::Message(message) => message,
        SolanaPayload::Partial { .. } | SolanaPayload::Offchain { .. } | SolanaPayload::Unknown => {
            return signature_only(&request.payload, signature, signer);
        }
    };
    // 还缺其他人的签名时,这笔交易不能广播,结果只有本机的签名.
    if message.required_signatures != 1 {
        return signature_only(&request.payload, signature, signer);
    }
    let signer: &[u8; 32] = signer.try_into().map_err(|_| Error::InvalidKey)?;
    if message.keys[0] != *signer {
        return Err(Error::BadSignature);
    }
    check_message_signature(&request.payload, signature, signer)?;
    let mut raw = Vec::with_capacity(1 + 64 + request.payload.len());
    // v1 的签名在消息之后,数量由头部决定,没有长度前缀.
    if matches!(message.version, Version::V1(_)) {
        raw.extend_from_slice(&request.payload);
        raw.extend_from_slice(signature);
    } else {
        put_len(&mut raw, 1);
        raw.extend_from_slice(signature);
        raw.extend_from_slice(&request.payload);
    }
    Ok(Verified {
        raw,
        hash: bitcoin::base58::encode(signature),
        fee_unconfirmed: false,
        signature_only: false,
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

    /// 本机不认识的版本可以签,但这里只认交易消息.
    #[test]
    fn other_versions_are_not_transaction_messages() {
        let mut bytes = v0().encode();
        for first in [0x82, 0xfe, 0xff] {
            bytes[0] = first;
            assert_eq!(Message::decode(&bytes), Err(Forbid::Unsupported), "{first:#x}");
        }
        assert!(matches!(parse_solana(&bytes), Ok(SolanaPayload::Unknown)));
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

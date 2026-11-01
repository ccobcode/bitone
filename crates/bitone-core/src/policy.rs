//! 策略:风险类别,确认方式,规则表和阈值.
//!
//! `assess` 只读树,不修改树,拿不到原始载荷.新内容默认是无法识别,可以签名,不会默认变成普通转账.
//! 依赖方向由 `tests/architecture.rs` 检查.阈值是初始值.
//! Bitcoin,EVM,TRON,Solana 的类别和告警都在规则表里,`assess_bitcoin`,`assess_evm`,`assess_tron`,`assess_solana` 只算出命中了哪些种类.

use crate::decode::{Decoded, NodeId, SourceKind, Value, Visit, display};
use crate::{Chain, Mode, Network};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// 风险从低到高排序,一笔交易取全部动作里最高的一个.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Transfer,
    Recognized,
    Approval,
    PermissionChange,
    Unknown,
    Forbidden,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Transfer => "普通转账",
            Risk::Recognized => "已识别调用",
            Risk::Unknown => "无法识别",
            Risk::Approval => "授权",
            Risk::PermissionChange => "权限变更",
            Risk::Forbidden => "禁止签名",
        }
    }

    /// 页面顶部放哪个动作的排序,无法识别在授权之下:无法识别的指令没有金额和对方,不能盖住授权.
    pub(crate) fn headline_rank(self) -> u8 {
        match self {
            Risk::Transfer => 0,
            Risk::Recognized => 1,
            Risk::Unknown => 2,
            Risk::Approval => 3,
            Risk::PermissionChange => 4,
            Risk::Forbidden => 5,
        }
    }
}

/// 从低到高排序.告警和规则只升级,不降低.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confirm {
    /// 点按,再输 PIN.
    Tap,
    /// 长按 3 秒,再输 PIN.
    Hold,
    /// 长按 3 秒,再明确确认一次,再输 PIN.裸哈希和整段未知载荷走这里.
    HoldConfirm,
    /// 不可签名.
    Refuse,
}

impl Confirm {
    /// 告警只升级确认方式,不降低类别.
    pub fn of(risk: Risk, alerted: bool) -> Self {
        match risk {
            Risk::Forbidden => Confirm::Refuse,
            Risk::Unknown | Risk::Approval | Risk::PermissionChange => Confirm::Hold,
            Risk::Transfer | Risk::Recognized if alerted => Confirm::Hold,
            Risk::Transfer | Risk::Recognized => Confirm::Tap,
        }
    }
}

/// 规则能给的类别.禁止只来自完整性检查,规则表写不出来.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum RuleClass {
    Transfer,
    Recognized,
    Approval,
    PermissionChange,
    Unknown,
}

impl From<RuleClass> for Risk {
    fn from(class: RuleClass) -> Self {
        match class {
            RuleClass::Transfer => Risk::Transfer,
            RuleClass::Recognized => Risk::Recognized,
            RuleClass::Approval => Risk::Approval,
            RuleClass::PermissionChange => Risk::PermissionChange,
            RuleClass::Unknown => Risk::Unknown,
        }
    }
}

/// `alert` 进入告警列表,要长按.`note` 只解释这一行.里面的 `{0}`,`{1}` 由信号的参数替换,
/// 树里的动作只有 `{0}`,是这一行的显示值.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Rule {
    class: RuleClass,
    alert: Option<String>,
    note: Option<String>,
    /// 长按之后还要二次确认.
    #[serde(default)]
    second: bool,
    /// 不算动作,不影响类别.只有这种节点的交易和没有动作一样是无法识别.
    #[serde(default)]
    passive: bool,
    summary: Option<SummaryFields>,
}

/// 一种动作的页面顶部要读哪些字段.路径是节点名,用点分隔.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SummaryFields {
    pub amount: Option<String>,
    /// 金额的资产:Solana 是 mint,TRON 是 TRC-10 的编号.没有时金额按字段自己的单位显示.
    pub asset: Option<String>,
    /// 指令写明的小数位,和清单不一致时显示原始整数.
    pub decimals: Option<String>,
    /// 金额是 64 位最大值时表示无限额度.
    #[serde(default)]
    pub unlimited: bool,
    pub party: Option<String>,
    pub label: Option<String>,
    /// `party` 是代币账户时,它的所有者.没有这一行就是所有者没有确认.
    pub owner: Option<String>,
    pub next: Option<String>,
    pub current: Option<String>,
    /// `next` 的值写明没有时的含义.
    pub none: Option<String>,
    /// 新权限的全部持有人.有 `next` 之外的地址时要逐个核对.
    #[serde(default)]
    pub holders: Vec<String>,
}

/// 链的 ID 到动作种类名到规则.
type Rules = BTreeMap<String, BTreeMap<String, Rule>>;

static RULES: OnceLock<Rules> = OnceLock::new();

/// 规则表是内置数据,清单和描述文件都改不了.
fn rules() -> &'static Rules {
    RULES.get_or_init(|| serde_json::from_str(include_str!("../assets/rules.json")).expect("内置规则表有效"))
}

/// 这种动作的页面顶部要读的字段.
pub(crate) fn summary_fields(chain: Chain, kind: &str) -> Option<&'static SummaryFields> {
    rules().get(chain.id())?.get(kind)?.summary.as_ref()
}

/// 规则说这种节点不算动作.
pub(crate) fn is_passive(chain: Chain, kind: &str) -> bool {
    let rule = rules().get(chain.id()).and_then(|table| table.get(kind));
    rule.is_some_and(|rule| rule.passive)
}

/// 没有规则,或者内容不认识时的告警.
const UNKNOWN_ALERT: &str = "本机不认识这笔交易的部分内容,无法判断签名后果,签名前请确认来源和原始数据";
const UNKNOWN_NETWORK_ALERT: &str = "本机不认识这个网络,请确认网络名和交易内容";

/// 清单里的资产.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetFact {
    pub symbol: String,
    pub decimals: u8,
}

/// `assess` 读清单和本机状态的接口,`policy` 只依赖模型,不依赖 `manifest`,`keys`.
pub trait Facts {
    fn asset(&self, network: &str, contract: &str) -> Option<AssetFact>;
    /// 本机已签过的最高找零索引.按私钥类型分开记,所以不需要认识网络.
    fn highest_change(&self, mode: Mode) -> u32;
}

/// 策略按节点 ID 追加的发现.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finding {
    pub risk: Option<Risk>,
    pub note: Option<String>,
    /// 替换 `value` 的展示值,比如带符号的金额.
    pub display: Option<String>,
    /// 本账户标记.
    pub own: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub risk: Risk,
    pub confirm: Confirm,
    pub alerts: Vec<String>,
    pub findings: BTreeMap<NodeId, Finding>,
}

pub fn assess(decoded: &Decoded, _facts: &impl Facts) -> Assessment {
    assess_with(decoded, rules(), &[])
}

/// 策略从树和事实里得出的一项判定.种类名在规则表里,类别和告警都来自规则表.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signal {
    node: NodeId,
    kind: &'static str,
    args: Vec<String>,
}

impl Signal {
    fn new(node: NodeId, kind: &'static str, args: Vec<String>) -> Self {
        Self { node, kind, args }
    }
}

fn filled(text: &str, args: &[String]) -> String {
    args.iter().enumerate().fold(text.to_string(), |text, (index, arg)| {
        text.replace(&["{", &index.to_string(), "}"].concat(), arg)
    })
}

fn assess_with(decoded: &Decoded, rules: &Rules, signals: &[Signal]) -> Assessment {
    assess_quiet(decoded, rules, signals, &[])
}

/// `proven` 是 `tx` 层证明属于本钱包的节点,它们的规则告警不适用,类别不变.
fn assess_quiet(decoded: &Decoded, rules: &Rules, signals: &[Signal], proven: &[NodeId]) -> Assessment {
    let table = rules.get(decoded.chain().id());
    let actions = decoded.actions();
    let mut findings: BTreeMap<NodeId, Finding> = BTreeMap::new();
    let mut alerts: Vec<String> = Vec::new();
    let mut risks = Vec::new();
    let mut unknown = actions.is_empty();
    // 整段未知的载荷在长按之后还要二次确认.
    let mut second = matches!(decoded.root().value, Value::Opaque(_));
    let actions = actions
        .into_iter()
        .map(|(id, node, kind)| (id, kind, node.source.kind().is_evidence(), vec![display(&node.value)]));
    let signals = signals
        .iter()
        .map(|signal| (signal.node, signal.kind, true, signal.args.clone()));
    for (id, kind, evidence, args) in actions.chain(signals) {
        let Some(rule) = table.and_then(|table| table.get(kind)).filter(|_| evidence) else {
            unknown = true;
            findings.entry(id).or_default().risk = Some(Risk::Unknown);
            continue;
        };
        if rule.passive {
            continue;
        }
        let risk = Risk::from(rule.class);
        risks.push(risk);
        second |= rule.second;
        let alert = rule.alert.as_deref().filter(|_| !proven.contains(&id));
        let alert = alert.map(|text| filled(text, &args));
        let note = rule.note.as_deref().map(|text| filled(text, &args)).or(alert.clone());
        let finding = findings.entry(id).or_default();
        finding.risk = finding.risk.max(Some(risk));
        finding.note = finding.note.take().or(note);
        if let Some(alert) = alert.filter(|alert| !alerts.contains(alert)) {
            alerts.push(alert);
        }
    }
    unknown |= risks.is_empty();
    // 进入签名原文的 `opaque` 字节含义不明,动作的规则不能替它担保.
    for visit in decoded.visits() {
        let opaque = matches!(&visit.node.value, Value::Opaque(bytes) if !bytes.is_empty());
        if opaque && visit.node.source.kind() == SourceKind::Signed {
            unknown = true;
            findings.entry(visit.id).or_default().risk = Some(Risk::Unknown);
        }
    }
    if unknown {
        alerts.insert(0, UNKNOWN_ALERT.to_string());
    }
    let foreign_network = Network::from_id(decoded.network()).is_none();
    if foreign_network {
        alerts.insert(0, UNKNOWN_NETWORK_ALERT.to_string());
    }
    if unknown || foreign_network {
        risks.push(Risk::Unknown);
    }
    let risk = risks.into_iter().max().unwrap_or(Risk::Unknown);
    let confirm = Confirm::of(risk, !alerts.is_empty());
    Assessment {
        risk,
        confirm: if second {
            confirm.max(Confirm::HoldConfirm)
        } else {
            confirm
        },
        alerts,
        findings,
    }
}

/// Bitcoin 的一个本账户条目.树里只有声明,`verified` 表示已经用保存的公钥重新算出同一个脚本.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnEntry {
    pub path: String,
    pub verified: bool,
    pub usual: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpendFact {
    pub own: Option<OwnEntry>,
    /// 这个签名不绑定全部内容时,sighash 的名字.
    pub unbound_sighash: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputFact {
    pub own: Option<OwnEntry>,
    pub sats: u64,
    /// 标准找零链上的索引.
    pub change_index: Option<u32>,
}

/// Bitcoin 的事实.重新派生和前序交易都由 `tx` 层完成,这里不碰密钥和载荷.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitcoinFacts {
    pub spends: Vec<SpendFact>,
    pub outputs: Vec<OutputFact>,
    pub fee: u64,
    /// 有输入的金额没有经过前序交易确认,签名也不能替它担保.
    pub fee_unconfirmed: bool,
    pub highest_change: u32,
}

const ROOT: NodeId = 0;
const INPUT_ACTION: &str = "input";
pub(crate) const OUTPUT_PREFIX: &str = "output-";
const FEE_ROW: &str = "fee";

impl BitcoinFacts {
    fn sent(&self) -> u64 {
        self.outputs
            .iter()
            .filter(|output| !output.is_own())
            .map(|output| output.sats)
            .sum()
    }

    /// 手续费和发送额比较,这里的金额都已经过总量上限检查.
    fn fee_is_high(&self) -> bool {
        let percent = u128::from(self.fee) * 100 > u128::from(self.sent()) * u128::from(BITCOIN_FEE_PERCENT);
        percent || u128::from(self.fee) > fee_ceiling(Network::Bitcoin)
    }
}

impl OutputFact {
    /// 用公钥算出来才算本钱包.
    fn is_own(&self) -> bool {
        self.own.as_ref().is_some_and(|own| own.verified)
    }
}

fn spend_signals(index: usize, node: NodeId, fact: &SpendFact, signals: &mut Vec<Signal>) {
    let ordinal = (index + 1).to_string();
    if let Some(own) = &fact.own {
        if !own.verified {
            signals.push(Signal::new(
                node,
                "input-path-unverified",
                vec![ordinal.clone(), own.path.clone()],
            ));
        } else if !own.usual {
            signals.push(Signal::new(
                node,
                "input-path-unusual",
                vec![ordinal.clone(), own.path.clone()],
            ));
        }
    }
    if let Some(sighash) = &fact.unbound_sighash {
        signals.push(Signal::new(
            node,
            "input-sighash-unbound",
            vec![ordinal, sighash.clone()],
        ));
    }
}

fn output_signals(node: NodeId, fact: &OutputFact, highest: u32, signals: &mut Vec<Signal>) {
    let Some(own) = &fact.own else {
        return;
    };
    if !own.verified {
        signals.push(Signal::new(node, "output-claim-unverified", vec![own.path.clone()]));
    } else if !own.usual {
        signals.push(Signal::new(node, "change-path-unusual", vec![own.path.clone()]));
    }
    if let Some(index) = fact
        .change_index
        .filter(|index| *index > highest.saturating_add(CHANGE_GAP))
    {
        signals.push(Signal::new(node, "change-index-far", vec![index.to_string()]));
    }
}

/// Bitcoin 的策略.输入输出的顺序就是树里动作的顺序,规则都在规则表里.
pub fn assess_bitcoin(decoded: &Decoded, facts: &BitcoinFacts) -> Assessment {
    let actions = decoded.actions();
    let nodes = |matches: &dyn Fn(&str) -> bool| -> Vec<NodeId> {
        actions
            .iter()
            .filter(|(_, _, kind)| matches(kind))
            .map(|(id, _, _)| *id)
            .collect()
    };
    let inputs = nodes(&|kind| kind == INPUT_ACTION);
    let outputs = nodes(&|kind| kind.starts_with(OUTPUT_PREFIX));
    let mut signals = Vec::new();
    let foreign = facts.spends.iter().filter(|spend| spend.own.is_none()).count();
    if foreign > 0 {
        signals.push(Signal::new(ROOT, "input-foreign", vec![foreign.to_string()]));
    }
    for (index, (node, fact)) in inputs.iter().zip(&facts.spends).enumerate() {
        spend_signals(index, *node, fact, &mut signals);
    }
    for (node, fact) in outputs.iter().zip(&facts.outputs) {
        output_signals(*node, fact, facts.highest_change, &mut signals);
    }
    let visits = decoded.visits();
    let fee_row = visits
        .iter()
        .find(|visit| visit.node.name == FEE_ROW)
        .map_or(ROOT, |visit| visit.id);
    if facts.fee_unconfirmed {
        signals.push(Signal::new(fee_row, "fee-unconfirmed", Vec::new()));
    }
    let recipients = facts.outputs.iter().filter(|output| !output.is_own()).count();
    if recipients > 1 {
        signals.push(Signal::new(ROOT, "recipients-many", vec![recipients.to_string()]));
    }
    if facts.fee_is_high() {
        signals.push(Signal::new(fee_row, "fee-high", Vec::new()));
    }
    let mut assessment = assess_with(decoded, rules(), &signals);
    let own_rows = inputs.iter().zip(facts.spends.iter().map(|spend| spend.own.as_ref()));
    let own_rows = own_rows.chain(
        outputs
            .iter()
            .zip(facts.outputs.iter().map(|output| output.own.as_ref())),
    );
    for (node, own) in own_rows {
        if let Some(own) = own {
            let finding = assessment.findings.entry(*node).or_default();
            finding.own = own.verified;
            finding.display = Some(own.path.clone());
        }
    }
    assessment
}

pub(crate) const CALL_ACTION: &str = "call";
const FEE_CAP_ROW: &str = "fee_cap";
const NETWORK_ROW: &str = "network";
const PATH_ROW: &str = "derivation_path";
const PATH_KIND: &str = "path-unusual";
const SOLANA_FEE_ROW: &str = "priority_fee";

/// 一次合约调用是怎么解开的.描述文件只决定展示,类别由这里和规则表决定.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CallShape {
    /// 没有描述,或者按没有描述处理.
    #[default]
    Undecoded,
    /// 选择器是这个标准函数,调用数据不是规范编码,参数按旧版编译器的读法取值.
    Irregular(String),
    /// 这个标准函数,里面是规范签名.
    Standard(String),
    /// 清单的描述文件.
    Described,
}

/// 一次合约调用的事实.嵌入调用各有一条.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallFact {
    pub shape: CallShape,
    pub contract: String,
    /// 目标合约在清单里.
    pub listed: bool,
    /// 这次调用同时转了原生币.
    pub has_value: bool,
    pub unlimited: bool,
}

/// EVM 的事实.路径,合约调用和网络都由 `tx` 层算出,这里不碰密钥和载荷.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvmFacts {
    pub signer: Option<String>,
    /// 这条派生路径不是本机导出的账户.
    pub unusual_path: Option<String>,
    /// 这笔交易的全部合约调用.
    pub calls: Vec<CallFact>,
    /// 调用的方法和合约,显示在 `data` 这一行.
    pub call_note: Option<String>,
    pub fee_high: bool,
    /// 请求里的网络名和 Chain ID 认出的网络.
    pub renamed: Option<(String, String)>,
    /// 谁也不认识的 Chain ID 和请求里的网络名.
    pub unknown_chain: Option<(String, String)>,
    pub permit: Option<PermitFact>,
}

/// 一条 Permit 消息里的授权.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermitFact {
    pub grants: usize,
    pub unlimited: usize,
}

impl PermitFact {
    fn signals(&self, signals: &mut Vec<Signal>) {
        if self.grants > 1 {
            signals.push(Signal::new(ROOT, "permit-many", vec![self.grants.to_string()]));
            if self.unlimited > 0 {
                signals.push(Signal::new(ROOT, "permit-unlimited", vec![self.unlimited.to_string()]));
            }
        }
    }
}

const FUNCTION_PREFIX: &str = "fn:";

/// 标准函数在规则表里的种类.规则表是 `'static` 的,所以种类名不用复制.
fn function_kind(chain: Chain, signature: &str) -> Option<&'static str> {
    let key = [FUNCTION_PREFIX, signature].concat();
    let table = rules().get(chain.id())?;
    table.get_key_value(&key).map(|(kind, _)| kind.as_str())
}

/// 附带原生币的调用不适用转账和已知函数的展示,授权类调用的类别更高,不变.
fn plain_class(chain: Chain, kind: &str) -> bool {
    let rule = rules().get(chain.id()).and_then(|table| table.get(kind));
    rule.is_some_and(|rule| matches!(rule.class, RuleClass::Transfer | RuleClass::Recognized))
}

fn call_kinds(chain: Chain, call: &CallFact) -> Vec<(&'static str, Vec<String>)> {
    let mut kinds = match &call.shape {
        CallShape::Undecoded => vec![("call-unknown", Vec::new())],
        CallShape::Irregular(signature) if call.listed => vec![("call-irregular", vec![signature.clone()])],
        CallShape::Irregular(signature) => vec![
            ("call-irregular", vec![signature.clone()]),
            ("call-unlisted", vec![call.contract.clone()]),
        ],
        CallShape::Described if call.has_value => vec![("call-with-value", Vec::new())],
        CallShape::Described => vec![("call-recognized", Vec::new())],
        CallShape::Standard(_) if !call.listed => vec![("call-unlisted", vec![call.contract.clone()])],
        CallShape::Standard(signature) => match function_kind(chain, signature) {
            Some(kind) if call.has_value && plain_class(chain, kind) => vec![("call-with-value", Vec::new())],
            Some(kind) => vec![(kind, Vec::new())],
            None => vec![("call-unknown", Vec::new())],
        },
    };
    if call.unlimited {
        kinds.push(("call-unlimited", Vec::new()));
    }
    kinds
}

/// 一次调用的类别,页面顶部按它选动作.
pub fn call_risk(chain: Chain, call: &CallFact) -> Risk {
    let table = rules().get(chain.id());
    let classes = call_kinds(chain, call).into_iter().map(|(kind, _)| {
        let rule = table.and_then(|table| table.get(kind));
        rule.map_or(Risk::Unknown, |rule| Risk::from(rule.class))
    });
    classes.max().unwrap_or(Risk::Unknown)
}

fn call_signals(chain: Chain, node: NodeId, calls: &[CallFact], signals: &mut Vec<Signal>) {
    // 没有事实的调用不能因为规则表里 `call` 是最低类别就过关.
    if calls.is_empty() {
        signals.push(Signal::new(node, "call-unknown", Vec::new()));
    }
    let kinds = calls.iter().flat_map(|call| call_kinds(chain, call));
    signals.extend(kinds.map(|(kind, args)| Signal::new(node, kind, args)));
}

/// EVM 的策略.类别和告警都在规则表里.
pub fn assess_evm(decoded: &Decoded, facts: &EvmFacts) -> Assessment {
    let visits = decoded.visits();
    let row = |name: &str| {
        visits
            .iter()
            .find(|visit| visit.node.name == name)
            .map_or(ROOT, |visit| visit.id)
    };
    let call = decoded
        .actions()
        .into_iter()
        .find_map(|(id, _, kind)| (kind == CALL_ACTION).then_some(id));
    let mut signals = Vec::new();
    if let Some(node) = call {
        call_signals(decoded.chain(), node, &facts.calls, &mut signals);
    }
    if facts.fee_high {
        signals.push(Signal::new(row(FEE_CAP_ROW), "fee-high", Vec::new()));
    }
    if let Some(path) = &facts.unusual_path {
        signals.push(Signal::new(row(PATH_ROW), PATH_KIND, vec![path.clone()]));
    }
    if let Some((supplied, shown)) = &facts.renamed {
        let args = vec![supplied.clone(), shown.clone()];
        signals.push(Signal::new(row(NETWORK_ROW), "network-renamed", args));
    }
    if let Some((chain_id, supplied)) = &facts.unknown_chain {
        let args = vec![chain_id.clone(), supplied.clone()];
        signals.push(Signal::new(row(NETWORK_ROW), "chain-unknown", args));
    }
    if let Some(permit) = &facts.permit {
        permit.signals(&mut signals);
    }
    let mut assessment = assess_with(decoded, rules(), &signals);
    if let (Some(node), Some(note)) = (call, &facts.call_note) {
        assessment.findings.entry(node).or_default().note = Some(note.clone());
    }
    mark_own(&mut assessment, &visits, facts.signer.as_ref());
    assessment
}

fn mark_own(assessment: &mut Assessment, visits: &[Visit<'_>], signer: Option<&String>) {
    for visit in visits {
        let own = matches!(&visit.node.value, Value::Address(address) if Some(address) == signer);
        if own {
            assessment.findings.entry(visit.id).or_default().own = true;
        }
    }
}

pub(crate) const TRIGGER_ACTION: &str = "TriggerSmartContract";
pub(crate) const ASSET_ACTION: &str = "TransferAssetContract";
const AMOUNT_ROW: &str = "amount";
const EXPIRATION_ROW: &str = "expiration";
const TIMESTAMP_ROW: &str = "timestamp";
const FEE_LIMIT_ROW: &str = "fee_limit";
const OWNER_ROW: &str = "owner_address";

/// 节点接受的最长有效期.离线设备没有时钟,所以比较的是 `timestamp`.
pub const MAX_VALIDITY_MS: i64 = 24 * 60 * 60 * 1000;

/// `expiration` 在 `timestamp` 之后的 24 小时内.节点比较的是区块时间,不看 `timestamp`,所以这里只能告警.
pub fn expiration_in_window(expiration: i64, timestamp: i64) -> bool {
    let span = i128::from(expiration) - i128::from(timestamp);
    span > 0 && span <= i128::from(MAX_VALIDITY_MS)
}

/// `owner_address` 和签名账户的关系.里面是发起方的地址.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OwnerFact {
    /// 就是签名账户,或者这种合约没有发起方.
    #[default]
    Own,
    /// 不是签名账户,`Permission_id` 不为 0,这是 TRON 的多签.
    Foreign(String),
    /// 保存的公钥算不出这条路径的地址,解锁后核对,不符时拒绝.
    Unverified(String),
    /// 同上,但 `Permission_id` 不为 0,解锁后也不会拒绝.
    UnverifiedMultisig(String),
}

impl OwnerFact {
    fn signal(&self, node: NodeId) -> Option<Signal> {
        let (kind, owner) = match self {
            Self::Own => return None,
            Self::Foreign(owner) => ("owner-foreign", owner),
            Self::Unverified(owner) => ("owner-unverified", owner),
            Self::UnverifiedMultisig(owner) => ("owner-multisig-unverified", owner),
        };
        Some(Signal::new(node, kind, vec![owner.clone()]))
    }
}

/// TRON 的事实.路径,合约调用和账户都由 `tx` 层算出,这里不碰密钥和载荷.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TronFacts {
    pub signer: Option<String>,
    /// 这条派生路径不是本机导出的账户.
    pub unusual_path: Option<String>,
    /// 这笔交易的全部合约调用.
    pub calls: Vec<CallFact>,
    /// 调用的方法和合约.
    pub call_note: Option<String>,
    pub fee_high: bool,
    pub owner: OwnerFact,
    /// 新权限里有 owner 第一个地址之外的地址.
    pub permission_shared: bool,
    /// TRC-10 在清单内时,按清单小数位显示的金额.
    pub listed_asset: Option<String>,
    pub permit: Option<PermitFact>,
}

fn time_of(visits: &[Visit<'_>], name: &str) -> i64 {
    let visit = visits.iter().find(|visit| visit.node.name == name);
    match visit.map(|visit| &visit.node.value) {
        Some(Value::Time(millis)) => *millis,
        _ => 0,
    }
}

/// TRON 的消息和裸哈希.没有交易的字段,规则都在树的动作上.
pub fn assess_tron_message(decoded: &Decoded, facts: &TronFacts) -> Assessment {
    let visits = decoded.visits();
    let mut signals = Vec::new();
    if let Some(path) = &facts.unusual_path {
        let row = visits.iter().find(|visit| visit.node.name == PATH_ROW);
        let node = row.map_or(ROOT, |visit| visit.id);
        signals.push(Signal::new(node, PATH_KIND, vec![path.clone()]));
    }
    if let Some(permit) = &facts.permit {
        permit.signals(&mut signals);
    }
    let mut assessment = assess_with(decoded, rules(), &signals);
    mark_own(&mut assessment, &visits, facts.signer.as_ref());
    assessment
}

/// TRON 的策略.类别和告警都在规则表里.
pub fn assess_tron(decoded: &Decoded, facts: &TronFacts) -> Assessment {
    let visits = decoded.visits();
    let row = |name: &str| {
        visits
            .iter()
            .find(|visit| visit.node.name == name)
            .map_or(ROOT, |visit| visit.id)
    };
    let actions = decoded.actions();
    let action = |wanted: &str| {
        actions
            .iter()
            .find_map(|(id, _, kind)| (*kind == wanted).then_some(*id))
    };
    let call = action(TRIGGER_ACTION);
    let mut signals = Vec::new();
    if let Some(node) = call {
        call_signals(decoded.chain(), node, &facts.calls, &mut signals);
    }
    if let Some(node) = action(ASSET_ACTION).filter(|_| facts.listed_asset.is_none()) {
        signals.push(Signal::new(node, "asset-unlisted", Vec::new()));
    }
    if facts.fee_high {
        signals.push(Signal::new(row(FEE_LIMIT_ROW), "fee-high", Vec::new()));
    }
    if let Some(path) = &facts.unusual_path {
        signals.push(Signal::new(row(PATH_ROW), PATH_KIND, vec![path.clone()]));
    }
    signals.extend(facts.owner.signal(row(OWNER_ROW)));
    if facts.permission_shared {
        signals.push(Signal::new(ROOT, "permission-shared", Vec::new()));
    }
    if !expiration_in_window(time_of(&visits, EXPIRATION_ROW), time_of(&visits, TIMESTAMP_ROW)) {
        signals.push(Signal::new(row(EXPIRATION_ROW), "expiration-window", Vec::new()));
    }
    let mut assessment = assess_with(decoded, rules(), &signals);
    if let (Some(node), Some(note)) = (call, &facts.call_note) {
        assessment.findings.entry(node).or_default().note = Some(note.clone());
    }
    if let Some(amount) = &facts.listed_asset {
        assessment.findings.entry(row(AMOUNT_ROW)).or_default().display = Some(amount.clone());
    }
    mark_own(&mut assessment, &visits, facts.signer.as_ref());
    assessment
}

/// Solana 的一项判定.种类名在规则表里.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolanaFinding {
    pub node: NodeId,
    pub kind: &'static str,
    pub args: Vec<String>,
}

/// Solana 的事实.路径,清单和本账户的核对都由 `tx` 层完成,这里不碰密钥和载荷.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SolanaFacts {
    pub signer: Option<String>,
    /// 这条派生路径不是本机导出的账户.
    pub unusual_path: Option<String>,
    pub fee_high: bool,
    pub findings: Vec<SolanaFinding>,
    /// 清单内代币的金额,带符号.
    pub displays: Vec<(NodeId, String)>,
    /// 本钱包 nonce 创建的指令,地址和 authority 都核对过.
    pub proven: Vec<NodeId>,
}

/// Solana 的策略.类别和告警都在规则表里.
pub fn assess_solana(decoded: &Decoded, facts: &SolanaFacts) -> Assessment {
    let visits = decoded.visits();
    let row = |name: &str| {
        visits
            .iter()
            .find(|visit| visit.node.name == name)
            .map_or(ROOT, |visit| visit.id)
    };
    let mut signals: Vec<Signal> = facts
        .findings
        .iter()
        .map(|finding| Signal::new(finding.node, finding.kind, finding.args.clone()))
        .collect();
    if facts.fee_high {
        signals.push(Signal::new(row(SOLANA_FEE_ROW), "fee-high", Vec::new()));
    }
    if let Some(path) = &facts.unusual_path {
        signals.push(Signal::new(row(PATH_ROW), PATH_KIND, vec![path.clone()]));
    }
    let mut assessment = assess_quiet(decoded, rules(), &signals, &facts.proven);
    for (node, display) in &facts.displays {
        assessment.findings.entry(*node).or_default().display = Some(display.clone());
    }
    for visit in &visits {
        let own = matches!(&visit.node.value, Value::Address(address) if Some(address) == facts.signer.as_ref());
        if own {
            assessment.findings.entry(visit.id).or_default().own = true;
        }
    }
    assessment
}

/// 找零索引超过已签过的最高索引加这个数时按高风险处理.
pub const CHANGE_GAP: u32 = 20;

/// 手续费上限,单位是最小单位.超过时进入高风险样式.
pub fn fee_ceiling(network: Network) -> u128 {
    use Network::*;
    match network {
        Bitcoin | BtcTestnet4 | BtcSignet | BtcTestnet3 => 100_000,
        Ethereum | Sepolia | Hoodi => 50_000_000_000_000_000,
        Bsc | BscTestnet => 20_000_000_000_000_000,
        Polygon | Amoy => 20_000_000_000_000_000_000,
        Tron | TronNile | TronShasta => 200_000_000,
        Solana | SolanaDevnet | SolanaTestnet => 10_000_000,
    }
}

/// Bitcoin 手续费超过发送额的这个百分比时按高风险处理.
pub const BITCOIN_FEE_PERCENT: u64 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::fixture::{PARTIAL, PAYLOAD, Records, Tampered, UNKNOWN_FORMAT, input};
    use crate::decode::{Node, Source, Walked, decode};

    fn table(rows: &str) -> Rules {
        serde_json::from_str(&format!("{{\"evm\":{{{rows}}}}}")).unwrap()
    }

    fn assessed(payload: &[u8], rows: &str) -> Assessment {
        assess_with(&decode(&Records, &input(payload)).unwrap(), &table(rows), &[])
    }

    const TRANSFER_RULE: &str = "\"transfer\":{\"class\":\"transfer\",\"alert\":null}";

    #[test]
    fn the_order_is_the_standard_order() {
        let order = [
            Risk::Transfer,
            Risk::Recognized,
            Risk::Approval,
            Risk::PermissionChange,
            Risk::Unknown,
            Risk::Forbidden,
        ];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_headline_prefers_a_known_approval_to_an_unknown_action() {
        let order = [
            Risk::Transfer,
            Risk::Recognized,
            Risk::Unknown,
            Risk::Approval,
            Risk::PermissionChange,
            Risk::Forbidden,
        ];
        assert!(
            order
                .windows(2)
                .all(|pair| pair[0].headline_rank() < pair[1].headline_rank())
        );
    }

    #[test]
    fn confirm_follows_the_table() {
        assert_eq!(Confirm::of(Risk::Transfer, false), Confirm::Tap);
        assert_eq!(Confirm::of(Risk::Recognized, false), Confirm::Tap);
        assert_eq!(Confirm::of(Risk::Transfer, true), Confirm::Hold);
        assert_eq!(Confirm::of(Risk::Recognized, true), Confirm::Hold);
        assert_eq!(Confirm::of(Risk::Approval, false), Confirm::Hold);
        assert_eq!(Confirm::of(Risk::PermissionChange, false), Confirm::Hold);
        assert_eq!(Confirm::of(Risk::Unknown, false), Confirm::Hold);
        assert_eq!(Confirm::of(Risk::Forbidden, false), Confirm::Refuse);
    }

    #[test]
    fn the_second_confirmation_sits_between_hold_and_refuse() {
        assert!(Confirm::Hold < Confirm::HoldConfirm && Confirm::HoldConfirm < Confirm::Refuse);
    }

    #[test]
    fn a_rule_gives_the_class() {
        let assessment = assessed(&PAYLOAD, TRANSFER_RULE);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Transfer, Confirm::Tap));
        assert_eq!(assessment.findings[&2].risk, Some(Risk::Transfer));
        assert!(assessment.alerts.is_empty());
    }

    #[test]
    fn a_node_without_a_rule_is_unknown_and_signable() {
        let assessment = assessed(&PAYLOAD, "");
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.findings[&2].risk, Some(Risk::Unknown));
        assert_eq!(assessment.alerts, [UNKNOWN_ALERT]);
    }

    #[test]
    fn a_network_this_build_does_not_know_is_unknown_even_with_a_plain_rule() {
        let local = crate::decode::Input {
            network: "local-debug",
            ..input(&PAYLOAD)
        };
        let decoded = decode(&Records, &local).unwrap();
        let assessment = assess_with(&decoded, &table(TRANSFER_RULE), &[]);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.alerts, [UNKNOWN_NETWORK_ALERT]);
    }

    #[test]
    fn a_chain_without_a_table_is_unknown() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        assert_eq!(assess_with(&decoded, &Rules::new(), &[]).risk, Risk::Unknown);
    }

    #[test]
    fn no_actions_is_unknown_never_plain() {
        let assessment = assessed(&[1, 2, b'o', b'k'], TRANSFER_RULE);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
    }

    #[test]
    fn an_unknown_payload_is_signable_after_a_second_confirmation() {
        let assessment = assessed(&[UNKNOWN_FORMAT, 1], TRANSFER_RULE);
        assert_eq!(
            (assessment.risk, assessment.confirm),
            (Risk::Unknown, Confirm::HoldConfirm)
        );
        assert_eq!(assessment.findings[&0].risk, Some(Risk::Unknown));
        assert_eq!(assessment.alerts, [UNKNOWN_ALERT]);
    }

    #[test]
    fn a_rule_can_ask_for_the_second_confirmation() {
        let rule = "\"transfer\":{\"class\":\"unknown\",\"alert\":\"后果\",\"second\":true}";
        assert_eq!(assessed(&PAYLOAD, rule).confirm, Confirm::HoldConfirm);
    }

    #[test]
    fn an_opaque_rest_is_a_long_press_not_a_second_confirmation() {
        assert_eq!(assessed(&PARTIAL, TRANSFER_RULE).confirm, Confirm::Hold);
    }

    #[test]
    fn an_opaque_rest_cannot_hide_behind_a_plain_action() {
        let rules = format!("{TRANSFER_RULE},\"tag-9\":{{\"class\":\"transfer\",\"alert\":null}}");
        let assessment = assessed(&PARTIAL, &rules);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.findings[&3].risk, Some(Risk::Unknown));
    }

    #[test]
    fn unknown_outranks_a_permission_change() {
        let rules = "\"transfer\":{\"class\":\"permission-change\",\"alert\":null}";
        assert_eq!(assessed(&PARTIAL, rules).risk, Risk::Unknown);
    }

    #[test]
    fn a_rule_can_mark_a_dangerous_action_unknown_with_its_own_alert() {
        let assessment = assessed(&PAYLOAD, "\"transfer\":{\"class\":\"unknown\",\"alert\":\"后果\"}");
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.alerts, ["后果"]);
        assert_eq!(assessment.findings[&2].note.as_deref(), Some("后果"));
    }

    #[test]
    fn a_rule_alert_raises_the_confirmation_not_the_class() {
        let assessment = assessed(&PAYLOAD, "\"transfer\":{\"class\":\"transfer\",\"alert\":\"费用\"}");
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Transfer, Confirm::Hold));
    }

    #[test]
    fn the_rule_table_cannot_refuse() {
        let result: Result<Rules, _> =
            serde_json::from_str("{\"evm\":{\"x\":{\"class\":\"forbidden\",\"alert\":null}}}");
        assert!(result.is_err());
    }

    #[test]
    fn a_hint_cannot_earn_a_class() {
        let decoded = decode(
            &Tampered(|walked: &mut Walked| {
                let hinted = Node::new("hinted", Value::Struct, Source::Hint).action("transfer");
                walked.root.children.push(hinted);
            }),
            &input(&[1, 2, b'o', b'k']),
        )
        .unwrap();
        assert_eq!(assess_with(&decoded, &table(TRANSFER_RULE), &[]).risk, Risk::Unknown);
    }

    #[test]
    fn the_builtin_table_parses() {
        assert!(rules().contains_key("evm"));
    }

    #[test]
    fn only_the_migrated_chains_have_tables() {
        for (chain, table) in rules() {
            let migrated = ["bitcoin", "evm", "solana", "tron"].contains(&chain.as_str());
            assert_eq!(table.is_empty(), !migrated, "{chain}");
        }
    }

    const EVM_KINDS: [&str; 33] = [
        "no-chain-id",
        "native-transfer",
        "contract-creation",
        "call",
        "call-recognized",
        "call-unknown",
        "call-unlisted",
        "call-irregular",
        "call-with-value",
        "call-unlimited",
        "authorization",
        "authorization-clear",
        "authorization-every-chain",
        "authorization-other-chain",
        "authorization-unrecovered",
        "eip7702-authorization",
        "eip712-digest",
        "personal-sign",
        "eip191-validator",
        "eip191-unknown",
        "raw-hash",
        "path-unusual",
        "fee-high",
        "network-renamed",
        "chain-unknown",
        "eip712-message",
        "eip712-permit",
        "eip712-described",
        "eip712-no-chain-id",
        "eip712-no-contract",
        "eip712-other-chain",
        "permit-many",
        "permit-unlimited",
    ];

    #[test]
    fn every_evm_kind_has_a_rule() {
        let table = &rules()["evm"];
        for kind in EVM_KINDS.into_iter().chain(crate::decode::transaction_actions()) {
            assert!(table.contains_key(kind), "{kind}");
        }
        assert_eq!(
            table.len(),
            EVM_KINDS.len() + 5 + function_kinds("evm").len(),
            "规则表里有用不到的种类"
        );
    }

    /// 策略不依赖描述文件,函数名在这里和规则表自己比较,`tx/abi.rs` 的测试比较内置函数.
    fn function_kinds(chain: &str) -> Vec<&'static String> {
        let table = &rules()[chain];
        table.keys().filter(|kind| kind.starts_with(FUNCTION_PREFIX)).collect()
    }

    /// 这些内容都是无法识别,告警要说清后果.
    #[test]
    fn dangerous_evm_content_is_unknown_with_an_alert() {
        let table = &rules()["evm"];
        for kind in [
            "tx-eip4844",
            "no-chain-id",
            "authorization-every-chain",
            "authorization-other-chain",
            "authorization-unrecovered",
            "eip7702-authorization",
            "eip712-digest",
            "personal-sign",
            "raw-hash",
            "chain-unknown",
            "eip712-message",
            "eip712-no-chain-id",
            "eip712-no-contract",
            "eip712-other-chain",
        ] {
            assert_eq!(table[kind].class, RuleClass::Unknown, "{kind}");
            assert!(table[kind].alert.is_some(), "{kind}");
        }
    }

    #[test]
    fn only_blind_payloads_ask_for_the_second_confirmation() {
        let second: Vec<&str> = rules()["evm"]
            .iter()
            .filter(|(_, rule)| rule.second)
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(second, ["eip191-unknown", "eip712-digest", "raw-hash"]);
    }

    const SOLANA_KINDS: [&str; 29] = [
        "program-unknown",
        "instruction-unknown",
        "lookup-table",
        "lookup-account",
        "fee-payer-foreign",
        "signer-unverified",
        "other-signers",
        "path-unusual",
        "fee-high",
        "actions-many",
        "owner-unconfirmed",
        "token-accounts-many",
        "token-unlisted",
        "token-decimals-mismatch",
        "token-2022-undeclared",
        "ata-underived",
        "compute-budget-repeated",
        "nonce-cancel",
        "nonce-deposit-high",
        "nonce-advance-foreign",
        "nonce-step-foreign",
        "v1-transaction-size",
        "v1-signatures",
        "v1-addresses",
        "v1-instructions",
        "v1-heap-size",
        "v1-config-unknown",
        "offchain-message",
        "offchain-unknown",
    ];

    /// 官方枚举的每种指令都要有规则.重新生成 IDL 后出现新指令时这里失败.
    #[test]
    fn every_solana_kind_has_a_rule() {
        let table = &rules()["solana"];
        let actions = crate::decode::solana::instruction_actions();
        for kind in actions.iter().map(String::as_str).chain(SOLANA_KINDS) {
            assert!(table.contains_key(kind), "{kind}");
        }
        let unique: std::collections::BTreeSet<&str> = SOLANA_KINDS.into_iter().collect();
        assert_eq!(table.len(), actions.len() + unique.len(), "规则表里有用不到的种类");
    }

    #[test]
    fn solana_matches_the_standard_matrix() {
        let table = &rules()["solana"];
        let classes = [
            ("system.transferSol", RuleClass::Transfer),
            ("system.assignWithSeed", RuleClass::PermissionChange),
            ("system.authorizeNonceAccount", RuleClass::PermissionChange),
            ("token.transferChecked", RuleClass::Transfer),
            ("token.approve", RuleClass::Approval),
            ("token2022.approveChecked", RuleClass::Approval),
            ("token.setAuthority", RuleClass::PermissionChange),
            ("token2022.updateTransferHook", RuleClass::PermissionChange),
            ("token2022.initializePermanentDelegate", RuleClass::PermissionChange),
            ("token2022.confidentialTransfer", RuleClass::Unknown),
            ("token2022.permissionedConfidentialBurn", RuleClass::Unknown),
            ("computeBudget.requestUnits", RuleClass::Unknown),
            ("stake.setLockupChecked", RuleClass::PermissionChange),
            ("vote.updateValidatorIdentity", RuleClass::PermissionChange),
            ("addressLookupTable.freezeLookupTable", RuleClass::Recognized),
            ("memo.addMemo", RuleClass::Recognized),
        ];
        for (kind, class) in classes {
            assert_eq!(table[kind].class, class, "{kind}");
        }
        for kind in [
            "token.burn",
            "stake.withdraw",
            "vote.withdraw",
            "system.withdrawNonceAccount",
        ] {
            assert_eq!(table[kind].class, RuleClass::Recognized, "{kind}");
            assert!(table[kind].alert.is_some(), "{kind}");
        }
    }

    /// 规则表写的路径都要落在官方定义上,重新生成 IDL 后字段改名时这里失败.
    #[test]
    fn every_solana_summary_path_is_in_the_idl() {
        let idl: serde_json::Value = serde_json::from_str(include_str!("../assets/solana-idl.json")).unwrap();
        for program in idl["programs"].as_array().unwrap() {
            for instruction in program["instructions"].as_array().unwrap() {
                let kind = [program["name"].as_str().unwrap(), instruction["name"].as_str().unwrap()].join(".");
                let Some(fields) = rules()["solana"][&kind].summary.as_ref() else {
                    continue;
                };
                for path in summary_paths(fields) {
                    let found = match path.split('.').collect::<Vec<_>>().as_slice() {
                        ["accounts", name] => named(&instruction["accounts"], name).is_some(),
                        ["data", "recipient_owner"] => kind.contains("transferChecked"),
                        ["data", name] => named(&instruction["arguments"], name).is_some(),
                        ["data", name, field] => named(&instruction["arguments"], name)
                            .and_then(|argument| {
                                let link = argument["type"]["name"].as_str()?;
                                named(&program["definedTypes"][link]["fields"], field)
                            })
                            .is_some(),
                        _ => false,
                    };
                    assert!(found, "{kind}: {path}");
                }
            }
        }
    }

    fn summary_paths(fields: &SummaryFields) -> Vec<&String> {
        let single = [
            &fields.amount,
            &fields.asset,
            &fields.decimals,
            &fields.party,
            &fields.owner,
            &fields.next,
            &fields.current,
        ];
        single.into_iter().flatten().chain(&fields.holders).collect()
    }

    fn named<'a>(list: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
        list.as_array()?.iter().find(|item| item["name"] == name)
    }

    #[test]
    fn every_tron_summary_path_is_in_the_schema() {
        let schema: serde_json::Value = serde_json::from_str(include_str!("../assets/tron-schema.json")).unwrap();
        let contract = |name: &str| {
            let contracts = schema["contracts"].as_object().unwrap().values();
            let found = contracts.into_iter().find(|contract| contract["name"] == name);
            found.and_then(|contract| contract["message"].as_str())
        };
        for (kind, rule) in &rules()["tron"] {
            let Some(fields) = rule.summary.as_ref() else {
                continue;
            };
            let message = contract(kind).unwrap_or_else(|| panic!("{kind}"));
            for path in summary_paths(fields) {
                let mut at = message;
                let mut names = path.split('.').peekable();
                while let Some(name) = names.next() {
                    let spec = named(&schema["messages"][at], name).unwrap_or_else(|| panic!("{kind}: {path}"));
                    // `repeated` 字段的每一项和列表同名.
                    if spec["repeated"] == true {
                        assert_eq!(names.next(), Some(name), "{kind}: {path}");
                    }
                    at = spec["of"].as_str().unwrap_or_default();
                }
            }
        }
    }

    /// 权限变更的控制方都来自规则表写明的字段,手机没有别的来源.
    #[test]
    fn every_permission_change_names_its_controllers() {
        for chain in ["solana", "tron"] {
            for (kind, rule) in &rules()[chain] {
                if rule.class == RuleClass::PermissionChange {
                    let fields = rule.summary.as_ref().unwrap_or_else(|| panic!("{kind}"));
                    assert!(fields.next.is_some() && fields.current.is_some(), "{kind}");
                }
            }
        }
    }

    /// 协议里的金额和收款方都要到第一屏,重新生成 IDL 后出现新指令时这里失败.
    #[test]
    fn every_solana_value_field_reaches_the_first_screen() {
        const RECIPIENTS: [&str; 7] = [
            "destination",
            "recipient",
            "recipientAccount",
            "newAccount",
            "delegate",
            "feeReceiver",
            "destinationStake",
        ];
        let idl: serde_json::Value = serde_json::from_str(include_str!("../assets/solana-idl.json")).unwrap();
        for program in idl["programs"].as_array().unwrap() {
            for instruction in program["instructions"].as_array().unwrap() {
                let kind = [program["name"].as_str().unwrap(), instruction["name"].as_str().unwrap()].join(".");
                let fields = rules()["solana"][&kind].summary.as_ref();
                let party = fields.and_then(|fields| fields.party.as_deref()).unwrap_or_default();
                let accounts = instruction["accounts"].as_array().unwrap();
                let recipient = accounts.iter().find_map(|account| {
                    let name = account["name"].as_str()?;
                    RECIPIENTS.contains(&name).then_some(name)
                });
                // 分配空间不转钱,`newAccount` 不是收款方.
                let allocation = ["system.allocate", "system.allocateWithSeed"].contains(&kind.as_str());
                if let Some(name) = recipient.filter(|_| !allocation) {
                    assert_eq!(party, ["accounts.", name].concat(), "{kind}");
                }
                if program["name"] == "computeBudget" {
                    continue;
                }
                for argument in instruction["arguments"].as_array().unwrap() {
                    let display = &argument["type"]["display"];
                    let optional = &argument["type"]["item"]["display"];
                    if display["unit"] == "SOL" || optional["unit"] == "SOL" {
                        let name = argument["name"].as_str().unwrap();
                        let amount = fields.and_then(|fields| fields.amount.as_deref());
                        assert_eq!(amount, Some(["data.", name].concat().as_str()), "{kind}");
                    }
                }
            }
        }
    }

    #[test]
    fn only_blind_solana_payloads_ask_for_the_second_confirmation() {
        let second: Vec<&str> = rules()["solana"]
            .iter()
            .filter(|(_, rule)| rule.second)
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(second, ["offchain-unknown", "v1-config-unknown"]);
    }

    #[test]
    fn only_compute_budget_batch_and_the_nonce_advance_are_passive() {
        let passive: Vec<&str> = rules()["solana"]
            .iter()
            .filter(|(_, rule)| rule.passive)
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(
            passive,
            [
                "computeBudget.requestHeapFrame",
                "computeBudget.setComputeUnitLimit",
                "computeBudget.setComputeUnitPrice",
                "computeBudget.setLoadedAccountsDataSizeLimit",
                "system.advanceNonceAccount",
                "token.batch",
                "token2022.batch",
            ]
        );
    }

    #[test]
    fn no_action_of_substance_means_unknown() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let rows = "\"transfer\":{\"class\":\"recognized\",\"alert\":null,\"passive\":true}";
        let assessment = assess_with(&decoded, &table(rows), &[]);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.alerts, [UNKNOWN_ALERT]);
    }

    /// 清单外的合约解码显示,类别是无法识别.
    #[test]
    fn standard_calls_on_an_unlisted_contract_are_unknown_with_the_contract_named() {
        let call = CallFact {
            shape: CallShape::Standard("transfer(address,uint256)".to_string()),
            contract: "0xabc".to_string(),
            ..CallFact::default()
        };
        assert_eq!(call_risk(Chain::Evm, &call), Risk::Unknown);
        let listed = CallFact {
            listed: true,
            ..call.clone()
        };
        assert_eq!(call_risk(Chain::Evm, &listed), Risk::Transfer);
        assert_eq!(
            call_risk(
                Chain::Evm,
                &CallFact {
                    has_value: true,
                    ..listed
                }
            ),
            Risk::Unknown
        );
        let kinds = call_kinds(Chain::Evm, &call);
        assert_eq!(kinds, [("call-unlisted", vec!["0xabc".to_string()])]);
    }

    /// 不规范的调用不管合约在不在清单都是无法识别,告警写明这一点,无限额度的告警保留.
    #[test]
    fn an_irregular_call_is_unknown_and_keeps_its_other_alerts() {
        let call = CallFact {
            shape: CallShape::Irregular("approve(address,uint256)".to_string()),
            contract: "0xabc".to_string(),
            unlimited: true,
            ..CallFact::default()
        };
        assert_eq!(call_risk(Chain::Evm, &call), Risk::Unknown);
        let kinds: Vec<&str> = call_kinds(Chain::Evm, &call)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(kinds, ["call-irregular", "call-unlisted", "call-unlimited"]);
        let listed = CallFact { listed: true, ..call };
        assert_eq!(call_risk(Chain::Tron, &listed), Risk::Unknown);
        let kinds: Vec<&str> = call_kinds(Chain::Tron, &listed)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(kinds, ["call-irregular", "call-unlimited"]);
    }

    #[test]
    fn an_approval_keeps_its_class_with_a_native_value() {
        let call = CallFact {
            shape: CallShape::Standard("approve(address,uint256)".to_string()),
            listed: true,
            has_value: true,
            ..CallFact::default()
        };
        assert_eq!(call_risk(Chain::Tron, &call), Risk::Approval);
    }

    #[test]
    fn a_descriptor_gives_recognized_never_transfer() {
        let call = CallFact {
            shape: CallShape::Described,
            listed: true,
            ..CallFact::default()
        };
        assert_eq!(call_risk(Chain::Evm, &call), Risk::Recognized);
    }

    #[test]
    fn a_call_without_facts_is_unknown() {
        let decoded = decode(
            &Tampered(|walked: &mut Walked| {
                walked.root.children[0] = walked.root.children[0].clone().action("call");
            }),
            &input(&PAYLOAD),
        )
        .unwrap();
        assert_eq!(assess_evm(&decoded, &EvmFacts::default()).risk, Risk::Unknown);
    }

    #[test]
    fn every_bitcoin_kind_has_a_rule() {
        let table = &rules()["bitcoin"];
        let scripts = crate::decode::ScriptClass::ALL.map(|class| crate::decode::output_action(class, 0));
        let burned = crate::decode::output_action(crate::decode::ScriptClass::OpReturn, 1);
        for kind in scripts.iter().chain([&burned]) {
            assert!(table.contains_key(kind.as_str()), "{kind}");
        }
        for kind in [
            INPUT_ACTION,
            "input-foreign",
            "input-path-unusual",
            "input-path-unverified",
            "input-sighash-unbound",
            "output-claim-unverified",
            "change-path-unusual",
            "change-index-far",
            "fee-unconfirmed",
            "fee-high",
            "recipients-many",
            "tx-modifiable",
            "psbt-key-unknown",
            "psbt-key-unimplemented",
            "version-1",
            "version-2",
            "version-3",
            "version-other",
            "lock-time-none",
            "lock-time-height",
            "lock-time-utc",
            "lock-time-inactive",
            "sequence-final",
            "sequence-no-rbf",
            "sequence-rbf",
        ] {
            assert!(table.contains_key(kind), "{kind}");
        }
    }

    #[test]
    fn the_rule_table_only_lowers_nothing_for_unknown_bitcoin_content() {
        let table = &rules()["bitcoin"];
        for kind in [
            "output-witness-future",
            "output-unknown",
            "input-sighash-unbound",
            "fee-unconfirmed",
            "psbt-key-unknown",
        ] {
            assert_eq!(table[kind].class, RuleClass::Unknown, "{kind}");
            assert!(table[kind].alert.is_some(), "{kind}");
        }
    }

    const TRON_SIGNALS: [&str; 27] = [
        "call-recognized",
        "call-unknown",
        "call-unlisted",
        "call-irregular",
        "call-with-value",
        "call-unlimited",
        "asset-unlisted",
        "fee-high",
        "path-unusual",
        "owner-foreign",
        "owner-unverified",
        "owner-multisig-unverified",
        "permission-shared",
        "expiration-window",
        "field-unused",
        "memo",
        "permission-id",
        "personal-sign",
        "raw-hash",
        "eip712-message",
        "eip712-permit",
        "eip712-described",
        "eip712-no-chain-id",
        "eip712-no-contract",
        "eip712-other-chain",
        "permit-many",
        "permit-unlimited",
    ];

    #[test]
    fn every_tron_kind_has_a_rule() {
        let table = &rules()["tron"];
        let functions = function_kinds("tron");
        let kinds: std::collections::BTreeSet<&str> = crate::decode::tron_actions()
            .into_iter()
            .chain(TRON_SIGNALS)
            .chain(functions.iter().map(|kind| kind.as_str()))
            .collect();
        for kind in &kinds {
            assert!(table.contains_key(*kind), "{kind}");
        }
        assert_eq!(table.len(), kinds.len(), "规则表里有用不到的种类");
    }

    /// TRON 每种合约类型的类别.
    #[test]
    fn every_contract_type_has_the_class_of_the_matrix() {
        use RuleClass::{Approval, PermissionChange, Recognized, Transfer, Unknown};
        let table = &rules()["tron"];
        let classes = [
            (Transfer, 3),
            (Recognized, 31),
            (Approval, 1),
            (PermissionChange, 1),
            (Unknown, 5),
        ];
        for (class, count) in classes {
            let matching = crate::decode::tron_contracts()
                .iter()
                .filter(|(_, name)| table[*name].class == class)
                .count();
            assert_eq!(matching, count, "{class:?}");
        }
        for name in ["TransferContract", "TransferAssetContract", TRIGGER_ACTION] {
            assert_eq!(table[name].class, Transfer, "{name}");
        }
        assert_eq!(table["DelegateResourceContract"].class, Approval);
        assert_eq!(table["AccountPermissionUpdateContract"].class, PermissionChange);
        for name in [
            "VoteAssetContract",
            "CustomContract",
            "CreateSmartContract",
            "GetContract",
            "ShieldedTransferContract",
        ] {
            assert_eq!(table[name].class, Unknown, "{name}");
            assert!(table[name].alert.is_some(), "{name}");
        }
        for name in [
            "WitnessCreateContract",
            "AssetIssueContract",
            "ParticipateAssetIssueContract",
            "ExchangeCreateContract",
            "ExchangeInjectContract",
            "ExchangeTransactionContract",
            "MarketSellAssetContract",
        ] {
            assert_eq!(table[name].class, Recognized, "{name}");
            assert!(table[name].alert.is_some(), "{name}");
        }
    }

    /// 这些内容都能签,类别和确认方式来自规则表.
    #[test]
    fn unknown_tron_content_is_unknown_with_an_alert() {
        let table = &rules()["tron"];
        for kind in [
            "contract-unknown",
            "unknown-field",
            "enum-unknown",
            "negative-integer",
            "field-unused",
            "owner-foreign",
            "owner-multisig-unverified",
            "personal-sign",
            "raw-hash",
            "eip712-message",
            "eip712-no-chain-id",
            "eip712-no-contract",
            "eip712-other-chain",
        ] {
            assert_eq!(table[kind].class, RuleClass::Unknown, "{kind}");
            assert!(table[kind].alert.is_some(), "{kind}");
        }
        for kind in ["memo", "permission-id", "expiration-window", "path-unusual"] {
            assert!(table[kind].alert.is_some(), "{kind}");
        }
        let second: Vec<&str> = table
            .iter()
            .filter(|(_, rule)| rule.second)
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(second, ["contract-unknown", "raw-hash"]);
    }

    #[test]
    fn the_validity_window_is_compared_to_the_timestamp() {
        let now = 1_800_000_000_000;
        assert!(expiration_in_window(now + 1, now));
        assert!(expiration_in_window(now + MAX_VALIDITY_MS, now));
        assert!(!expiration_in_window(now, now));
        assert!(!expiration_in_window(now - 1, now));
        assert!(!expiration_in_window(now + MAX_VALIDITY_MS + 1, now));
        assert!(!expiration_in_window(i64::MAX, i64::MIN));
    }

    #[test]
    fn a_rule_note_stays_on_its_row() {
        let assessment = assessed(
            &PAYLOAD,
            "\"transfer\":{\"class\":\"transfer\",\"alert\":null,\"note\":\"含义\"}",
        );
        assert_eq!(assessment.confirm, Confirm::Tap);
        assert!(assessment.alerts.is_empty());
        assert_eq!(assessment.findings[&2].note.as_deref(), Some("含义"));
    }

    #[test]
    fn signals_fill_their_arguments_and_repeat_only_once() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let rules = table(
            "\"transfer\":{\"class\":\"transfer\",\"alert\":null},\"s\":{\"class\":\"transfer\",\"alert\":\"第 {0} 个 {1}\"}",
        );
        let signal = Signal::new(ROOT, "s", vec!["2".to_string(), "x".to_string()]);
        let assessment = assess_with(&decoded, &rules, &[signal.clone(), signal]);
        assert_eq!(assessment.alerts, ["第 2 个 x"]);
    }
}

//! 策略:风险类别,确认方式,规则表和阈值.标准见 docs/10-tx-decoding.md 第 1.1 节和第 3 节.
//!
//! `assess` 只读树,不修改树,拿不到原始载荷.新内容默认是无法识别,可以签名,不会默认变成普通转账.
//! 依赖方向由 `tests/architecture.rs` 检查.阈值是初始值,技术方案里校准.
//! Bitcoin,EVM 的类别和告警都在规则表里,`assess_bitcoin`,`assess_evm` 只算出命中了哪些种类.

use crate::decode::{Decoded, NodeId, SourceKind, Value};
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

    /// 旧链路的排序,无法识别在授权之下.旧链路的页面按这个顺序选样式,删除旧链路时一起删除.
    pub(crate) fn legacy_rank(self) -> u8 {
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

/// 规则能给的类别.禁止只来自第 1.1 节的完整性检查,规则表写不出来.
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

/// `alert` 进入告警列表,要长按.`note` 只解释这一行.里面的 `{0}`,`{1}` 由信号的参数替换.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Rule {
    class: RuleClass,
    alert: Option<String>,
    note: Option<String>,
    /// 长按之后还要二次确认.
    #[serde(default)]
    second: bool,
}

/// 链的 ID 到动作种类名到规则.
type Rules = BTreeMap<String, BTreeMap<String, Rule>>;

static RULES: OnceLock<Rules> = OnceLock::new();

/// 规则表是内置数据,清单和描述文件都改不了.
fn rules() -> &'static Rules {
    RULES.get_or_init(|| serde_json::from_str(include_str!("../assets/rules.json")).expect("内置规则表有效"))
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
    let table = rules.get(decoded.chain().id());
    let actions = decoded.actions();
    let mut findings: BTreeMap<NodeId, Finding> = BTreeMap::new();
    let mut alerts: Vec<String> = Vec::new();
    let mut risks = Vec::new();
    let mut unknown = actions.is_empty();
    // 第 1.1 节:整段未知的载荷在长按之后还要二次确认.
    let mut second = matches!(decoded.root().value, Value::Opaque(_));
    let actions = actions
        .into_iter()
        .map(|(id, node, kind)| (id, kind, node.source.kind().is_evidence(), Vec::new()));
    let signals = signals
        .iter()
        .map(|signal| (signal.node, signal.kind, true, signal.args.clone()));
    for (id, kind, evidence, args) in actions.chain(signals) {
        let Some(rule) = table.and_then(|table| table.get(kind)).filter(|_| evidence) else {
            unknown = true;
            findings.entry(id).or_default().risk = Some(Risk::Unknown);
            continue;
        };
        let risk = Risk::from(rule.class);
        risks.push(risk);
        second |= rule.second;
        let alert = rule.alert.as_deref().map(|text| filled(text, &args));
        let note = rule.note.as_deref().map(|text| filled(text, &args)).or(alert.clone());
        let finding = findings.entry(id).or_default();
        finding.risk = finding.risk.max(Some(risk));
        finding.note = finding.note.take().or(note);
        if let Some(alert) = alert.filter(|alert| !alerts.contains(alert)) {
            alerts.push(alert);
        }
    }
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
const OUTPUT_PREFIX: &str = "output-";
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

const CALL_ACTION: &str = "call";
const FEE_CAP_ROW: &str = "fee_cap";
const NETWORK_ROW: &str = "network";
const PATH_ROW: &str = "derivation_path";
const PATH_KIND: &str = "path-unusual";

/// EVM 的事实.路径,合约调用和网络都由 `tx` 层算出,这里不碰密钥和载荷.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvmFacts {
    pub signer: Option<String>,
    /// 这条派生路径不是本机导出的账户.
    pub unusual_path: Option<String>,
    /// 合约调用的每个动作的类别.清单描述文件接入之前来自 `abi.rs`.
    pub calls: Vec<Risk>,
    pub call_alerts: Vec<String>,
    /// 调用的方法和合约,显示在 `data` 这一行.
    pub call_note: Option<String>,
    pub fee_high: bool,
    /// 请求里的网络名和 Chain ID 认出的网络.
    pub renamed: Option<(String, String)>,
    /// 谁也不认识的 Chain ID 和请求里的网络名.
    pub unknown_chain: Option<(String, String)>,
}

fn call_kind(risk: Risk) -> &'static str {
    match risk {
        Risk::Transfer => "call-transfer",
        Risk::Recognized => "call-recognized",
        Risk::Approval => "call-approval",
        Risk::PermissionChange => "call-permission-change",
        Risk::Unknown | Risk::Forbidden => "call-unknown",
    }
}

fn call_signals(node: NodeId, facts: &EvmFacts, signals: &mut Vec<Signal>) {
    // 没有事实的调用不能因为规则表里 `call` 是最低类别就过关.
    if facts.calls.is_empty() {
        signals.push(Signal::new(node, call_kind(Risk::Unknown), Vec::new()));
    }
    signals.extend(
        facts
            .calls
            .iter()
            .map(|risk| Signal::new(node, call_kind(*risk), Vec::new())),
    );
    signals.extend(
        facts
            .call_alerts
            .iter()
            .map(|alert| Signal::new(node, "call-alert", vec![alert.clone()])),
    );
}

/// 还没迁移的 EIP-712 页面也要说明路径,文案在规则表里.
pub fn unusual_path_alert(path: &str) -> String {
    let alert = rules()[Chain::Evm.id()][PATH_KIND].alert.as_deref().unwrap_or_default();
    filled(alert, &[path.to_string()])
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
        call_signals(node, facts, &mut signals);
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
    let mut assessment = assess_with(decoded, rules(), &signals);
    if let (Some(node), Some(note)) = (call, &facts.call_note) {
        assessment.findings.entry(node).or_default().note = Some(note.clone());
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
    fn the_old_path_keeps_its_order() {
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
                .all(|pair| pair[0].legacy_rank() < pair[1].legacy_rank())
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
            assert_eq!(table.is_empty(), chain != "bitcoin" && chain != "evm", "{chain}");
        }
    }

    const EVM_KINDS: [&str; 25] = [
        "no-chain-id",
        "native-transfer",
        "contract-creation",
        "call",
        "call-transfer",
        "call-recognized",
        "call-approval",
        "call-permission-change",
        "call-unknown",
        "call-alert",
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
    ];

    #[test]
    fn every_evm_kind_has_a_rule() {
        let table = &rules()["evm"];
        for kind in EVM_KINDS.into_iter().chain(crate::decode::transaction_actions()) {
            assert!(table.contains_key(kind), "{kind}");
        }
        assert_eq!(table.len(), EVM_KINDS.len() + 5, "规则表里有用不到的种类");
    }

    /// 第 1.1 节:这些内容都是无法识别,告警要说清后果.
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

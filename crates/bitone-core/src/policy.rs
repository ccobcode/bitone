//! 策略:风险类别,确认方式,规则表和阈值.
//!
//! `assess` 只读树,不修改树,拿不到原始载荷.新内容默认是无法识别,可以签名,不会默认变成普通转账.
//! 依赖方向由 `tests/architecture.rs` 检查.
//! Bitcoin,EVM,TRON,Solana 的类别和告警都在规则表里,`assess_bitcoin`,`assess_evm`,`assess_tron`,`assess_solana` 只算出命中了哪些种类.

use crate::Network;
use crate::decode::{Decoded, NodeId, SourceKind, Value, Visit, display};
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
pub(crate) enum RuleClass {
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
pub struct Rule {
    pub(crate) class: RuleClass,
    pub(crate) alert: Option<String>,
    pub(crate) note: Option<String>,
    /// 长按之后还要二次确认.
    #[serde(default)]
    pub(crate) second: bool,
    /// 不算动作,不影响类别.只有这种节点的交易和没有动作一样是无法识别.
    #[serde(default)]
    pub(crate) passive: bool,
    pub(crate) summary: Option<SummaryFields>,
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
    /// 金额是原生币的最小单位.官方类型只写了整数,单位在官方文档里,比如 Vote 的 lamports.
    #[serde(default)]
    pub native: bool,
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

/// 一条链的规则表:动作种类名到规则.每条链在自己的目录里内置一份,清单和描述文件都改不了.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct RuleTable(BTreeMap<String, Rule>);

impl std::ops::Index<&str> for RuleTable {
    type Output = Rule;

    fn index(&self, kind: &str) -> &Rule {
        self.get(kind).unwrap_or_else(|| panic!("规则表没有 {kind}"))
    }
}

impl RuleTable {
    /// 内置的规则表.解析失败是构建错误.
    pub(crate) fn builtin(cell: &'static OnceLock<RuleTable>, json: &str) -> &'static RuleTable {
        cell.get_or_init(|| serde_json::from_str(json).expect("内置规则表有效"))
    }

    pub(crate) fn get(&self, kind: &str) -> Option<&Rule> {
        self.0.get(kind)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, kind: &str) -> bool {
        self.0.contains_key(kind)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Rule)> {
        self.0.iter().map(|(kind, rule)| (kind.as_str(), rule))
    }

    /// 这种动作的页面顶部要读的字段.
    pub(crate) fn summary_fields(&self, kind: &str) -> Option<&SummaryFields> {
        self.get(kind)?.summary.as_ref()
    }

    /// 规则说这种节点不算动作.
    pub(crate) fn is_passive(&self, kind: &str) -> bool {
        self.get(kind).is_some_and(|rule| rule.passive)
    }

    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// 手续费上限,单位是网络原生币的最小单位,超过时告警.没有官方标准,是本机的阈值,和规则表一样是内置数据.
pub(crate) fn fee_ceiling(network: Network) -> u128 {
    static CEILINGS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    let table = CEILINGS
        .get_or_init(|| serde_json::from_str(include_str!("../assets/fee-ceilings.json")).expect("内置手续费上限有效"));
    let text = table.get(network.id()).expect("每个网络都有手续费上限");
    text.parse().expect("内置手续费上限有效")
}

/// 没有规则,或者内容不认识时的告警.
pub(crate) const UNKNOWN_ALERT: &str = "本机不认识这笔交易的部分内容,无法判断签名后果,签名前请确认来源和原始数据";
pub(crate) const UNKNOWN_NETWORK_ALERT: &str = "本机不认识这个网络,请确认网络名和交易内容";

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

/// 只按树的动作判定,没有链自己的事实.
pub(crate) fn assess(decoded: &Decoded, rules: &RuleTable) -> Assessment {
    assess_with(decoded, rules, &[])
}

/// 策略从树和事实里得出的一项判定.种类名在规则表里,类别和告警都来自规则表.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Signal {
    node: NodeId,
    kind: &'static str,
    args: Vec<String>,
}

impl Signal {
    pub(crate) fn new(node: NodeId, kind: &'static str, args: Vec<String>) -> Self {
        Self { node, kind, args }
    }
}

pub(crate) fn filled(text: &str, args: &[String]) -> String {
    args.iter().enumerate().fold(text.to_string(), |text, (index, arg)| {
        text.replace(&["{", &index.to_string(), "}"].concat(), arg)
    })
}

pub(crate) fn assess_with(decoded: &Decoded, rules: &RuleTable, signals: &[Signal]) -> Assessment {
    assess_quiet(decoded, rules, signals, &[])
}

/// `proven` 是 `tx` 层证明属于本钱包的节点,它们的规则告警不适用,类别不变.
pub(crate) fn assess_quiet(decoded: &Decoded, rules: &RuleTable, signals: &[Signal], proven: &[NodeId]) -> Assessment {
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
        let Some(rule) = rules.get(kind).filter(|_| evidence) else {
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

pub(crate) const CALL_ACTION: &str = "call";
pub(crate) const PATH_ROW: &str = "derivation_path";
pub(crate) const PATH_KIND: &str = "path-unusual";
pub(crate) const ROOT: NodeId = 0;

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

/// 一条 Permit 消息里的授权.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermitFact {
    pub grants: usize,
    pub unlimited: usize,
}

impl PermitFact {
    pub(crate) fn signals(&self, signals: &mut Vec<Signal>) {
        if self.grants > 1 {
            signals.push(Signal::new(ROOT, "permit-many", vec![self.grants.to_string()]));
            if self.unlimited > 0 {
                signals.push(Signal::new(ROOT, "permit-unlimited", vec![self.unlimited.to_string()]));
            }
        }
    }
}

pub(crate) const FUNCTION_PREFIX: &str = "fn:";

/// 标准函数在规则表里的种类.规则表是 `'static` 的,所以种类名不用复制.
fn function_kind(rules: &'static RuleTable, signature: &str) -> Option<&'static str> {
    let key = [FUNCTION_PREFIX, signature].concat();
    rules.0.get_key_value(&key).map(|(kind, _)| kind.as_str())
}

/// 附带原生币的调用不适用转账和已知函数的展示,授权类调用的类别更高,不变.
fn plain_class(rules: &RuleTable, kind: &str) -> bool {
    rules
        .get(kind)
        .is_some_and(|rule| matches!(rule.class, RuleClass::Transfer | RuleClass::Recognized))
}

pub(crate) fn call_kinds(rules: &'static RuleTable, call: &CallFact) -> Vec<(&'static str, Vec<String>)> {
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
        CallShape::Standard(signature) => match function_kind(rules, signature) {
            Some(kind) if call.has_value && plain_class(rules, kind) => vec![("call-with-value", Vec::new())],
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
pub fn call_risk(rules: &'static RuleTable, call: &CallFact) -> Risk {
    let classes = call_kinds(rules, call)
        .into_iter()
        .map(|(kind, _)| rules.get(kind).map_or(Risk::Unknown, |rule| Risk::from(rule.class)));
    classes.max().unwrap_or(Risk::Unknown)
}

pub(crate) fn call_signals(rules: &'static RuleTable, node: NodeId, calls: &[CallFact], signals: &mut Vec<Signal>) {
    // 没有事实的调用不能因为规则表里 `call` 是最低类别就过关.
    if calls.is_empty() {
        signals.push(Signal::new(node, "call-unknown", Vec::new()));
    }
    let kinds = calls.iter().flat_map(|call| call_kinds(rules, call));
    signals.extend(kinds.map(|(kind, args)| Signal::new(node, kind, args)));
}

pub(crate) fn mark_own(assessment: &mut Assessment, visits: &[Visit<'_>], signer: Option<&String>) {
    for visit in visits {
        let own = matches!(&visit.node.value, Value::Address(address) if Some(address) == signer);
        if own {
            assessment.findings.entry(visit.id).or_default().own = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::fixture::{PARTIAL, PAYLOAD, Records, Tampered, UNKNOWN_FORMAT, input};
    use crate::decode::{Node, Source, Walked, decode};

    fn table(rows: &str) -> RuleTable {
        serde_json::from_str(&format!("{{{rows}}}")).unwrap()
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
        assert_eq!(assess_with(&decoded, &RuleTable::default(), &[]).risk, Risk::Unknown);
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
        let result: Result<RuleTable, _> = serde_json::from_str("{\"x\":{\"class\":\"forbidden\",\"alert\":null}}");
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
    fn every_network_has_a_fee_ceiling() {
        for network in Network::ALL {
            assert!(fee_ceiling(network) > 0, "{network:?}");
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

    #[test]
    fn no_action_of_substance_means_unknown() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let rows = "\"transfer\":{\"class\":\"recognized\",\"alert\":null,\"passive\":true}";
        let assessment = assess_with(&decoded, &table(rows), &[]);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.alerts, [UNKNOWN_ALERT]);
    }
}

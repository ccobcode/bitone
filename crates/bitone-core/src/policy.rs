//! 策略:风险类别,确认方式,规则表和阈值.标准见 docs/10-tx-decoding.md 第 1.1 节和第 3 节.
//!
//! `assess` 只读树,不修改树,拿不到原始载荷.新内容默认是无法识别,可以签名,不会默认变成普通转账.
//! 依赖方向由 `tests/architecture.rs` 检查.阈值是初始值,技术方案里校准.

use crate::decode::{Decoded, NodeId, SourceKind, Value};
use crate::{Mode, Network};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confirm {
    /// 点按,再输 PIN.
    Tap,
    /// 长按 3 秒,再输 PIN.
    Hold,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Rule {
    class: RuleClass,
    alert: Option<String>,
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
    assess_with(decoded, rules())
}

fn assess_with(decoded: &Decoded, rules: &Rules) -> Assessment {
    let table = rules.get(decoded.chain().id());
    let actions = decoded.actions();
    let mut findings = BTreeMap::new();
    let mut alerts = Vec::new();
    let mut risks = Vec::new();
    let mut unknown = actions.is_empty();
    for (id, node, kind) in actions {
        let rule = table
            .and_then(|table| table.get(kind))
            .filter(|_| node.source.kind().is_evidence());
        let Some(rule) = rule else {
            unknown = true;
            findings.insert(id, unknown_finding());
            continue;
        };
        let risk = Risk::from(rule.class);
        risks.push(risk);
        alerts.extend(rule.alert.clone());
        findings.insert(
            id,
            Finding {
                risk: Some(risk),
                note: rule.alert.clone(),
                ..Finding::default()
            },
        );
    }
    // 进入签名原文的 `opaque` 字节含义不明,动作的规则不能替它担保.
    for visit in decoded.visits() {
        let opaque = matches!(&visit.node.value, Value::Opaque(bytes) if !bytes.is_empty());
        if opaque && visit.node.source.kind() == SourceKind::Signed {
            unknown = true;
            findings.insert(visit.id, unknown_finding());
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
    Assessment {
        risk,
        confirm: Confirm::of(risk, !alerts.is_empty()),
        alerts,
        findings,
    }
}

fn unknown_finding() -> Finding {
    Finding {
        risk: Some(Risk::Unknown),
        ..Finding::default()
    }
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
        assess_with(&decode(&Records, &input(payload)).unwrap(), &table(rows))
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
        let assessment = assess_with(&decoded, &table(TRANSFER_RULE));
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.alerts, [UNKNOWN_NETWORK_ALERT]);
    }

    #[test]
    fn a_chain_without_a_table_is_unknown() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        assert_eq!(assess_with(&decoded, &Rules::new()).risk, Risk::Unknown);
    }

    #[test]
    fn no_actions_is_unknown_never_plain() {
        let assessment = assessed(&[1, 2, b'o', b'k'], TRANSFER_RULE);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
    }

    #[test]
    fn an_unknown_payload_is_signable_with_the_strongest_alert() {
        let assessment = assessed(&[UNKNOWN_FORMAT, 1], TRANSFER_RULE);
        assert_eq!((assessment.risk, assessment.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(assessment.findings[&0].risk, Some(Risk::Unknown));
        assert_eq!(assessment.alerts, [UNKNOWN_ALERT]);
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
        assert_eq!(assess_with(&decoded, &table(TRANSFER_RULE)).risk, Risk::Unknown);
    }

    #[test]
    fn the_builtin_table_parses() {
        assert!(rules().contains_key("evm"));
    }

    #[test]
    fn nothing_is_migrated_yet_so_the_builtin_table_is_empty() {
        assert!(rules().values().all(BTreeMap::is_empty));
    }
}

//! TRON 的策略.类别和告警都在 `assets/tron/rules.json`,合约调用的判定和 EVM 共用 `policy`.
//! 这里只读树和 `tx` 层给的事实,不碰密钥和载荷.

use crate::decode::{Decoded, NodeId, Value, Visit};
use crate::policy::{
    Assessment, CallFact, PATH_KIND, PATH_ROW, PermitFact, ROOT, RuleTable, Signal, assess_with, call_signals, mark_own,
};
use std::sync::OnceLock;

/// 内置的规则表,清单和描述文件都改不了.
pub(crate) fn rules() -> &'static RuleTable {
    static RULES: OnceLock<RuleTable> = OnceLock::new();
    RuleTable::builtin(&RULES, include_str!("../../../assets/tron/rules.json"))
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
        call_signals(rules(), node, &facts.calls, &mut signals);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{FUNCTION_PREFIX, RuleClass, SummaryFields};

    fn function_kinds() -> Vec<&'static str> {
        rules()
            .kinds()
            .filter(|kind| kind.starts_with(FUNCTION_PREFIX))
            .collect()
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
        let schema: serde_json::Value = serde_json::from_str(include_str!("../../../assets/tron/schema.json")).unwrap();
        let contract = |name: &str| {
            let contracts = schema["contracts"].as_object().unwrap().values();
            let found = contracts.into_iter().find(|contract| contract["name"] == name);
            found.and_then(|contract| contract["message"].as_str())
        };
        for (kind, rule) in rules().iter() {
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
        for (kind, rule) in rules().iter() {
            if rule.class == RuleClass::PermissionChange {
                let fields = rule.summary.as_ref().unwrap_or_else(|| panic!("{kind}"));
                assert!(fields.next.is_some() && fields.current.is_some(), "{kind}");
            }
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
        let table = rules();
        let functions = function_kinds();
        let kinds: std::collections::BTreeSet<&str> = crate::chains::tron::decode::tron_actions()
            .into_iter()
            .chain(TRON_SIGNALS)
            .chain(functions)
            .collect();
        for kind in &kinds {
            assert!(table.contains(kind), "{kind}");
        }
        assert_eq!(table.len(), kinds.len(), "规则表里有用不到的种类");
    }

    /// 每种合约类型的类别都写在这里,规则表改了类别时这里失败.
    #[test]
    fn every_contract_type_has_the_class_of_the_matrix() {
        use RuleClass::{Approval, PermissionChange, Recognized, Transfer, Unknown};
        let table = rules();
        let classes = [
            (Transfer, 3),
            (Recognized, 31),
            (Approval, 1),
            (PermissionChange, 1),
            (Unknown, 5),
        ];
        for (class, count) in classes {
            let matching = crate::chains::tron::decode::tron_contracts()
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
        let table = rules();
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
            .map(|(kind, _)| kind)
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
}

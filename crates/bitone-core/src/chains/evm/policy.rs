//! EVM 的策略.类别和告警都在 `assets/evm/rules.json`,合约调用的判定和 TRON 共用 `policy`.
//! 这里只读树和 `tx` 层给的事实,不碰密钥和载荷.

use crate::decode::Decoded;
use crate::policy::{
    Assessment, CALL_ACTION, CallFact, PATH_KIND, PATH_ROW, PermitFact, ROOT, RuleTable, Signal, assess_with,
    call_signals, mark_own,
};
use std::sync::OnceLock;

/// 内置的规则表,清单和描述文件都改不了.
pub(crate) fn rules() -> &'static RuleTable {
    static RULES: OnceLock<RuleTable> = OnceLock::new();
    RuleTable::builtin(&RULES, include_str!("../../../assets/evm/rules.json"))
}

const FEE_CAP_ROW: &str = "fee_cap";
const NETWORK_ROW: &str = "network";

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
        call_signals(rules(), node, &facts.calls, &mut signals);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::fixture::{PAYLOAD, Tampered, input};
    use crate::decode::{Walked, decode};
    use crate::policy::{CallShape, FUNCTION_PREFIX, Risk, RuleClass, call_kinds, call_risk};

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
        let table = rules();
        for kind in EVM_KINDS
            .into_iter()
            .chain(crate::chains::evm::decode::transaction_actions())
        {
            assert!(table.contains(kind), "{kind}");
        }
        assert_eq!(
            table.len(),
            EVM_KINDS.len() + 5 + function_kinds().len(),
            "规则表里有用不到的种类"
        );
    }

    /// 策略不依赖描述文件,函数名在这里和规则表自己比较,`tx/abi.rs` 的测试比较内置函数.
    fn function_kinds() -> Vec<&'static str> {
        rules()
            .kinds()
            .filter(|kind| kind.starts_with(FUNCTION_PREFIX))
            .collect()
    }

    /// 这些内容可以签,但都是无法识别,告警要说清后果.
    #[test]
    fn dangerous_evm_content_is_unknown_with_an_alert() {
        let table = rules();
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
        let second: Vec<&str> = rules()
            .iter()
            .filter(|(_, rule)| rule.second)
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(second, ["eip191-unknown", "eip712-digest", "raw-hash"]);
    }

    /// 清单外的合约也解码显示,类别是无法识别.
    #[test]
    fn standard_calls_on_an_unlisted_contract_are_unknown_with_the_contract_named() {
        let call = CallFact {
            shape: CallShape::Standard("transfer(address,uint256)".to_string()),
            contract: "0xabc".to_string(),
            ..CallFact::default()
        };
        assert_eq!(call_risk(rules(), &call), Risk::Unknown);
        let listed = CallFact {
            listed: true,
            ..call.clone()
        };
        assert_eq!(call_risk(rules(), &listed), Risk::Transfer);
        assert_eq!(
            call_risk(
                rules(),
                &CallFact {
                    has_value: true,
                    ..listed
                }
            ),
            Risk::Unknown
        );
        let kinds = call_kinds(rules(), &call);
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
        assert_eq!(call_risk(rules(), &call), Risk::Unknown);
        let kinds: Vec<&str> = call_kinds(rules(), &call).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, ["call-irregular", "call-unlisted", "call-unlimited"]);
        let listed = CallFact { listed: true, ..call };
        assert_eq!(call_risk(rules(), &listed), Risk::Unknown);
        let kinds: Vec<&str> = call_kinds(rules(), &listed).into_iter().map(|(kind, _)| kind).collect();
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
        assert_eq!(call_risk(rules(), &call), Risk::Approval);
    }

    #[test]
    fn a_descriptor_gives_recognized_never_transfer() {
        let call = CallFact {
            shape: CallShape::Described,
            listed: true,
            ..CallFact::default()
        };
        assert_eq!(call_risk(rules(), &call), Risk::Recognized);
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
}

//! Solana 的策略.类别和告警都在 `assets/solana/rules.json`.
//! 这里只读树和 `tx` 层给的事实,不碰密钥和载荷.

use crate::decode::{Decoded, NodeId, Value};
use crate::policy::{Assessment, PATH_KIND, PATH_ROW, ROOT, RuleTable, Signal, assess_quiet};
use std::sync::OnceLock;

/// 内置的规则表,清单和描述文件都改不了.
pub(crate) fn rules() -> &'static RuleTable {
    static RULES: OnceLock<RuleTable> = OnceLock::new();
    RuleTable::builtin(&RULES, include_str!("../../../assets/solana/rules.json"))
}

const SOLANA_FEE_ROW: &str = "priority_fee";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{RuleClass, SummaryFields};

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

    /// 官方枚举的每个种类都有规则,规则表里也没有用不到的种类.重新生成 IDL 后出现新指令时这里失败.
    #[test]
    fn every_solana_kind_has_a_rule() {
        let table = rules();
        let actions = crate::chains::solana::decode::instruction_actions();
        for kind in actions.iter().map(String::as_str).chain(SOLANA_KINDS) {
            assert!(table.contains(kind), "{kind}");
        }
        let unique: std::collections::BTreeSet<&str> = SOLANA_KINDS.into_iter().collect();
        assert_eq!(table.len(), actions.len() + unique.len(), "规则表里有用不到的种类");
    }

    #[test]
    fn solana_matches_the_standard_matrix() {
        let table = rules();
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
            ("vote.UpdateValidatorIdentity", RuleClass::PermissionChange),
            ("loaderV3.setAuthority", RuleClass::PermissionChange),
            ("loader_v4.TransferAuthority", RuleClass::PermissionChange),
            ("addressLookupTable.freezeLookupTable", RuleClass::Recognized),
            ("memo.addMemo", RuleClass::Recognized),
        ];
        for (kind, class) in classes {
            assert_eq!(table[kind].class, class, "{kind}");
        }
        for kind in [
            "token.burn",
            "stake.withdraw",
            "vote.Withdraw",
            "vote.TowerSync",
            "loaderV3.upgrade",
            "system.withdrawNonceAccount",
        ] {
            assert_eq!(table[kind].class, RuleClass::Recognized, "{kind}");
            assert!(table[kind].alert.is_some(), "{kind}");
        }
    }

    /// 规则表写的路径都要落在官方定义上,重新生成 IDL 后字段改名时这里失败.
    #[test]
    fn every_solana_summary_path_is_in_the_idl() {
        let idl: serde_json::Value = serde_json::from_str(include_str!("../../../assets/solana/idl.json")).unwrap();
        for program in idl["programs"].as_array().unwrap() {
            for instruction in program["instructions"].as_array().unwrap() {
                let kind = [program["name"].as_str().unwrap(), instruction["name"].as_str().unwrap()].join(".");
                let Some(fields) = rules()[&kind].summary.as_ref() else {
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
        let idl: serde_json::Value = serde_json::from_str(include_str!("../../../assets/solana/idl.json")).unwrap();
        for program in idl["programs"].as_array().unwrap() {
            for instruction in program["instructions"].as_array().unwrap() {
                let kind = [program["name"].as_str().unwrap(), instruction["name"].as_str().unwrap()].join(".");
                let fields = rules()[&kind].summary.as_ref();
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
        let second: Vec<&str> = rules()
            .iter()
            .filter(|(_, rule)| rule.second)
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(second, ["offchain-unknown", "v1-config-unknown"]);
    }

    #[test]
    fn only_compute_budget_batch_and_the_nonce_advance_are_passive() {
        let passive: Vec<&str> = rules()
            .iter()
            .filter(|(_, rule)| rule.passive)
            .map(|(kind, _)| kind)
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
}

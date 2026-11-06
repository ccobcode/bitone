//! Bitcoin 的策略.输入输出的顺序就是树里动作的顺序,类别和告警都在 `assets/bitcoin/rules.json`.
//! 这里只读树和 `tx` 层给的事实,不碰密钥和载荷.

use crate::decode::{Decoded, NodeId};
use crate::policy::{Assessment, ROOT, RuleTable, Signal, assess_with};
use std::sync::OnceLock;

/// 内置的规则表,清单和描述文件都改不了.
pub(crate) fn rules() -> &'static RuleTable {
    static RULES: OnceLock<RuleTable> = OnceLock::new();
    RuleTable::builtin(&RULES, include_str!("../../../assets/bitcoin/rules.json"))
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
        percent || u128::from(self.fee) > crate::policy::fee_ceiling(crate::Network::Bitcoin)
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

/// 找零索引超过已签过的最高索引加这个数时按高风险处理.
pub const CHANGE_GAP: u32 = 20;

/// Bitcoin 手续费超过发送额的这个百分比时按高风险处理.
pub const BITCOIN_FEE_PERCENT: u64 = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::RuleClass;

    #[test]
    fn every_bitcoin_kind_has_a_rule() {
        let table = rules();
        let scripts =
            crate::chains::bitcoin::ScriptClass::ALL.map(|class| crate::chains::bitcoin::output_action(class, 0));
        let burned = crate::chains::bitcoin::output_action(crate::chains::bitcoin::ScriptClass::OpReturn, 1);
        for kind in scripts.iter().chain([&burned]) {
            assert!(table.contains(kind.as_str()), "{kind}");
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
            assert!(table.contains(kind), "{kind}");
        }
    }

    #[test]
    fn the_rule_table_only_lowers_nothing_for_unknown_bitcoin_content() {
        let table = rules();
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
}

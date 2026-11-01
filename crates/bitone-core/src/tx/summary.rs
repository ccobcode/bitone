//! 页面顶部.每种动作的金额,对方和控制方在哪个字段都写在规则表里,这里只按路径读树,
//! 不按指令或合约类型写代码.

use crate::Chain;
use crate::decode::solana::is_absent;
use crate::decode::{Amount, Node, NodeId, Value, display};
use crate::manifest::Asset;
use crate::policy::{self, SummaryFields};
use crate::review::{AmountView, Controller, Party, Risk, Summary};
use crate::units::{decimal_from_be, format_units};

const NEW_CONTROLLER: &str = "新控制方";
/// 代币收款方的所有者是网页端声明,推导不出目标账户时不采信.
const OWNER_UNCONFIRMED: &str = "无法确认所有者";

/// 一个动作在页面顶部的内容.`node` 是动作的节点,类别和告警来自策略.
#[derive(Debug, Default)]
pub(crate) struct Effect {
    pub node: NodeId,
    pub amount: Option<AmountView>,
    pub counterparty: Option<(String, String)>,
    /// 权限变更页第一屏的对方.
    pub parties: Vec<Party>,
    pub control: Option<Controller>,
    /// 代币收款方的所有者没有确认.
    pub owner_unconfirmed: bool,
    /// 新权限里有新控制方之外的地址.
    pub shared: bool,
}

impl Effect {
    /// 这个动作有可以放在顶部的内容.
    pub fn is_named(&self) -> bool {
        self.amount.is_some() || self.counterparty.is_some() || self.control.is_some() || !self.parties.is_empty()
    }
}

/// 路径的每一段是子节点的名字.`repeated` 字段的每一项都算.
fn find<'a>(scope: &'a Node, path: &str) -> Vec<&'a Node> {
    path.split('.').fold(vec![scope], |nodes, name| {
        let children = nodes.into_iter().flat_map(|node| &node.children);
        children.filter(|child| child.name == name).collect()
    })
}

fn first<'a>(scope: &'a Node, path: Option<&String>) -> Option<&'a Node> {
    find(scope, path?).first().copied()
}

/// 节点的显示文字.option 写明没有时不是值.
fn text_of(node: &Node) -> Option<String> {
    (!is_absent(&node.value)).then(|| display(&node.value))
}

pub(crate) fn number_of(value: &Value) -> Option<u64> {
    let raw = match value {
        Value::Uint(raw) => raw,
        Value::Amount(amount) => &amount.raw,
        _ => return None,
    };
    let mut wide = [0u8; 8];
    let start = wide.len().checked_sub(raw.len())?;
    wide[start..].copy_from_slice(raw);
    Some(u64::from_be_bytes(wide))
}

/// 原始整数.不是数字时没有金额,比如 option 写明没有.
fn integer_text(value: &Value) -> Option<String> {
    match value {
        Value::Uint(raw) => Some(decimal_from_be(raw)),
        Value::Amount(amount) => Some(decimal_from_be(&amount.raw)),
        Value::Int(number) => Some(number.to_string()),
        _ => None,
    }
}

/// 字段有小数位和单位就是原生币,本机不认识的网络没有这两项.
fn is_coin(value: &Value) -> bool {
    matches!(
        value,
        Value::Amount(Amount {
            decimals: Some(_),
            unit: Some(_),
            ..
        })
    )
}

fn amount_view<'a>(
    fields: &SummaryFields,
    scope: &Node,
    native: Option<&'a Asset>,
    listed: &impl Fn(&str, Option<u64>) -> Option<&'a Asset>,
) -> Option<AmountView> {
    let node = first(scope, fields.amount.as_ref())?;
    let raw = integer_text(&node.value)?;
    let decimals = first(scope, fields.decimals.as_ref()).and_then(|row| number_of(&row.value));
    let token = first(scope, fields.asset.as_ref()).and_then(|row| listed(&display(&row.value), decimals));
    let asset = token.or(native.filter(|_| is_coin(&node.value)));
    Some(AmountView {
        text: asset.map_or_else(|| raw.clone(), |asset| format_units(&raw, asset.decimals)),
        symbol: asset.map(|asset| asset.symbol.clone()),
        asset: asset.map(Asset::key),
        unlimited: fields.unlimited && number_of(&node.value) == Some(u64::MAX),
    })
}

/// 收款方.代币账户的所有者确认过时显示所有者,否则显示代币账户,并说明所有者没有确认.
fn read_party(fields: &SummaryFields, scope: &Node, effect: &mut Effect) {
    let (Some(path), Some(label)) = (&fields.party, &fields.label) else {
        return;
    };
    let nodes = find(scope, path);
    let values: Vec<String> = nodes.iter().filter_map(|node| text_of(node)).collect();
    let party = |value: &String| Party {
        label: label.clone(),
        value: value.clone(),
    };
    // `repeated` 字段有几个对方,都在列表里,不能只把第一个放在顶部.
    if let [only] = values.as_slice() {
        let owner = first(scope, fields.owner.as_ref()).and_then(text_of);
        effect.owner_unconfirmed = fields.owner.is_some() && owner.is_none();
        let confirmed = owner
            .clone()
            .or_else(|| fields.owner.as_ref().map(|_| OWNER_UNCONFIRMED.to_string()));
        effect.parties.push(party(confirmed.as_ref().unwrap_or(only)));
        effect.counterparty = Some((label.clone(), owner.unwrap_or_else(|| only.clone())));
    } else {
        effect.parties.extend(values.iter().map(party));
    }
}

/// 新控制方.字段写明没有时用规则的说明,规则没有说明就是这笔变更没有新控制方.
fn read_control(fields: &SummaryFields, scope: &Node, effect: &mut Effect) {
    let Some(path) = &fields.next else {
        return;
    };
    let node = first(scope, Some(path));
    let next = node.and_then(|node| text_of(node).or_else(|| fields.none.clone()));
    effect.shared = next.as_ref().is_some_and(|next| {
        let holders = fields.holders.iter().flat_map(|path| find(scope, path));
        holders.into_iter().any(|holder| display(&holder.value) != *next)
    });
    effect.counterparty = next.clone().map(|next| (NEW_CONTROLLER.to_string(), next));
    effect.control = Some(Controller {
        current: first(scope, fields.current.as_ref()).and_then(text_of),
        next,
    });
}

/// 按规则表读一个动作.`scope` 是路径的起点.`listed` 按链的规则在清单里找金额的资产.
pub(crate) fn effect<'a>(
    chain: Chain,
    kind: &str,
    node: NodeId,
    scope: &Node,
    native: Option<&'a Asset>,
    listed: impl Fn(&str, Option<u64>) -> Option<&'a Asset>,
) -> Effect {
    let mut effect = Effect {
        node,
        ..Effect::default()
    };
    if let Some(fields) = policy::summary_fields(chain, kind) {
        effect.amount = amount_view(fields, scope, native, &listed);
        read_party(fields, scope, &mut effect);
        read_control(fields, scope, &mut effect);
    }
    effect
}

/// 页面顶部只显示一个动作,用户看的就是它,所以它必须是唯一类别最高的动作.
/// 最高的有多个,或者没有内容可放的动作不低于它时,顶部留空,不能让小额动作盖住真正的动作.
/// `rank` 是 `Risk::headline_rank`.
pub(crate) fn headline(effects: Vec<(Effect, u8)>) -> Summary {
    let unknown = Risk::Unknown.headline_rank();
    let named = || {
        effects
            .iter()
            .filter(|(effect, _)| effect.is_named())
            .map(|(_, rank)| *rank)
    };
    let top = named().max();
    let tied = named().filter(|rank| Some(*rank) == top).count();
    let unnamed = effects
        .iter()
        .filter(|(effect, rank)| !effect.is_named() && *rank >= unknown)
        .map(|(_, rank)| *rank)
        .max();
    let shown = tied == 1 && unnamed.is_none_or(|rank| Some(rank) < top);

    let mut summary = Summary::default();
    for (effect, rank) in effects {
        let lead = shown && effect.is_named() && Some(rank) == top;
        summary.parties.extend(effect.parties);
        summary.controllers.extend(effect.control);
        if lead {
            summary.amount = effect.amount;
            (summary.counterparty_label, summary.counterparty) = effect.counterparty.unzip();
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{Extent, Source};

    fn signed() -> Source {
        Source::Signed(Extent::Inherited)
    }

    fn address(name: &str, value: &str) -> Node {
        Node::new(name, Value::Address(value.to_string()), signed())
    }

    fn group(name: &str, children: Vec<Node>) -> Node {
        Node::new(name, Value::Struct, signed()).children(children)
    }

    #[test]
    fn a_path_walks_names_and_every_item_of_a_list() {
        let scope = group(
            "root",
            vec![group(
                "keys",
                vec![
                    group("keys", vec![address("address", "a")]),
                    group("keys", vec![address("address", "b")]),
                ],
            )],
        );
        let found: Vec<String> = find(&scope, "keys.keys.address")
            .iter()
            .map(|node| display(&node.value))
            .collect();
        assert_eq!(found, ["a", "b"]);
        assert!(find(&scope, "keys.other").is_empty());
    }

    fn named(rank: Risk, party: &str) -> (Effect, u8) {
        let effect = Effect {
            counterparty: Some(("收款地址".to_string(), party.to_string())),
            ..Effect::default()
        };
        (effect, rank.headline_rank())
    }

    fn unnamed(rank: Risk) -> (Effect, u8) {
        (Effect::default(), rank.headline_rank())
    }

    #[test]
    fn the_highest_action_takes_the_headline() {
        let summary = headline(vec![named(Risk::Transfer, "a"), named(Risk::Approval, "b")]);
        assert_eq!(summary.counterparty.as_deref(), Some("b"));
    }

    #[test]
    fn a_tie_leaves_the_headline_empty() {
        let summary = headline(vec![named(Risk::Approval, "a"), named(Risk::Approval, "b")]);
        assert_eq!(summary.counterparty, None);
    }

    #[test]
    fn an_action_with_nothing_to_show_cannot_be_covered_by_a_lower_one() {
        let summary = headline(vec![named(Risk::Transfer, "a"), unnamed(Risk::Unknown)]);
        assert_eq!(summary.counterparty, None);
        let summary = headline(vec![named(Risk::Approval, "a"), unnamed(Risk::Unknown)]);
        assert_eq!(summary.counterparty.as_deref(), Some("a"));
        let summary = headline(vec![named(Risk::Transfer, "a"), unnamed(Risk::PermissionChange)]);
        assert_eq!(summary.counterparty, None);
    }

    #[test]
    fn a_harmless_action_does_not_empty_the_headline() {
        let summary = headline(vec![named(Risk::Transfer, "a"), unnamed(Risk::Recognized)]);
        assert_eq!(summary.counterparty.as_deref(), Some("a"));
    }
}

//! 核对结果.解析和分类都在核心库里完成,界面只负责画出来,所以 Android 和 iOS 的判断不会不一致.

use crate::decode::{self, Decoded, Failure, Kind, NodeId, SourceKind};
use crate::policy::{Assessment, Finding};
use crate::{Chain, Mode, Network};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use thiserror::Error;

pub use crate::policy::{Confirm, Risk};

/// 禁止签名的原因.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Forbid {
    #[error("这个请求不属于本钱包")]
    FingerprintMismatch,
    #[error("网络不支持")]
    UnknownNetwork,
    #[error("派生路径不在白名单")]
    PathNotAllowed,
    #[error("派生路径与网络不符")]
    PathNetworkMismatch,
    #[error("地址与派生路径不符")]
    AddressMismatch,
    #[error("交易数据无法解析")]
    ParseFailed,
    #[error("Chain ID 与网络不符")]
    ChainIdMismatch,
    #[error("不支持裸哈希签名")]
    RawHashSigning,
    #[error("不支持消息签名")]
    MessageSigning,
    #[error("缺少必需的前序交易,无法确认输入金额")]
    MissingPrevTx,
    #[error("前序交易与输入不符")]
    PrevTxMismatch,
    #[error("这类交易暂不支持")]
    Unsupported,
}

/// 金额.`symbol` 为空表示不在内置清单,`text` 是原始整数.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmountView {
    pub text: String,
    pub symbol: Option<String>,
    /// 内置清单的条目,`网络:合约`,用来取图标.
    pub asset: Option<String>,
    pub unlimited: bool,
}

/// 权限变更页的一组控制方.`current` 为空时当前控制方就是付款账户,`next` 为空时这笔权限变更没有新控制方.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub current: Option<String>,
    pub next: Option<String>,
}

/// 权限变更打包的转账或调用的对方.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Party {
    pub label: String,
    pub value: String,
}

/// 页面顶部,在字段列表上方,不能代替列表.每个值都引用 `field_ids` 里的行.
/// `controllers`,`parties`,`payer` 给权限变更页,通用列表不按字段名分支,表达不了"谁是当前控制方".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub title: String,
    pub amount: Option<AmountView>,
    /// 收款地址,被授权方,新控制方等.
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    pub field_ids: Vec<NodeId>,
    pub controllers: Vec<Controller>,
    pub parties: Vec<Party>,
    pub payer: Option<String>,
}

impl Summary {
    /// 顶部显示的文字.金额文字和行的值相同时才算找到,算出来的金额由 `field_ids` 预先给出行.
    fn values(&self) -> Vec<String> {
        let amount = self.amount.iter().map(|amount| match &amount.symbol {
            Some(symbol) => [amount.text.as_str(), symbol].join(" "),
            None => amount.text.clone(),
        });
        let controllers = self
            .controllers
            .iter()
            .flat_map(|controller| controller.current.iter().chain(&controller.next).cloned());
        let parties = self.parties.iter().map(|party| party.value.clone());
        amount
            .chain(self.counterparty.iter().cloned())
            .chain(controllers)
            .chain(parties)
            .chain(self.payer.iter().cloned())
            .collect()
    }

    fn linked(mut self, fields: &[FieldRow]) -> Self {
        let values = self.values();
        let found = values
            .iter()
            .filter_map(|value| fields.iter().find(|row| row.value == *value));
        self.field_ids.extend(found.map(|row| row.id));
        self.field_ids.sort_unstable();
        self.field_ids.dedup();
        self
    }
}

/// 字段列表的一行.`value` 已经按种类格式化,手机不做格式化.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldRow {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: u32,
    pub name: String,
    pub label: String,
    pub kind: Kind,
    pub value: String,
    pub source: SourceKind,
    pub offset: Option<u32>,
    pub len: Option<u32>,
    pub risk: Option<Risk>,
    pub note: Option<String>,
    pub own: bool,
}

static LABELS: OnceLock<BTreeMap<String, String>> = OnceLock::new();

fn labels() -> &'static BTreeMap<String, String> {
    LABELS.get_or_init(|| serde_json::from_str(include_str!("../assets/field-labels.json")).expect("内置字段名有效"))
}

/// 字段名的显示文字.同一个名字在不同链上意思不同时,数据文件的键带链名.
/// 数据文件里没有时显示字段名,EIP-712 的字段名来自载荷,所以要转义.
pub(crate) fn field_label(chain: Chain, name: &str) -> String {
    let labels = labels();
    labels
        .get(&[chain.id(), name].join("."))
        .or_else(|| labels.get(name))
        .cloned()
        .unwrap_or_else(|| decode::escape_text(name))
}

fn field_rows(decoded: &Decoded, findings: &BTreeMap<NodeId, Finding>) -> Vec<FieldRow> {
    let none = Finding::default();
    decoded
        .visits()
        .into_iter()
        .map(|visit| {
            let finding = findings.get(&visit.id).unwrap_or(&none);
            let node = visit.node;
            FieldRow {
                id: visit.id,
                parent: visit.parent,
                depth: visit.depth,
                name: node.name.clone(),
                label: field_label(decoded.chain(), &node.name),
                kind: node.value.kind(),
                value: finding.display.clone().unwrap_or_else(|| decode::display(&node.value)),
                source: node.source.kind(),
                offset: visit.span.map(|span| span.offset),
                len: visit.span.map(|span| span.len),
                risk: finding.risk,
                note: finding.note.clone(),
                own: finding.own,
            }
        })
        .collect()
}

/// 全部行,风险和确认方式的 SHA-256.
fn review_digest(fields: &[FieldRow], risk: Risk, confirm: Confirm) -> [u8; 32] {
    let encoded = serde_json::to_vec(&(fields, risk, confirm)).expect("写入内存不会失败");
    Sha256::digest(encoded).into()
}

/// 不能确认是测试网时都显示主网提示.
fn is_mainnet(network: &str) -> bool {
    Network::from_id(network).is_none_or(|network| network.mode() == Mode::Mainnet)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub risk: Risk,
    pub confirm: Confirm,
    pub network: String,
    /// 页面顶部显示"主网,真实资产".
    pub mainnet: bool,
    pub summary: Summary,
    /// 高风险原因.不为空时要长按.
    pub alerts: Vec<String>,
    /// 解码树的全部节点.只有拒绝时才是空的.
    pub fields: Vec<FieldRow>,
    pub field_count: u32,
    pub review_digest: [u8; 32],
    pub forbidden: Option<Forbid>,
    /// Bitcoin 这笔交易用到的最高找零索引.签名后冷钱包要把它存下来.
    pub change_index: Option<u32>,
    /// `change_index` 属于测试币账户.本机不认识的网络也有这个值,它来自派生路径.
    pub change_is_testnet: bool,
}

impl Review {
    pub fn forbidden(network: &str, reason: Forbid) -> Self {
        Self {
            risk: Risk::Forbidden,
            confirm: Confirm::Refuse,
            network: network.to_string(),
            mainnet: is_mainnet(network),
            summary: Summary {
                title: Risk::Forbidden.label().to_string(),
                ..Summary::default()
            },
            alerts: vec![reason.to_string()],
            fields: Vec::new(),
            field_count: 0,
            review_digest: review_digest(&[], Risk::Forbidden, Confirm::Refuse),
            forbidden: Some(reason),
            change_index: None,
            change_is_testnet: false,
        }
    }

    /// 格式已识别而检查失败.页面显示失败原因和载荷的 SHA-256.
    pub fn undecodable(network: &str, failure: &Failure) -> Self {
        Self {
            alerts: vec![failure.to_string()],
            ..Self::forbidden(network, Forbid::ParseFailed)
        }
    }

    /// 解码树加策略的结果.类别是禁止时树也在,拒绝页能显示解码后的内容.
    pub fn assessed(decoded: &Decoded, assessment: Assessment) -> Self {
        let Assessment {
            risk,
            confirm,
            alerts,
            findings,
        } = assessment;
        let fields = field_rows(decoded, &findings);
        // 指向不存在节点的发现也是渲染和树不一致.
        let stray = findings.keys().any(|id| *id >= decoded.node_count());
        if let Err(failure) = decoded.check_rendered(fields.len() + usize::from(stray)) {
            return Self::undecodable(decoded.network(), &failure);
        }
        Self {
            risk,
            confirm,
            // 本机不认识的网络名来自请求,显示时不能带控制字符.
            network: decode::escape_text(decoded.network()),
            mainnet: is_mainnet(decoded.network()),
            summary: Summary {
                title: risk.label().to_string(),
                ..Summary::default()
            },
            alerts,
            field_count: fields.len() as u32,
            review_digest: review_digest(&fields, risk, confirm),
            fields,
            forbidden: None,
            change_index: None,
            change_is_testnet: false,
        }
    }

    /// 页面顶部.`title` 留空时是类别的名字.顶部的值都要落到列表的行上.
    pub fn summarized(mut self, mut summary: Summary) -> Self {
        if summary.title.is_empty() {
            summary.title.clone_from(&self.summary.title);
        }
        self.summary = summary.linked(&self.fields);
        self
    }

    /// 加一条告警.告警只升级确认方式,类别不变,摘要要跟着确认方式重算.
    pub fn alerted(mut self, alert: &str) -> Self {
        self.alerts.push(alert.to_string());
        self.confirm = self.confirm.max(Confirm::of(self.risk, true));
        self.review_digest = review_digest(&self.fields, self.risk, self.confirm);
        self
    }

    pub fn is_signable(&self) -> bool {
        self.confirm != Confirm::Refuse
    }

    /// 手机带回的结果和这次核对相同才能签名:确认的就是这份内容,每一行都画出来了,要二次确认的也确认过了.
    pub fn matches_shown(&self, digest: &[u8], rendered: u32, confirmed_twice: bool) -> bool {
        self.review_digest.as_slice() == digest
            && self.field_count == rendered
            && (confirmed_twice || self.confirm != Confirm::HoldConfirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::fixture::{PAYLOAD, Records, Tampered, UNKNOWN_FORMAT, input};
    use crate::decode::{Extent, Node, Source, Value, Walked, decode};

    fn assessment(risk: Risk) -> Assessment {
        Assessment {
            risk,
            confirm: Confirm::of(risk, false),
            alerts: Vec::new(),
            findings: BTreeMap::new(),
        }
    }

    fn reviewed(payload: &[u8]) -> Review {
        Review::assessed(&decode(&Records, &input(payload)).unwrap(), assessment(Risk::Transfer))
    }

    #[test]
    fn an_added_alert_raises_the_confirmation_and_changes_the_digest() {
        let plain = reviewed(&PAYLOAD);
        let alerted = plain.clone().alerted("提示");
        assert_eq!((plain.confirm, alerted.confirm), (Confirm::Tap, Confirm::Hold));
        assert_ne!(plain.review_digest, alerted.review_digest);
        let refused = Review::forbidden("tron", Forbid::ParseFailed).alerted("提示");
        assert_eq!(refused.confirm, Confirm::Refuse);
        assert_eq!(refused.alerts.len(), 2);
    }

    #[test]
    fn an_added_alert_cannot_lower_the_second_confirmation() {
        let mut review = reviewed(&PAYLOAD);
        review.risk = Risk::Unknown;
        review.confirm = Confirm::HoldConfirm;
        assert_eq!(review.alerted("提示").confirm, Confirm::HoldConfirm);
    }

    #[test]
    fn forbidden_cannot_be_signed() {
        let review = Review::forbidden("tron", Forbid::ParseFailed);
        assert!(!review.is_signable());
        assert!(review.fields.is_empty());
        assert_eq!(review.summary.title, Risk::Forbidden.label());
    }

    #[test]
    fn the_title_defaults_to_the_class() {
        assert_eq!(reviewed(&PAYLOAD).summary.title, Risk::Transfer.label());
        let named = reviewed(&PAYLOAD).summarized(Summary {
            title: "创建 nonce 账户".to_string(),
            ..Summary::default()
        });
        assert_eq!(named.summary.title, "创建 nonce 账户");
    }

    /// 摘要的每个值都引用列表里的行.
    #[test]
    fn every_summary_value_points_at_a_row() {
        let memo = reviewed(&PAYLOAD).fields[3].value.clone();
        let review = reviewed(&PAYLOAD).summarized(Summary {
            amount: Some(AmountView {
                text: "256".to_string(),
                symbol: None,
                asset: None,
                unlimited: false,
            }),
            counterparty: Some(memo),
            ..Summary::default()
        });
        assert_eq!(review.summary.field_ids, [2, 3]);
        let seeded = reviewed(&PAYLOAD).summarized(Summary {
            field_ids: vec![1, 1],
            ..Summary::default()
        });
        assert_eq!(seeded.summary.field_ids, [1]);
    }

    #[test]
    fn every_node_becomes_a_row_with_its_parent_and_depth() {
        let review = reviewed(&PAYLOAD);
        assert_eq!(review.field_count, 4);
        let names: Vec<_> = review.fields.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["records", "network", "amount", "memo"]);
        assert_eq!(review.fields[2].parent, Some(0));
        assert_eq!(review.fields[2].depth, 1);
        assert_eq!(review.fields[2].value, "256");
        assert_eq!(review.fields[2].source, SourceKind::Signed);
        assert_eq!((review.fields[2].offset, review.fields[2].len), (Some(2), Some(2)));
        assert_eq!(review.fields[1].source, SourceKind::Derived);
        assert_eq!((review.fields[1].offset, review.fields[1].len), (None, None));
    }

    #[test]
    fn a_missing_label_shows_the_escaped_name() {
        assert_eq!(field_label(Chain::Evm, "not_in_the_file"), "not_in_the_file");
        assert_eq!(field_label(Chain::Evm, "evil\u{202E}name"), "evil\\u{202e}name");
    }

    #[test]
    fn a_chain_can_have_its_own_label_for_a_shared_name() {
        assert_eq!(field_label(Chain::Evm, "value"), "金额");
        assert_eq!(field_label(Chain::Bitcoin, "value"), "值");
    }

    #[test]
    fn findings_attach_by_node_id() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let mut assessed = assessment(Risk::Approval);
        assessed.findings.insert(
            2,
            Finding {
                risk: Some(Risk::Approval),
                note: Some("备注".to_string()),
                display: Some("1 USDT".to_string()),
                own: true,
            },
        );
        let review = Review::assessed(&decoded, assessed);
        let row = &review.fields[2];
        assert_eq!(row.value, "1 USDT");
        assert_eq!(
            (row.risk, row.note.as_deref(), row.own),
            (Some(Risk::Approval), Some("备注"), true)
        );
        assert_eq!(review.fields[3].risk, None);
    }

    #[test]
    fn a_finding_on_a_missing_node_fails_the_render() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let mut assessed = assessment(Risk::Transfer);
        assessed.findings.insert(99, Finding::default());
        let review = Review::assessed(&decoded, assessed);
        assert_eq!(review.forbidden, Some(Forbid::ParseFailed));
        assert!(review.alerts[0].contains(decoded.payload_digest()));
    }

    #[test]
    fn a_refused_tree_keeps_its_fields_for_the_refuse_page() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let review = Review::assessed(&decoded, assessment(Risk::Forbidden));
        assert!(!review.is_signable());
        assert_eq!(review.fields.len(), 4);
    }

    #[test]
    fn an_opaque_root_renders_as_one_row_plus_the_envelope() {
        let review = reviewed(&[UNKNOWN_FORMAT, 0xab]);
        assert_eq!(review.field_count, 2);
        assert_eq!(
            (review.fields[0].kind, review.fields[0].value.as_str()),
            (Kind::Opaque, "ffab")
        );
    }

    #[test]
    fn a_decode_failure_shows_the_reason_and_the_digest() {
        let failure = decode(
            &Tampered(|walked: &mut Walked| {
                walked.root.children.pop();
            }),
            &input(&PAYLOAD),
        )
        .unwrap_err();
        let review = Review::undecodable("ethereum", &failure);
        assert!(!review.is_signable());
        assert_eq!(review.forbidden, Some(Forbid::ParseFailed));
        assert!(review.alerts[0].contains(&failure.digest));
    }

    #[test]
    fn the_digest_covers_rows_risk_and_confirmation() {
        let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
        let base = Review::assessed(&decoded, assessment(Risk::Transfer));
        let riskier = Review::assessed(&decoded, assessment(Risk::Approval));
        assert_ne!(base.review_digest, riskier.review_digest);
        let other = decode(
            &Tampered(|walked: &mut Walked| {
                walked.root.children[1]
                    .children
                    .push(Node::new("extra", Value::Bool(true), Source::Derived));
                walked.root.children[1].source = Source::Signed(Extent::Own(crate::decode::Span::new(6, 3)));
            }),
            &input(&PAYLOAD),
        )
        .unwrap();
        let more = Review::assessed(&other, assessment(Risk::Transfer));
        assert_ne!(base.review_digest, more.review_digest);
    }

    #[test]
    fn signing_needs_the_shown_digest_and_every_row() {
        let review = reviewed(&PAYLOAD);
        assert!(review.matches_shown(&review.review_digest, 4, false));
        assert!(!review.matches_shown(&review.review_digest, 3, false));
        assert!(!review.matches_shown(&review.review_digest, 5, false));
        assert!(!review.matches_shown(&[0; 32], 4, false));
        assert!(!review.matches_shown(&[], 4, false));
    }

    #[test]
    fn the_second_confirmation_cannot_be_skipped() {
        let mut review = reviewed(&PAYLOAD);
        review.confirm = Confirm::HoldConfirm;
        assert!(!review.matches_shown(&review.review_digest, 4, false));
        assert!(review.matches_shown(&review.review_digest, 4, true));
    }

    #[test]
    fn a_network_counts_as_mainnet_unless_it_is_a_known_testnet() {
        assert!(is_mainnet("ethereum"));
        assert!(is_mainnet("made-up"));
        assert!(!is_mainnet("sepolia"));
    }

    #[test]
    fn the_builtin_labels_are_not_blank() {
        assert!(labels().values().all(|label| !label.trim().is_empty()));
    }
}

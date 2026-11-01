//! 核对结果.解析和分类都在核心库里完成,界面只负责画出来,所以 Android 和 iOS 的判断不会不一致.

use crate::decode::{self, Decoded, Failure, Kind, NodeId, SourceKind};
use crate::policy::{Assessment, Finding};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use thiserror::Error;

pub use crate::policy::{Confirm, Risk};

/// 禁止签名的原因.见 docs/04-security.md 第 2.7 节.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Forbid {
    #[error("这个请求不属于本钱包,如果用了 BIP39 密码,请检查是否输错")]
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
    #[error("这笔交易没有 Chain ID,签名可以在多条链上重放")]
    LegacyWithoutChainId,
    #[error("这个授权的 chain_id 为 0,对所有链生效")]
    UniversalAuthorization,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub label: String,
    pub value: String,
    pub mono: bool,
}

impl Row {
    pub fn text(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            mono: false,
        }
    }

    pub fn mono(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            mono: true,
        }
    }
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

/// 一个动作:一次调用,一条指令或一个输出.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub risk: Risk,
    pub title: String,
    pub rows: Vec<Row>,
}

/// 原始数据的一行.`note` 是分析出的提示,比如疑似地址.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawLine {
    pub text: String,
    pub note: Option<String>,
}

/// 字段列表的一行.`value` 已经按种类格式化,手机不做格式化.见 docs/10-tx-decoding.md 第 4 节.
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

/// 字段名的显示文字.数据文件里没有时显示字段名,EIP-712 的字段名来自载荷,所以要转义.
fn field_label(name: &str) -> String {
    LABELS
        .get_or_init(|| serde_json::from_str(include_str!("../assets/field-labels.json")).expect("内置字段名有效"))
        .get(name)
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
                label: field_label(&node.name),
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub risk: Risk,
    pub confirm: Confirm,
    pub network: String,
    pub title: String,
    pub amount: Option<AmountView>,
    /// 收款地址,被授权方,新控制方等.
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    /// 高风险原因.不为空时要长按.
    pub alerts: Vec<String>,
    pub actions: Vec<ActionView>,
    pub rows: Vec<Row>,
    pub raw: Vec<RawLine>,
    /// 解码树的全部节点.不为空时手机用通用列表,还没迁移的链是空的.
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
            title: Risk::Forbidden.label().to_string(),
            amount: None,
            counterparty_label: None,
            counterparty: None,
            alerts: vec![reason.to_string()],
            actions: Vec::new(),
            rows: Vec::new(),
            raw: Vec::new(),
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
            title: risk.label().to_string(),
            amount: None,
            counterparty_label: None,
            counterparty: None,
            alerts,
            actions: Vec::new(),
            rows: Vec::new(),
            raw: Vec::new(),
            field_count: fields.len() as u32,
            review_digest: review_digest(&fields, risk, confirm),
            fields,
            forbidden: None,
            change_index: None,
            change_is_testnet: false,
        }
    }

    /// 加一条告警.告警只升级确认方式,类别不变,摘要要跟着确认方式重算.
    pub fn alerted(mut self, alert: &str) -> Self {
        self.alerts.push(alert.to_string());
        self.confirm = Confirm::of(self.risk, true);
        self.review_digest = review_digest(&self.fields, self.risk, self.confirm);
        self
    }

    pub fn is_signable(&self) -> bool {
        self.confirm != Confirm::Refuse
    }

    /// 手机带回的结果和这次核对相同才能签名:确认的就是这份内容,而且每一行都画出来了.
    pub fn matches_shown(&self, digest: &[u8], rendered: u32) -> bool {
        self.review_digest.as_slice() == digest && self.field_count == rendered
    }
}

/// 收集动作,最后统一算出风险和确认方式,页面不用自己判断.
#[derive(Default)]
pub struct ReviewBuilder {
    pub amount: Option<AmountView>,
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    pub alerts: Vec<String>,
    pub actions: Vec<ActionView>,
    pub rows: Vec<Row>,
    pub raw: Vec<RawLine>,
    pub change_index: Option<u32>,
}

impl ReviewBuilder {
    pub fn finish(self, network: &str) -> Review {
        let risk = self
            .actions
            .iter()
            .map(|a| a.risk)
            .max_by_key(|r| r.legacy_rank())
            .unwrap_or(Risk::Unknown);
        let confirm = Confirm::of(risk, !self.alerts.is_empty());
        Review {
            risk,
            confirm,
            network: network.to_string(),
            title: risk.label().to_string(),
            amount: self.amount,
            counterparty_label: self.counterparty_label,
            counterparty: self.counterparty,
            alerts: self.alerts,
            actions: self.actions,
            rows: self.rows,
            raw: self.raw,
            fields: Vec::new(),
            field_count: 0,
            review_digest: review_digest(&[], risk, confirm),
            forbidden: None,
            change_index: self.change_index,
            change_is_testnet: false,
        }
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

    fn action(risk: Risk) -> ActionView {
        ActionView {
            risk,
            title: String::new(),
            rows: Vec::new(),
        }
    }

    #[test]
    fn risk_is_the_highest_of_all_actions() {
        let builder = ReviewBuilder {
            actions: vec![action(Risk::Transfer), action(Risk::Approval), action(Risk::Recognized)],
            ..Default::default()
        };
        let review = builder.finish("ethereum");
        assert_eq!(review.risk, Risk::Approval);
        assert_eq!(review.confirm, Confirm::Hold);
    }

    #[test]
    fn alerts_raise_a_plain_transfer_to_hold() {
        let plain = ReviewBuilder {
            actions: vec![action(Risk::Transfer)],
            ..Default::default()
        };
        assert_eq!(plain.finish("bitcoin").confirm, Confirm::Tap);
        let alerted = ReviewBuilder {
            actions: vec![action(Risk::Transfer)],
            alerts: vec!["手续费偏高".into()],
            ..Default::default()
        };
        assert_eq!(alerted.finish("bitcoin").confirm, Confirm::Hold);
    }

    #[test]
    fn an_added_alert_raises_the_confirmation_and_changes_the_digest() {
        let builder = ReviewBuilder {
            actions: vec![action(Risk::Transfer)],
            ..Default::default()
        };
        let plain = builder.finish("bitcoin");
        let alerted = plain.clone().alerted("提示");
        assert_eq!((plain.confirm, alerted.confirm), (Confirm::Tap, Confirm::Hold));
        assert_ne!(plain.review_digest, alerted.review_digest);
        let refused = Review::forbidden("tron", Forbid::ParseFailed).alerted("提示");
        assert_eq!(refused.confirm, Confirm::Refuse);
        assert_eq!(refused.alerts.len(), 2);
    }

    #[test]
    fn no_actions_is_never_treated_as_safe() {
        assert_eq!(ReviewBuilder::default().finish("ethereum").risk, Risk::Unknown);
    }

    #[test]
    fn forbidden_cannot_be_signed() {
        let review = Review::forbidden("tron", Forbid::ParseFailed);
        assert!(!review.is_signable());
    }

    #[test]
    fn a_mix_on_the_old_path_keeps_its_headline() {
        let builder = ReviewBuilder {
            actions: vec![action(Risk::Unknown), action(Risk::Approval)],
            ..Default::default()
        };
        let review = builder.finish("ethereum");
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
    }

    #[test]
    fn the_old_path_has_no_fields() {
        let review = ReviewBuilder::default().finish("ethereum");
        assert!(review.fields.is_empty());
        assert_eq!(review.field_count, 0);
        assert!(Review::forbidden("tron", Forbid::ParseFailed).fields.is_empty());
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
        assert_eq!(field_label("max_fee_per_gas"), "max_fee_per_gas");
        assert_eq!(field_label("evil\u{202E}name"), "evil\\u{202e}name");
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
        assert!(review.matches_shown(&review.review_digest, 4));
        assert!(!review.matches_shown(&review.review_digest, 3));
        assert!(!review.matches_shown(&review.review_digest, 5));
        assert!(!review.matches_shown(&[0; 32], 4));
        assert!(!review.matches_shown(&[], 4));
    }

    #[test]
    fn the_builtin_labels_are_not_blank() {
        field_label("anything");
        assert!(LABELS.get().unwrap().values().all(|label| !label.trim().is_empty()));
    }
}

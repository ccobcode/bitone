//! 核对结果.解析和分类都在核心库里完成,界面只负责画出来,所以 Android 和 iOS 的判断不会不一致.

use serde::Serialize;
use thiserror::Error;

/// 风险从低到高排序,一笔交易取全部动作里最高的一个.见 docs/04-security.md 第 2.7 节.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Transfer,
    Recognized,
    Unknown,
    Approval,
    PermissionChange,
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
    pub forbidden: Option<Forbid>,
    /// Bitcoin 这笔交易用到的最高找零索引.签名后冷钱包要把它存下来.
    pub change_index: Option<u32>,
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
            forbidden: Some(reason),
            change_index: None,
        }
    }

    pub fn is_signable(&self) -> bool {
        self.confirm != Confirm::Refuse
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
        let risk = self.actions.iter().map(|a| a.risk).max().unwrap_or(Risk::Unknown);
        let confirm = match risk {
            Risk::Forbidden => Confirm::Refuse,
            Risk::Unknown | Risk::Approval | Risk::PermissionChange => Confirm::Hold,
            Risk::Transfer | Risk::Recognized if self.alerts.is_empty() => Confirm::Tap,
            Risk::Transfer | Risk::Recognized => Confirm::Hold,
        };
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
            forbidden: None,
            change_index: self.change_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn no_actions_is_never_treated_as_safe() {
        assert_eq!(ReviewBuilder::default().finish("ethereum").risk, Risk::Unknown);
    }

    #[test]
    fn forbidden_cannot_be_signed() {
        let review = Review::forbidden("tron", Forbid::ParseFailed);
        assert!(!review.is_signable());
    }
}

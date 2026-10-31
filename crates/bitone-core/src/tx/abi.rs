//! EVM 风格的合约调用解析,EVM 和 TRON 共用.规则见 docs/04-security.md 第 2.7 节.

use crate::manifest::Manifest;
use crate::review::{ActionView, AmountView, Risk, Row};
use crate::units::{decimal_from_be, format_units};
use bitcoin::hex::DisplayHex;

pub type Word = [u8; 32];

const TRANSFER: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
const APPROVE: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];
const INCREASE_ALLOWANCE: [u8; 4] = [0x39, 0x50, 0x93, 0x51];
/// USDT 等旧合约用的名字.
const INCREASE_APPROVAL: [u8; 4] = [0xd7, 0x3d, 0xd6, 0x23];
const SET_APPROVAL_FOR_ALL: [u8; 4] = [0xa2, 0x2c, 0xb4, 0x65];
const PERMIT: [u8; 4] = [0xd5, 0x05, 0xac, 0xcf];
const PERMIT2_APPROVE: [u8; 4] = [0x87, 0x51, 0x7c, 0x45];
const MULTICALL: [u8; 4] = [0xac, 0x96, 0x50, 0xd8];
const MULTICALL_DEADLINE: [u8; 4] = [0x5a, 0xe4, 0x01, 0xdc];

/// multicall 最多展开的层数和条数,超过时按无法识别处理.
const MAX_DEPTH: usize = 3;
const MAX_INNER_CALLS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    Transfer {
        to: [u8; 20],
        amount: Word,
    },
    Approve {
        method: &'static str,
        spender: [u8; 20],
        amount: Word,
        irregular: bool,
    },
    SetApprovalForAll {
        operator: [u8; 20],
        approved: bool,
        irregular: bool,
    },
    Permit2Approve {
        token: [u8; 20],
        spender: [u8; 20],
        amount: Word,
        irregular: bool,
    },
    Multicall(Vec<Vec<u8>>),
    Other,
}

fn word(args: &[u8], index: usize) -> Option<Word> {
    args.get(index * 32..(index + 1) * 32)?.try_into().ok()
}

/// 高 12 字节不为 0 的地址参数不合法,按无法识别处理,防止显示和实际不一致.
fn address(args: &[u8], index: usize) -> Option<[u8; 20]> {
    let value = word(args, index)?;
    if value[..12] != [0; 12] {
        return None;
    }
    value[12..].try_into().ok()
}

fn boolean(args: &[u8], index: usize) -> Option<bool> {
    let value = word(args, index)?;
    if value[..31] != [0; 31] || value[31] > 1 {
        return None;
    }
    Some(value[31] == 1)
}

fn offset(args: &[u8], index: usize) -> Option<usize> {
    let value = word(args, index)?;
    if value[..24] != [0; 24] {
        return None;
    }
    usize::try_from(u64::from_be_bytes(value[24..].try_into().ok()?)).ok()
}

/// 解析 `bytes[]`.`base` 是偏移量的起点.
fn bytes_array(args: &[u8], base: usize) -> Option<Vec<Vec<u8>>> {
    let tail = args.get(base..)?;
    let count = offset(tail, 0)?;
    if count > MAX_INNER_CALLS {
        return None;
    }
    let head = tail.get(32..)?;
    (0..count)
        .map(|i| {
            let start = offset(head, i)?;
            let len = offset(head.get(start..)?, 0)?;
            Some(head.get(start + 32..start.checked_add(32)?.checked_add(len)?)?.to_vec())
        })
        .collect()
}

/// 授权类调用按前缀判断.合约照样会执行带尾部字节或高位不干净的调用,
/// 所以这类数据不能降级成无法识别,只标为参数异常.
fn loose_address(args: &[u8], index: usize) -> Option<[u8; 20]> {
    word(args, index)?[12..].try_into().ok()
}

fn irregular(args: &[u8], words: usize, addresses: &[usize]) -> bool {
    args.len() != words * 32 || addresses.iter().any(|i| address(args, *i).is_none())
}

fn approve(args: &[u8], method: &'static str) -> Option<Call> {
    Some(Call::Approve {
        method,
        spender: loose_address(args, 0)?,
        amount: word(args, 1)?,
        irregular: irregular(args, 2, &[0]),
    })
}

pub fn decode(data: &[u8]) -> Call {
    let Some((selector, args)) = data.split_first_chunk::<4>() else {
        return Call::Other;
    };
    let call = match *selector {
        // 转账相反:格式不对就不是普通转账.
        TRANSFER if args.len() == 64 => address(args, 0)
            .zip(word(args, 1))
            .map(|(to, amount)| Call::Transfer { to, amount }),
        APPROVE => approve(args, "approve"),
        INCREASE_ALLOWANCE => approve(args, "increaseAllowance"),
        INCREASE_APPROVAL => approve(args, "increaseApproval"),
        SET_APPROVAL_FOR_ALL => {
            loose_address(args, 0)
                .zip(word(args, 1))
                .map(|(operator, value)| Call::SetApprovalForAll {
                    operator,
                    approved: value != [0; 32],
                    irregular: irregular(args, 2, &[0]) || boolean(args, 1).is_none(),
                })
        }
        PERMIT => loose_address(args, 1)
            .zip(word(args, 2))
            .map(|(spender, amount)| Call::Approve {
                method: "permit",
                spender,
                amount,
                irregular: irregular(args, 7, &[0, 1]),
            }),
        PERMIT2_APPROVE => loose_address(args, 0)
            .zip(loose_address(args, 1))
            .zip(word(args, 2))
            .map(|((token, spender), amount)| Call::Permit2Approve {
                token,
                spender,
                amount,
                irregular: irregular(args, 4, &[0, 1]),
            }),
        MULTICALL => offset(args, 0)
            .and_then(|base| bytes_array(args, base))
            .map(Call::Multicall),
        MULTICALL_DEADLINE => offset(args, 1)
            .and_then(|base| bytes_array(args, base))
            .map(Call::Multicall),
        _ => None,
    };
    call.unwrap_or(Call::Other)
}

/// 地址的写法随链不同,比如 TRON 用 Base58.
pub struct Style<'a> {
    pub network: &'a str,
    pub manifest: &'a Manifest,
    pub format: &'a dyn Fn(&[u8; 20]) -> String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    pub risk: Risk,
    pub title: String,
    pub amount: Option<AmountView>,
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    pub rows: Vec<Row>,
    pub alerts: Vec<String>,
    /// multicall 的内部调用.每一个都要让用户看到.
    pub inner: Vec<Classified>,
}

impl Classified {
    /// 金额和对方写进行里,给列表展示用.单个动作时这些内容在页面顶部,不用重复.
    pub fn detailed(&self) -> ActionView {
        let mut rows = Vec::new();
        if let Some(amount) = &self.amount {
            let symbol = amount.symbol.as_deref().map(|s| format!(" {s}")).unwrap_or_default();
            let text = if amount.unlimited {
                "无限".to_string()
            } else {
                amount.text.clone()
            };
            rows.push(Row::text("金额", format!("{text}{symbol}")));
        }
        if let (Some(label), Some(party)) = (&self.counterparty_label, &self.counterparty) {
            rows.push(Row::mono(label, party.clone()));
        }
        rows.extend(self.rows.iter().cloned());
        ActionView {
            risk: self.risk,
            title: self.title.clone(),
            rows,
        }
    }

    pub fn actions(&self) -> Vec<ActionView> {
        let mut list = vec![ActionView {
            risk: self.risk,
            title: self.title.clone(),
            rows: self.rows.clone(),
        }];
        for call in &self.inner {
            if call.inner.is_empty() {
                list.push(call.detailed());
            } else {
                list.extend(call.actions());
            }
        }
        list
    }
}

/// uint160 的最大值就是 Permit2 的"无限".更大的额度也一样对待.
fn is_unlimited(amount: &Word) -> bool {
    amount[..12] != [0; 12] || amount[12..] == [0xff; 20]
}

fn amount_view(style: &Style, contract: &str, amount: &Word) -> AmountView {
    let raw = decimal_from_be(amount);
    match style.manifest.lookup(style.network, contract) {
        Some(asset) => AmountView {
            text: format_units(&raw, asset.decimals),
            symbol: Some(asset.symbol.clone()),
            asset: Some(asset.key()),
            unlimited: false,
        },
        None => AmountView {
            text: raw,
            symbol: None,
            asset: None,
            unlimited: false,
        },
    }
}

fn listed(style: &Style, contract: &str) -> &'static str {
    if style.manifest.lookup(style.network, contract).is_some() {
        "在内置清单"
    } else {
        "不在内置清单"
    }
}

fn approval(
    style: &Style,
    contract: &str,
    method: &str,
    spender: &[u8; 20],
    amount: &Word,
    irregular: bool,
) -> Classified {
    let mut view = amount_view(style, contract, amount);
    view.unlimited = is_unlimited(amount);
    Classified {
        risk: Risk::Approval,
        title: method.to_string(),
        amount: Some(view),
        counterparty_label: Some("被授权方".into()),
        counterparty: Some((style.format)(spender)),
        rows: vec![
            Row::text("调用方法", method),
            Row::mono("合约", contract),
            Row::text("合约清单", listed(style, contract)),
        ],
        alerts: if irregular {
            vec!["授权参数格式异常,实际效果可能和显示的不同".to_string()]
        } else {
            Vec::new()
        },
        inner: Vec::new(),
    }
}

fn unknown(contract: &str, data: &[u8]) -> Classified {
    let selector = data
        .get(..4)
        .map_or_else(|| "无".to_string(), |s| format!("0x{}", s.to_lower_hex_string()));
    Classified {
        risk: Risk::Unknown,
        title: Risk::Unknown.label().to_string(),
        amount: None,
        counterparty_label: Some("目标合约".into()),
        counterparty: Some(contract.to_string()),
        rows: vec![
            Row::mono("方法选择器", selector),
            Row::text("数据长度", format!("{} 字节", data.len())),
        ],
        alerts: Vec::new(),
        inner: Vec::new(),
    }
}

/// 给一次合约调用定类别.`has_value` 表示这次调用同时转了原生币.
pub fn classify(style: &Style, contract: &[u8; 20], data: &[u8], has_value: bool) -> Classified {
    classify_at(style, contract, data, has_value, 0)
}

fn classify_at(style: &Style, contract: &[u8; 20], data: &[u8], has_value: bool, depth: usize) -> Classified {
    let contract_text = (style.format)(contract);
    match decode(data) {
        // 普通转账是白名单:合约在清单里,且没有附带原生币.其余一律无法识别.
        Call::Transfer { to, amount } => {
            let in_manifest = style.manifest.lookup(style.network, &contract_text).is_some();
            if !in_manifest || has_value {
                let mut result = unknown(&contract_text, data);
                result.title = "未知合约的转账".to_string();
                result.amount = Some(amount_view(style, &contract_text, &amount));
                result.rows.push(Row::mono("声称的收款方", (style.format)(&to)));
                return result;
            }
            Classified {
                risk: Risk::Transfer,
                title: Risk::Transfer.label().to_string(),
                amount: Some(amount_view(style, &contract_text, &amount)),
                counterparty_label: Some("收款地址".into()),
                counterparty: Some((style.format)(&to)),
                rows: vec![
                    Row::text("调用方法", "transfer"),
                    Row::mono("合约", contract_text.clone()),
                    Row::text("合约清单", listed(style, &contract_text)),
                ],
                alerts: Vec::new(),
                inner: Vec::new(),
            }
        }
        Call::Approve {
            method,
            spender,
            amount,
            irregular,
        } => approval(style, &contract_text, method, &spender, &amount, irregular),
        Call::Permit2Approve {
            token,
            spender,
            amount,
            irregular,
        } => {
            let token_text = (style.format)(&token);
            let mut result = approval(style, &token_text, "Permit2 approve", &spender, &amount, irregular);
            result.rows.push(Row::mono("Permit2 合约", contract_text));
            result
        }
        Call::SetApprovalForAll {
            operator,
            approved,
            irregular,
        } => {
            let mut result = approval(
                style,
                &contract_text,
                "setApprovalForAll",
                &operator,
                &[0xff; 32],
                irregular,
            );
            if !approved && !irregular {
                result.risk = Risk::Unknown;
                result.title = "撤销全部授权".to_string();
                result.amount = None;
            }
            result
        }
        Call::Multicall(calls) if depth < MAX_DEPTH => {
            let inner: Vec<Classified> = calls
                .iter()
                .map(|call| classify_at(style, contract, call, false, depth + 1))
                .collect();
            // 一组调用不算普通转账,最低是无法识别.
            let top = inner
                .iter()
                .map(|c| c.risk)
                .max()
                .unwrap_or(Risk::Unknown)
                .max(Risk::Unknown);
            let mut result = unknown(&contract_text, data);
            result.risk = top;
            result.title = "批量调用".to_string();
            result.rows.push(Row::text("内部调用", format!("{} 个", inner.len())));
            // 只有一个动作是最高风险时才放到页面顶部,否则金额和对方会误导用户.
            let mut leaders = inner.iter().filter(|c| c.risk == top);
            if let (Some(leader), None) = (leaders.next(), leaders.next())
                && leader.risk > Risk::Unknown
            {
                result.amount = leader.amount.clone();
                result.counterparty_label = leader.counterparty_label.clone();
                result.counterparty = leader.counterparty.clone();
            } else {
                result.counterparty_label = None;
                result.counterparty = None;
            }
            result.alerts = inner.iter().flat_map(|c| c.alerts.clone()).collect();
            result.inner = inner;
            result
        }
        _ => {
            let mut result = unknown(&contract_text, data);
            // 附带原生币的调用不适用已知函数的展示.
            if let Some(known) = style
                .manifest
                .function(style.network, &contract_text, &selector_hex(data))
                .filter(|_| !has_value)
            {
                result.risk = Risk::Recognized;
                result.title = known.title.clone();
            }
            result
        }
    }
}

fn selector_hex(data: &[u8]) -> String {
    data.get(..4).map_or_else(String::new, |s| s.to_lower_hex_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::evm_text;
    use sha3::{Digest, Keccak256};

    const USDT: [u8; 20] = [
        0xda, 0xc1, 0x7f, 0x95, 0x8d, 0x2e, 0xe5, 0x23, 0xa2, 0x20, 0x62, 0x06, 0x99, 0x45, 0x97, 0xc1, 0x3d, 0x83,
        0x1e, 0xc7,
    ];
    const OTHER: [u8; 20] = [0x11; 20];
    const PEER: [u8; 20] = [0x22; 20];

    fn style() -> Style<'static> {
        Style {
            network: "ethereum",
            manifest: Manifest::builtin(),
            format: &evm_text,
        }
    }

    fn selector(signature: &str) -> [u8; 4] {
        Keccak256::digest(signature.as_bytes())[..4].try_into().unwrap()
    }

    fn call(sel: [u8; 4], words: &[Word]) -> Vec<u8> {
        let mut data = sel.to_vec();
        words.iter().for_each(|w| data.extend_from_slice(w));
        data
    }

    fn addr(a: [u8; 20]) -> Word {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(&a);
        w
    }

    fn uint(v: u64) -> Word {
        let mut w = [0u8; 32];
        w[24..].copy_from_slice(&v.to_be_bytes());
        w
    }

    #[test]
    fn selectors_match_their_signatures() {
        assert_eq!(TRANSFER, selector("transfer(address,uint256)"));
        assert_eq!(APPROVE, selector("approve(address,uint256)"));
        assert_eq!(INCREASE_ALLOWANCE, selector("increaseAllowance(address,uint256)"));
        assert_eq!(INCREASE_APPROVAL, selector("increaseApproval(address,uint256)"));
        assert_eq!(SET_APPROVAL_FOR_ALL, selector("setApprovalForAll(address,bool)"));
        assert_eq!(
            PERMIT,
            selector("permit(address,address,uint256,uint256,uint8,bytes32,bytes32)")
        );
        assert_eq!(PERMIT2_APPROVE, selector("approve(address,address,uint160,uint48)"));
        assert_eq!(MULTICALL, selector("multicall(bytes[])"));
        assert_eq!(MULTICALL_DEADLINE, selector("multicall(uint256,bytes[])"));
    }

    #[test]
    fn listed_token_transfer_is_plain() {
        let result = classify(
            &style(),
            &USDT,
            &call(TRANSFER, &[addr(PEER), uint(1_250_000_000)]),
            false,
        );
        assert_eq!(result.risk, Risk::Transfer);
        let amount = result.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1250", Some("USDT")));
    }

    #[test]
    fn a_fake_token_cannot_get_the_green_label() {
        let result = classify(
            &style(),
            &OTHER,
            &call(TRANSFER, &[addr(PEER), uint(1_250_000_000)]),
            false,
        );
        assert_eq!(result.risk, Risk::Unknown);
        let amount = result.amount.unwrap();
        assert_eq!(amount.text, "1250000000");
        assert_eq!(amount.symbol, None);
    }

    #[test]
    fn transfer_with_native_value_is_not_plain() {
        let result = classify(&style(), &USDT, &call(TRANSFER, &[addr(PEER), uint(1)]), true);
        assert_eq!(result.risk, Risk::Unknown);
    }

    #[test]
    fn trailing_bytes_are_not_a_plain_transfer() {
        let mut data = call(TRANSFER, &[addr(PEER), uint(1)]);
        data.push(0);
        assert_eq!(classify(&style(), &USDT, &data, false).risk, Risk::Unknown);
    }

    #[test]
    fn dirty_address_word_is_rejected() {
        let mut word = addr(PEER);
        word[0] = 1;
        assert_eq!(
            classify(&style(), &USDT, &call(TRANSFER, &[word, uint(1)]), false).risk,
            Risk::Unknown
        );
    }

    #[test]
    fn approvals_are_red_on_any_contract() {
        for contract in [USDT, OTHER] {
            let result = classify(&style(), &contract, &call(APPROVE, &[addr(PEER), [0xff; 32]]), false);
            assert_eq!(result.risk, Risk::Approval);
            assert!(result.amount.unwrap().unlimited);
            assert_eq!(result.counterparty_label.as_deref(), Some("被授权方"));
        }
    }

    #[test]
    fn set_approval_for_all_is_unlimited_and_revoking_is_not_an_approval() {
        let grant = classify(
            &style(),
            &OTHER,
            &call(SET_APPROVAL_FOR_ALL, &[addr(PEER), uint(1)]),
            false,
        );
        assert_eq!(grant.risk, Risk::Approval);
        assert!(grant.amount.unwrap().unlimited);
        let revoke = classify(
            &style(),
            &OTHER,
            &call(SET_APPROVAL_FOR_ALL, &[addr(PEER), uint(0)]),
            false,
        );
        assert_eq!(revoke.risk, Risk::Unknown);
    }

    #[test]
    fn irregular_approvals_stay_approvals() {
        let mut trailing = call(APPROVE, &[addr(PEER), uint(1)]);
        trailing.push(0);
        let mut dirty = addr(PEER);
        dirty[0] = 1;
        for data in [trailing, call(APPROVE, &[dirty, uint(1)])] {
            let result = classify(&style(), &USDT, &data, false);
            assert_eq!(result.risk, Risk::Approval);
            assert_eq!(result.alerts.len(), 1);
        }
        let usdt = classify(&style(), &USDT, &call(INCREASE_APPROVAL, &[addr(PEER), uint(1)]), false);
        assert_eq!((usdt.risk, usdt.alerts.len()), (Risk::Approval, 0));
    }

    #[test]
    fn irregular_revoke_is_not_trusted() {
        let mut dirty = addr(PEER);
        dirty[0] = 1;
        let result = classify(&style(), &OTHER, &call(SET_APPROVAL_FOR_ALL, &[dirty, uint(0)]), false);
        assert_eq!(result.risk, Risk::Approval);
    }

    fn multicall(calls: &[Vec<u8>]) -> Vec<u8> {
        let mut data = MULTICALL.to_vec();
        data.extend_from_slice(&uint(32));
        data.extend_from_slice(&uint(calls.len() as u64));
        let mut body = Vec::new();
        let mut heads = Vec::new();
        for item in calls {
            heads.extend_from_slice(&uint((calls.len() * 32 + body.len()) as u64));
            body.extend_from_slice(&uint(item.len() as u64));
            body.extend_from_slice(item);
            body.resize(body.len().div_ceil(32) * 32, 0);
        }
        data.extend_from_slice(&heads);
        data.extend_from_slice(&body);
        data
    }

    #[test]
    fn approval_hidden_inside_multicall_is_found() {
        let inner = vec![
            call(TRANSFER, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(PEER), [0xff; 32]]),
        ];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.risk, Risk::Approval);
    }

    #[test]
    fn every_inner_call_is_shown() {
        let attacker = [0x33; 20];
        let inner = vec![
            call(TRANSFER, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(PEER), uint(1)]),
            call(APPROVE, &[addr(attacker), [0xff; 32]]),
        ];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        let actions = result.actions();
        assert_eq!(actions.len(), 4);
        assert_eq!(actions[0].risk, Risk::Approval);
        assert!(
            actions
                .iter()
                .any(|a| a.rows.iter().any(|r| r.value == evm_text(&attacker)))
        );
        assert!(actions.iter().any(|a| a.rows.iter().any(|r| r.value == "无限 USDT")));
        // 两个授权并列最高,顶部不能只显示其中一个.
        assert_eq!(result.counterparty, None);
        assert_eq!(result.amount, None);
    }

    #[test]
    fn a_single_approval_in_a_multicall_leads_the_page() {
        let inner = vec![call(APPROVE, &[addr(PEER), [0xff; 32]])];
        let result = classify(&style(), &USDT, &multicall(&inner), false);
        assert_eq!(result.counterparty, Some(evm_text(&PEER)));
        assert!(result.amount.unwrap().unlimited);
    }

    #[test]
    fn transfers_in_a_multicall_keep_the_summary_risk() {
        let inner = vec![call(TRANSFER, &[addr(PEER), uint(1)])];
        let actions = classify(&style(), &USDT, &multicall(&inner), false).actions();
        assert_eq!(actions.iter().map(|a| a.risk).max(), Some(Risk::Unknown));
    }

    #[test]
    fn multicall_of_transfers_is_not_a_plain_transfer() {
        let inner = vec![call(TRANSFER, &[addr(PEER), uint(1)])];
        assert_eq!(classify(&style(), &USDT, &multicall(&inner), false).risk, Risk::Unknown);
    }

    #[test]
    fn nested_multicall_is_followed_until_the_limit() {
        let approve = call(APPROVE, &[addr(PEER), uint(1)]);
        let two = multicall(&[multicall(std::slice::from_ref(&approve))]);
        assert_eq!(classify(&style(), &USDT, &two, false).risk, Risk::Approval);
        let deep = multicall(&[multicall(&[multicall(&[multicall(std::slice::from_ref(&approve))])])]);
        assert_eq!(classify(&style(), &USDT, &deep, false).risk, Risk::Unknown);
    }

    #[test]
    fn malformed_multicall_does_not_panic() {
        for bad in [
            MULTICALL.to_vec(),
            call(MULTICALL, &[uint(u64::MAX)]),
            call(MULTICALL, &[uint(32), uint(u64::MAX)]),
            call(MULTICALL, &[uint(32), uint(1), uint(u64::MAX)]),
            call(MULTICALL, &[uint(32), uint(1), uint(32), uint(u64::MAX)]),
        ] {
            assert_eq!(classify(&style(), &USDT, &bad, false).risk, Risk::Unknown);
        }
    }

    #[test]
    fn known_function_only_changes_the_label() {
        let mut manifest = Manifest::builtin().clone();
        manifest.functions.push(crate::manifest::KnownFunction {
            network: "ethereum".into(),
            contract: evm_text(&OTHER),
            selector: "12345678".into(),
            title: "质押".into(),
        });
        let style = Style {
            network: "ethereum",
            manifest: &manifest,
            format: &evm_text,
        };
        let result = classify(&style, &OTHER, &[0x12, 0x34, 0x56, 0x78], false);
        assert_eq!((result.risk, result.title.as_str()), (Risk::Recognized, "质押"));
        let approve = classify(&style, &OTHER, &call(APPROVE, &[addr(PEER), uint(1)]), false);
        assert_eq!(approve.risk, Risk::Approval);
        let with_value = classify(&style, &OTHER, &[0x12, 0x34, 0x56, 0x78], true);
        assert_eq!(with_value.risk, Risk::Unknown);
    }
}

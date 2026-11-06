//! EIP-712 和 TIP-712 的页面顶部.解码和摘要在 `decode::parse_typed`,类别和告警都来自策略.

use crate::decode::{TypedMessage, typed_member};
use crate::policy::PermitFact;
use crate::review::AmountView;
use crate::tx::abi::Style;
use crate::units::format_units;
use alloy_dyn_abi::DynSolValue;
use alloy_primitives::U256;

/// 代币和额度.
struct Grant {
    token: Option<String>,
    amount: U256,
}

fn uint(value: &DynSolValue) -> Option<U256> {
    value.as_uint().map(|(number, _)| number)
}

fn grant_of(style: &Style, entry: &DynSolValue) -> Option<Grant> {
    Some(Grant {
        token: typed_member(entry, "token")?
            .as_address()
            .map(|address| (style.format)(&address.into_array())),
        amount: uint(typed_member(entry, "amount")?)?,
    })
}

fn grants_of(style: &Style, list: &DynSolValue) -> Option<Vec<Grant>> {
    match list {
        DynSolValue::Array(items) => items.iter().map(|item| grant_of(style, item)).collect(),
        item => Some(vec![grant_of(style, item)?]),
    }
}

/// 被授权方,代币和额度.批量授权取全部代币.类型串已经逐字比对过,这些成员一定在.
fn permit_grants(parsed: &TypedMessage, style: &Style) -> Option<(String, Vec<Grant>)> {
    let message = &parsed.message;
    let spender = typed_member(message, "spender")?.as_address()?;
    let grants = match parsed.primary.as_str() {
        "Permit" => vec![Grant {
            token: parsed
                .verifying_contract()
                .map(|address| (style.format)(&address.into_array())),
            amount: uint(typed_member(message, "value")?)?,
        }],
        "PermitSingle" | "PermitBatch" => grants_of(style, typed_member(message, "details")?)?,
        "PermitTransferFrom" | "PermitBatchTransferFrom" => grants_of(style, typed_member(message, "permitted")?)?,
        _ => return None,
    };
    Some(((style.format)(&spender.into_array()), grants))
}

/// 页面顶部.
#[derive(Default)]
pub(crate) struct TypedView {
    pub amount: Option<AmountView>,
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    pub permit: Option<PermitFact>,
}

fn permit_view(parsed: &TypedMessage, style: &Style) -> Option<TypedView> {
    let (spender, grants) = permit_grants(parsed, style)?;
    let shown: Vec<AmountView> = grants
        .iter()
        .map(|Grant { token, amount }| {
            let raw = amount.to_string();
            let unlimited = *amount >= U256::from(alloy_primitives::U160::MAX);
            let asset = token
                .as_ref()
                .and_then(|token| style.manifest.lookup(style.network, token));
            match asset {
                Some(asset) => AmountView {
                    text: format_units(&raw, asset.decimals),
                    symbol: Some(asset.symbol.clone()),
                    asset: Some(asset.key()),
                    unlimited,
                },
                None => AmountView {
                    text: raw,
                    symbol: None,
                    asset: None,
                    unlimited,
                },
            }
        })
        .collect();
    let permit = PermitFact {
        grants: shown.len(),
        unlimited: shown.iter().filter(|amount| amount.unlimited).count(),
    };
    Some(TypedView {
        // 多笔时顶部不放金额,否则小额会盖住后面的无限额度.全部额度都在字段列表里.
        amount: <[AmountView; 1]>::try_from(shown).ok().map(|[only]| only),
        counterparty_label: Some("被授权方".into()),
        counterparty: Some(spender),
        permit: Some(permit),
    })
}

/// 描述文件给的字段名.绑定是网络,`verifyingContract`,类型串,类型串里已经有 `primaryType`.
pub(crate) fn labels(parsed: &TypedMessage, style: &Style) -> Option<Vec<(String, String)>> {
    let contract = (style.format)(&parsed.verifying_contract()?.into_array());
    let mut found = style.manifest.descriptors(style.network, &contract);
    let descriptor = found.find(|descriptor| descriptor.signature == parsed.encode_type)?;
    let fields = descriptor.format.fields.iter();
    Some(fields.map(|field| (field.path.clone(), field.label.clone())).collect())
}

pub(crate) fn view(parsed: &TypedMessage, style: &Style) -> TypedView {
    parsed
        .is_permit()
        .then(|| permit_view(parsed, style))
        .flatten()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::evm_text;
    use crate::chains::evm::{keccak, typed_data_hash};
    use crate::decode::{Failure, Reject, TypedFlavor, parse_typed, permit_types};
    use bitcoin::hex::DisplayHex;
    use serde_json::{Value, json};

    /// 期望值是 EIP-2612 和 Permit2 合约里的常量,防止类型串抄错.
    #[test]
    fn accepted_types_match_the_contracts() {
        let hashes = [
            "6e71edae12b1b97f4d1f60370fef10105fa2faae0126114a169c64845d6126c9",
            "af1b0d30d2cab0380e68f0689007e3254993c596f2fdd0aaa7f4d04f79440863",
            "fcf35f5ac6a2c28868dc44c302166470266239195f02b0ee408334829333b766",
            "f3841cd1ff0085026a6327b620b67997ce40f282c88a8e905a7a5626e310f3d0",
            "939c21a48a8dbe3a9a2404a1d46691e4d39f6583d6ec6b35714604c986d80106",
        ];
        let found: Vec<String> = permit_types()
            .iter()
            .map(|canonical| keccak(canonical.as_bytes()).to_lower_hex_string())
            .collect();
        for hash in hashes {
            assert!(found.iter().any(|item| item == hash), "{hash}");
        }
        assert_eq!(found.len(), hashes.len());
    }

    fn mail() -> Value {
        json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                "Person": [{"name": "name", "type": "string"}, {"name": "wallet", "type": "address"}],
                "Mail": [
                    {"name": "from", "type": "Person"},
                    {"name": "to", "type": "Person"},
                    {"name": "contents", "type": "string"}
                ]
            },
            "primaryType": "Mail",
            "domain": {
                "name": "Ether Mail",
                "version": "1",
                "chainId": 1,
                "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
            },
            "message": {
                "from": {"name": "Cow", "wallet": "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"},
                "to": {"name": "Bob", "wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"},
                "contents": "Hello, Bob!"
            }
        })
    }

    fn parsed(data: &Value) -> Result<TypedMessage, Reject> {
        parse_typed(&serde_json::to_vec(data).unwrap(), TypedFlavor::Eip712)
    }

    /// EIP-712 规范里的 Mail 示例.
    #[test]
    fn matches_the_eip712_example() {
        let hash = typed_data_hash(&serde_json::to_vec(&mail()).unwrap()).unwrap();
        assert_eq!(
            hash.to_lower_hex_string(),
            "be609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2"
        );
    }

    /// `serde_json` 保留最后一个,签名和屏幕上的内容就能不一样.
    #[test]
    fn duplicate_keys_are_refused_at_every_level() {
        let text = serde_json::to_string(&mail()).unwrap();
        for (old, new) in [
            ("\"contents\":\"Hello, Bob!\"", "\"contents\":\"A\",\"contents\":\"B\""),
            ("\"chainId\":1", "\"chainId\":1,\"chainId\":137"),
            (
                "\"primaryType\":\"Mail\"",
                "\"primaryType\":\"Mail\",\"primaryType\":\"Person\"",
            ),
            (
                "\"contents\":\"Hello, Bob!\"",
                "\"contents\":\"A\",\"\\u0063ontents\":\"B\"",
            ),
        ] {
            let payload = text.replacen(old, new, 1);
            assert_ne!(payload, text);
            assert_eq!(
                parse_typed(payload.as_bytes(), TypedFlavor::Eip712).unwrap_err(),
                Reject::Malformed,
                "{new}"
            );
        }
        assert!(parse_typed(text.as_bytes(), TypedFlavor::Eip712).is_ok());
    }

    #[test]
    fn every_member_must_be_consumed() {
        let mut top = mail();
        top["extra"] = 1.into();
        let mut message = mail();
        message["message"]["from"]["hidden"] = "x".into();
        let mut domain = mail();
        domain["domain"]["salt"] = "x".into();
        let mut field = mail();
        field["types"]["Person"][0]["internalType"] = "x".into();
        let mut missing = mail();
        missing["message"].as_object_mut().unwrap().remove("contents");
        for data in [top, message, domain, field, missing] {
            assert!(parsed(&data).is_err(), "{data}");
        }
        let mut no_message = mail();
        no_message.as_object_mut().unwrap().remove("message");
        assert!(parsed(&no_message).is_err());
    }

    #[test]
    fn numbers_that_parsers_disagree_on_are_refused() {
        for number in [
            "1.5",
            "1e3",
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        ] {
            let text = serde_json::to_string(&mail()).unwrap();
            let payload = text.replacen("\"chainId\":1", &format!("\"chainId\":{number}"), 1);
            assert!(
                parse_typed(payload.as_bytes(), TypedFlavor::Eip712).is_err(),
                "{number}"
            );
        }
    }

    #[test]
    fn values_must_fit_their_types() {
        let mut data = json!({
            "types": {"EIP712Domain": [], "T": [{"name": "x", "type": "uint8"}]},
            "primaryType": "T",
            "domain": {},
            "message": {"x": 256}
        });
        assert!(parsed(&data).is_err());
        data["message"]["x"] = 255.into();
        assert!(parsed(&data).is_ok());
        data["types"]["T"][0]["type"] = "uint8[2]".into();
        data["message"]["x"] = json!([1]);
        assert!(parsed(&data).is_err());
        for kind in ["(uint8)", "function", "Missing", "uint7", "trcToken"] {
            data["types"]["T"][0]["type"] = kind.into();
            data["message"]["x"] = 1.into();
            assert!(parsed(&data).is_err(), "{kind}");
        }
    }

    /// 域按请求写的类型哈希,和 viem 一致.alloy 的 `Eip712Domain` 固定顺序,不能用来算这一步.
    #[test]
    fn the_domain_follows_its_declared_type() {
        let mut data = mail();
        let hash = |data: &Value| parsed(data).unwrap().domain_separator;
        let first = hash(&data);
        data["types"]["EIP712Domain"].as_array_mut().unwrap().reverse();
        assert_ne!(hash(&data), first);
    }

    #[test]
    fn a_missing_domain_type_is_inferred_from_its_members() {
        let mut data = mail();
        let declared = parsed(&data).unwrap().digest;
        data["types"].as_object_mut().unwrap().remove("EIP712Domain");
        assert_eq!(parsed(&data).unwrap().digest, declared);
        data["domain"]["extra"] = 1.into();
        assert!(parsed(&data).is_err());
    }

    #[test]
    fn a_domain_only_message_must_be_empty() {
        let mut data = json!({
            "types": {"EIP712Domain": [{"name": "name", "type": "string"}]},
            "primaryType": "EIP712Domain",
            "domain": {"name": "Only"},
            "message": {}
        });
        assert_eq!(parsed(&data).unwrap().hash_struct, None);
        data["message"]["x"] = 1.into();
        assert!(parsed(&data).is_err());
    }

    /// 类型里的字段按 JSON 顺序进哈希,不是字母序.
    #[test]
    fn the_tree_and_the_digest_share_one_value() {
        let data = parsed(&mail()).unwrap();
        let from = typed_member(&data.message, "from").unwrap();
        assert_eq!(
            evm_text(&typed_member(from, "wallet").unwrap().as_address().unwrap().into_array()),
            "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"
        );
    }

    #[test]
    fn a_failure_keeps_the_reason() {
        let failure = Failure {
            reason: parsed(&json!({})).unwrap_err(),
            digest: String::new(),
        };
        assert_eq!(failure.reason, Reject::Malformed);
    }
}

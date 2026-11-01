//! EIP-712.只接受 Permit 和 Permit2,其余一律禁止,见 docs/04-security.md 第 2.7 节.

use super::{keccak, signer_path_row};
use crate::Network;
use crate::address::{evm_text, parse_evm_address};
use crate::path::DerivationPath;
use crate::request::SignRequest;
use crate::review::{ActionView, AmountView, Forbid, Review, ReviewBuilder, Risk, Row};
use crate::tx::ReviewContext;
use crate::tx::tron::utc_time;
use crate::units::{decimal_from_be, format_units};
use bitcoin::hex::FromHex;
use serde_json::Value;
use std::collections::BTreeSet;

/// 接受的消息和它们的规范类型串,逐字比对.名字相同但字段不同的是另一种消息,
/// 比如 DAI 的 Permit 用 `allowed` 代替 `value`,这里显示的额度就和实际授权无关.
const ACCEPTED: [(&str, &str); 5] = [
    (
        "Permit",
        "Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)",
    ),
    (
        "PermitSingle",
        "PermitSingle(PermitDetails details,address spender,uint256 sigDeadline)\
         PermitDetails(address token,uint160 amount,uint48 expiration,uint48 nonce)",
    ),
    (
        "PermitBatch",
        "PermitBatch(PermitDetails[] details,address spender,uint256 sigDeadline)\
         PermitDetails(address token,uint160 amount,uint48 expiration,uint48 nonce)",
    ),
    (
        "PermitTransferFrom",
        "PermitTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline)\
         TokenPermissions(address token,uint256 amount)",
    ),
    (
        "PermitBatchTransferFrom",
        "PermitBatchTransferFrom(TokenPermissions[] permitted,address spender,uint256 nonce,uint256 deadline)\
         TokenPermissions(address token,uint256 amount)",
    ),
];

fn field_list<'a>(types: &'a Value, name: &str) -> Result<Vec<(&'a str, &'a str)>, Forbid> {
    types[name]
        .as_array()
        .ok_or(Forbid::ParseFailed)?
        .iter()
        .map(|f| {
            Ok((
                f["name"].as_str().ok_or(Forbid::ParseFailed)?,
                f["type"].as_str().ok_or(Forbid::ParseFailed)?,
            ))
        })
        .collect()
}

fn base_type(kind: &str) -> &str {
    kind.split('[').next().unwrap_or(kind)
}

fn dependencies<'a>(types: &'a Value, name: &'a str, found: &mut BTreeSet<&'a str>) -> Result<(), Forbid> {
    if found.contains(name) || types.get(name).is_none() {
        return Ok(());
    }
    found.insert(name);
    for (_, kind) in field_list(types, name)? {
        dependencies(types, base_type(kind), found)?;
    }
    Ok(())
}

fn encode_type(types: &Value, primary: &str) -> Result<String, Forbid> {
    let mut found = BTreeSet::new();
    dependencies(types, primary, &mut found)?;
    found.remove(primary);
    let mut out = String::new();
    for name in std::iter::once(primary).chain(found.iter().copied()) {
        let fields: Vec<String> = field_list(types, name)?
            .iter()
            .map(|(n, k)| format!("{k} {n}"))
            .collect();
        out.push_str(&format!("{name}({})", fields.join(",")));
    }
    Ok(out)
}

/// 数值可以是 JSON 数字,十进制字符串或十六进制字符串.
fn uint_word(value: &Value) -> Result<[u8; 32], Forbid> {
    let text = match value {
        Value::Number(n) => n.as_u64().ok_or(Forbid::ParseFailed)?.to_string(),
        Value::String(s) => s.clone(),
        _ => return Err(Forbid::ParseFailed),
    };
    let bytes = match text.strip_prefix("0x") {
        Some(hex) => {
            let padded = if hex.len().is_multiple_of(2) {
                hex.to_string()
            } else {
                format!("0{hex}")
            };
            hex_bytes(&padded)?
        }
        None => super::be_from_decimal(&text).map_err(|_| Forbid::ParseFailed)?,
    };
    if bytes.len() > 32 {
        return Err(Forbid::ParseFailed);
    }
    let mut word = [0u8; 32];
    word[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(word)
}

fn hex_bytes(hex: &str) -> Result<Vec<u8>, Forbid> {
    Vec::from_hex(hex).map_err(|_| Forbid::ParseFailed)
}

fn encode_value(types: &Value, kind: &str, value: &Value) -> Result<[u8; 32], Forbid> {
    if let Some(element) = kind.strip_suffix("[]") {
        let items = value.as_array().ok_or(Forbid::ParseFailed)?;
        let mut joined = Vec::with_capacity(items.len() * 32);
        for item in items {
            joined.extend_from_slice(&encode_value(types, element, item)?);
        }
        return Ok(keccak(&joined));
    }
    if types.get(kind).is_some() {
        return hash_struct(types, kind, value);
    }
    match kind {
        "address" => {
            let text = value.as_str().ok_or(Forbid::ParseFailed)?;
            let address = parse_evm_address(text).map_err(|_| Forbid::ParseFailed)?;
            let mut word = [0u8; 32];
            word[12..].copy_from_slice(&address);
            Ok(word)
        }
        "bool" => {
            let mut word = [0u8; 32];
            word[31] = value.as_bool().ok_or(Forbid::ParseFailed)? as u8;
            Ok(word)
        }
        "string" => Ok(keccak(value.as_str().ok_or(Forbid::ParseFailed)?.as_bytes())),
        "bytes" => {
            let text = value.as_str().ok_or(Forbid::ParseFailed)?;
            Ok(keccak(&hex_bytes(text.strip_prefix("0x").ok_or(Forbid::ParseFailed)?)?))
        }
        _ if kind.starts_with("uint") => uint_word(value),
        _ if kind.starts_with("bytes") => {
            let text = value.as_str().ok_or(Forbid::ParseFailed)?;
            let bytes = hex_bytes(text.strip_prefix("0x").ok_or(Forbid::ParseFailed)?)?;
            if bytes.len() > 32 {
                return Err(Forbid::ParseFailed);
            }
            let mut word = [0u8; 32];
            word[..bytes.len()].copy_from_slice(&bytes);
            Ok(word)
        }
        _ => Err(Forbid::Unsupported),
    }
}

fn hash_struct(types: &Value, name: &str, value: &Value) -> Result<[u8; 32], Forbid> {
    let fields = field_list(types, name)?;
    // 不进哈希的键不能留下,否则屏幕上的内容可以和签名无关.
    if value.as_object().map(|object| object.len()) != Some(fields.len()) {
        return Err(Forbid::ParseFailed);
    }
    let mut encoded = keccak(encode_type(types, name)?.as_bytes()).to_vec();
    for (field, kind) in fields {
        let item = value.get(field).ok_or(Forbid::ParseFailed)?;
        encoded.extend_from_slice(&encode_value(types, kind, item)?);
    }
    Ok(keccak(&encoded))
}

/// `eth_signTypedData_v4` 的哈希.
pub fn typed_data_hash(data: &Value) -> Result<[u8; 32], Forbid> {
    let types = &data["types"];
    let primary = data["primaryType"].as_str().ok_or(Forbid::ParseFailed)?;
    let mut preimage = vec![0x19, 0x01];
    preimage.extend_from_slice(&hash_struct(types, "EIP712Domain", &data["domain"])?);
    preimage.extend_from_slice(&hash_struct(types, primary, &data["message"])?);
    Ok(keccak(&preimage))
}

/// 类型来自请求,不可信.核对读的字段都要确实进了哈希,域里的网络和合约也要真的被签名承诺.
fn check_schema(data: &Value) -> Result<(), Forbid> {
    let types = &data["types"];
    let primary = data["primaryType"].as_str().ok_or(Forbid::MessageSigning)?;
    let (_, canonical) = ACCEPTED
        .iter()
        .find(|(name, _)| *name == primary)
        .ok_or(Forbid::MessageSigning)?;
    if encode_type(types, primary)? != *canonical {
        return Err(Forbid::MessageSigning);
    }
    let domain = field_list(types, "EIP712Domain")?;
    if !domain.contains(&("chainId", "uint256")) {
        return Err(Forbid::ChainIdMismatch);
    }
    if !domain.contains(&("verifyingContract", "address")) {
        return Err(Forbid::MessageSigning);
    }
    Ok(())
}

fn text_field<'a>(value: &'a Value, name: &str) -> Result<&'a str, Forbid> {
    value[name].as_str().ok_or(Forbid::ParseFailed)
}

/// 代币,额度和额度的过期时间.
type Grant = (String, [u8; 32], Option<[u8; 32]>);

/// 被授权方,代币和额度.批量授权取全部代币.
fn permit_grants(data: &Value) -> Result<(String, Vec<Grant>), Forbid> {
    let message = &data["message"];
    let spender = text_field(message, "spender")?.to_string();
    let grant = |token: &Value, amount: &Value, expiration: Option<&Value>| -> Result<Grant, Forbid> {
        Ok((
            token.as_str().ok_or(Forbid::ParseFailed)?.to_string(),
            uint_word(amount)?,
            expiration.map(uint_word).transpose()?,
        ))
    };
    let item = |entry: &Value| grant(&entry["token"], &entry["amount"], entry.get("expiration"));
    let items = |list: &Value| {
        list.as_array()
            .ok_or(Forbid::ParseFailed)?
            .iter()
            .map(item)
            .collect::<Result<Vec<_>, _>>()
    };
    let grants = match data["primaryType"].as_str() {
        Some("Permit") => vec![grant(&data["domain"]["verifyingContract"], &message["value"], None)?],
        Some("PermitSingle") => vec![item(&message["details"])?],
        Some("PermitBatch") => items(&message["details"])?,
        Some("PermitTransferFrom") => vec![item(&message["permitted"])?],
        Some("PermitBatchTransferFrom") => items(&message["permitted"])?,
        _ => return Err(Forbid::MessageSigning),
    };
    Ok((spender, grants))
}

/// 单位是秒.换算不到日期的值显示原始整数.
fn time_text(word: &[u8; 32]) -> String {
    let (high, low) = word.split_at(24);
    let seconds = u64::from_be_bytes(low.try_into().expect("8 字节"));
    match seconds.checked_mul(1000) {
        Some(ms) if high == [0; 24] => utc_time(ms),
        _ => decimal_from_be(word),
    }
}

pub fn review(
    network: Network,
    request: &SignRequest,
    ctx: &ReviewContext,
    payer: Option<&str>,
    path: &DerivationPath,
) -> Result<Review, Forbid> {
    let data: Value = serde_json::from_slice(&request.payload).map_err(|_| Forbid::ParseFailed)?;
    check_schema(&data)?;
    let (spender, grants) = permit_grants(&data)?;
    let expected = network.evm_chain_id().ok_or(Forbid::UnknownNetwork)?;
    let chain_id = uint_word(data["domain"].get("chainId").ok_or(Forbid::ChainIdMismatch)?)
        .map_err(|_| Forbid::ChainIdMismatch)?;
    if chain_id[..24] != [0; 24] || u64::from_be_bytes(chain_id[24..].try_into().expect("8 字节")) != expected {
        return Err(Forbid::ChainIdMismatch);
    }
    typed_data_hash(&data)?;

    let mut builder = ReviewBuilder {
        counterparty_label: Some("被授权方".into()),
        counterparty: Some(evm_text(&parse_evm_address(&spender).map_err(|_| Forbid::ParseFailed)?)),
        ..Default::default()
    };
    let mut unlimited_grants = 0;
    for (token, amount, expiration) in &grants {
        let raw = decimal_from_be(amount);
        let unlimited = amount[..12] != [0; 12] || amount[12..] == [0xff; 20];
        let asset = ctx.manifest.lookup(network.id(), token);
        let view = match asset {
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
        };
        let listed = if asset.is_some() {
            "在内置清单"
        } else {
            "不在内置清单"
        };
        let mut rows = vec![
            Row::mono("代币合约", token.clone()),
            Row::text("合约清单", listed),
            Row::text(
                "额度",
                if unlimited {
                    "无限".to_string()
                } else {
                    view.text.clone()
                },
            ),
        ];
        rows.extend(expiration.map(|time| Row::text("额度过期时间", time_text(&time))));
        builder.actions.push(ActionView {
            risk: Risk::Approval,
            title: data["primaryType"].as_str().unwrap_or("Permit").to_string(),
            rows,
        });
        unlimited_grants += usize::from(unlimited);
        // 多笔时顶部不放金额,否则小额会盖住后面的无限额度.全部额度都在动作里.
        if grants.len() == 1 {
            builder.amount = Some(view);
        }
    }
    if grants.len() > 1 {
        builder
            .alerts
            .push(format!("这笔签名包含 {} 个授权,请逐个核对", grants.len()));
        // 横幅只看顶部金额,多笔时要靠告警说出无限额度.
        if unlimited_grants > 0 {
            builder.alerts.push(format!(
                "其中 {unlimited_grants} 个是无限额度,对方可随时转走全部这种资产"
            ));
        }
    }
    let contract = text_field(&data["domain"], "verifyingContract")?;
    let contract = parse_evm_address(contract).map_err(|_| Forbid::ParseFailed)?;
    builder.rows.extend(signer_path_row(network, expected, payer, path));
    builder.rows.push(Row::text("交易类型", "EIP-712 消息"));
    builder.rows.push(Row::mono("验证合约", evm_text(&contract)));
    for field in ["deadline", "sigDeadline"] {
        if let Some(value) = data["message"].get(field) {
            builder
                .rows
                .push(Row::text("签名截止时间", time_text(&uint_word(value)?)));
        }
    }
    Ok(builder.finish(network.id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hex::DisplayHex;
    use serde_json::json;

    /// 期望值是 EIP-2612 和 Permit2 合约里的常量,防止类型串抄错.
    #[test]
    fn accepted_types_match_the_contracts() {
        let hashes = [
            "6e71edae12b1b97f4d1f60370fef10105fa2faae0126114a169c64845d6126c9",
            "f3841cd1ff0085026a6327b620b67997ce40f282c88a8e905a7a5626e310f3d0",
            "af1b0d30d2cab0380e68f0689007e3254993c596f2fdd0aaa7f4d04f79440863",
            "939c21a48a8dbe3a9a2404a1d46691e4d39f6583d6ec6b35714604c986d80106",
            "fcf35f5ac6a2c28868dc44c302166470266239195f02b0ee408334829333b766",
        ];
        for ((name, canonical), hash) in ACCEPTED.iter().zip(hashes) {
            assert_eq!(keccak(canonical.as_bytes()).to_lower_hex_string(), hash, "{name}");
        }
    }

    /// EIP-712 规范里的 Mail 示例.
    #[test]
    fn matches_the_eip712_example() {
        let data = json!({
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
        });
        let hash = typed_data_hash(&data).unwrap();
        assert_eq!(
            hash.to_lower_hex_string(),
            "be609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2"
        );
    }
}

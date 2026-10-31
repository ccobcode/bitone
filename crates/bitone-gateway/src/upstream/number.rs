//! 上游返回的数字.

use super::Failed;
use bitcoin::hex::{DisplayHex, FromHex};
use bitone_core::tx::evm::be_from_decimal;
use bitone_core::units::decimal_from_be;
use serde_json::Value;

/// `0x` 开头的十六进制转十进制字符串.ERC-20 余额是 uint256,放不进任何原生整数.
pub fn hex_to_decimal(hex: &str) -> Option<String> {
    let hex = hex
        .strip_prefix("0x")
        .filter(|h| !h.is_empty())?
        .trim_start_matches('0');
    if hex.len() > 64 {
        return None;
    }
    let even = format!("{hex:0>width$}", width = hex.len().next_multiple_of(2));
    Some(decimal_from_be(&Vec::from_hex(&even).ok()?))
}

/// 十进制字符串转 `0x` 开头的十六进制,上限 uint256.
pub fn decimal_to_hex(decimal: &str) -> Option<String> {
    let hex = be_from_decimal(decimal).ok()?.to_lower_hex_string();
    let digits = hex.trim_start_matches('0');
    Some(format!("0x{}", if digits.is_empty() { "0" } else { digits }))
}

pub fn hex_u64(value: &Value) -> Result<u64, Failed> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or(Failed)?;
    u64::from_str_radix(hex, 16).map_err(|_| Failed)
}

pub fn hex_u128(value: &Value) -> Result<u128, Failed> {
    let hex = value.as_str().and_then(|text| text.strip_prefix("0x")).ok_or(Failed)?;
    u128::from_str_radix(hex, 16).map_err(|_| Failed)
}

/// 十进制整数字符串,或者 JSON 整数.
pub fn decimal(value: &Value) -> Result<String, Failed> {
    match value {
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => Ok(s.clone()),
        Value::Number(n) => n.as_u64().map(|n| n.to_string()).ok_or(Failed),
        _ => Err(Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hex_to_decimal_handles_uint256() {
        assert_eq!(hex_to_decimal("0x0").unwrap(), "0");
        assert_eq!(hex_to_decimal("0xff").unwrap(), "255");
        assert_eq!(hex_to_decimal("0xDE0B6B3A7640000").unwrap(), "1000000000000000000");
        assert_eq!(
            hex_to_decimal(&format!("0x{}", "f".repeat(64))).unwrap(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        );
        assert_eq!(hex_to_decimal(&format!("0x{:0>64}", "2a")).unwrap(), "42");
    }

    #[test]
    fn hex_to_decimal_rejects_garbage() {
        // 合约不存在时 eth_call 返回 0x,不能当成余额 0.
        for bad in ["0x", "ff", "", "0xzz", &format!("0x1{}", "0".repeat(64))] {
            assert_eq!(hex_to_decimal(bad), None, "{bad}");
        }
    }

    #[test]
    fn decimal_to_hex_round_trips() {
        for value in ["0", "1", "255", "1000000000000000000"] {
            assert_eq!(hex_to_decimal(&decimal_to_hex(value).unwrap()).unwrap(), value);
        }
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(decimal_to_hex(max).unwrap(), format!("0x{}", "f".repeat(64)));
    }

    #[test]
    fn decimal_to_hex_rejects_garbage() {
        let above_max = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        for bad in ["", "-1", "1.5", "0x10", "abc", above_max] {
            assert_eq!(decimal_to_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn decimal_accepts_only_integers() {
        assert_eq!(decimal(&json!("123")).unwrap(), "123");
        assert_eq!(decimal(&json!(123)).unwrap(), "123");
        for bad in [json!(""), json!("-1"), json!("1.5"), json!(1.5), json!(-1), json!(null)] {
            assert!(decimal(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hex_numbers_reject_garbage() {
        assert_eq!(hex_u64(&json!("0x10")).unwrap(), 16);
        for bad in [json!("0x"), json!("10"), json!(16), json!(null), json!("0xzz")] {
            assert!(hex_u64(&bad).is_err(), "{bad}");
        }
    }
}

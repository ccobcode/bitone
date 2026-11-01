//! 按种类格式化节点的值,完整不截断.手机不做格式化.

use super::model::{Amount, Value};
use crate::hex::DisplayHex;
use crate::units::{decimal_from_be, format_units};
use bitcoin::Script;

const MILLIS_PER_DAY: i64 = 86_400_000;

pub fn display(value: &Value) -> String {
    match value {
        Value::Bool(flag) => flag.to_string(),
        Value::Int(number) => number.to_string(),
        Value::Uint(raw) => decimal_from_be(raw),
        Value::Amount(amount) => display_amount(amount),
        Value::Address(text) | Value::Enum(text) => text.clone(),
        Value::Hash(bytes) | Value::Bytes(bytes) | Value::Opaque(bytes) => bytes.to_lower_hex_string(),
        Value::Text(text) => escape_text(text),
        Value::Time(millis) => display_time(*millis),
        Value::Script(bytes) => Script::from_bytes(bytes).to_asm_string(),
        Value::Struct | Value::List => String::new(),
    }
}

fn display_amount(amount: &Amount) -> String {
    let raw = decimal_from_be(&amount.raw);
    let number = amount.decimals.map_or_else(|| raw.clone(), |d| format_units(&raw, d));
    match &amount.unit {
        Some(unit) => format!("{number} {unit}"),
        None => number,
    }
}

/// `text` 的值里,控制字符,双向控制符和零宽字符都转义,反斜杠一起转义,转义结果才不会和原文混淆.
pub fn escape_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            escaped.push_str("\\\\");
        } else if is_hidden(c) {
            escaped.extend(c.escape_unicode());
        } else {
            escaped.push(c);
        }
    }
    escaped
}

fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}' | '\u{FEFF}'
        )
}

/// UTC,毫秒为 0 时省略.换算方法是 Howard Hinnant 的 `civil_from_days`.
fn display_time(millis: i64) -> String {
    let day = millis.div_euclid(MILLIS_PER_DAY);
    let of_day = millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month, date) = civil_from_days(day);
    let (hour, minute, second) = (of_day / 3_600_000, of_day / 60_000 % 60, of_day / 1000 % 60);
    let fraction = of_day % 1000;
    let clock = format!("{year:04}-{month:02}-{date:02}T{hour:02}:{minute:02}:{second:02}");
    if fraction == 0 {
        format!("{clock}Z")
    } else {
        format!("{clock}.{fraction:03}Z")
    }
}

fn civil_from_days(day: i64) -> (i64, i64, i64) {
    let shifted = day + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let date = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, date)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amount(raw: &[u8], decimals: Option<u8>, unit: Option<&str>) -> Value {
        Value::Amount(Amount {
            raw: raw.to_vec(),
            decimals,
            unit: unit.map(str::to_string),
        })
    }

    #[test]
    fn values_are_complete() {
        assert_eq!(display(&Value::Uint(vec![0xff; 32])).len(), 78);
        assert_eq!(display(&Value::Bytes(vec![0xab; 300])).len(), 600);
        assert_eq!(display(&Value::Int(-7)), "-7");
        assert_eq!(display(&Value::Bool(true)), "true");
        assert_eq!(display(&Value::Struct), "");
    }

    #[test]
    fn amounts_show_the_raw_integer_without_decimals() {
        assert_eq!(display(&amount(&[0x0f, 0x42, 0x40], Some(6), Some("USDT"))), "1 USDT");
        assert_eq!(display(&amount(&[0x0f, 0x42, 0x41], Some(6), None)), "1.000001");
        assert_eq!(display(&amount(&[0x0f, 0x42, 0x40], None, None)), "1000000");
    }

    #[test]
    fn hidden_characters_are_escaped() {
        assert_eq!(escape_text("a\nb"), "a\\u{a}b");
        assert_eq!(escape_text("safe\u{202E}evil"), "safe\\u{202e}evil");
        assert_eq!(escape_text("zero\u{200B}width"), "zero\\u{200b}width");
        assert_eq!(escape_text("\u{FEFF}"), "\\u{feff}");
        assert_eq!(escape_text("备注 ok"), "备注 ok");
    }

    #[test]
    fn a_literal_escape_cannot_pass_for_a_real_one() {
        assert_ne!(escape_text("\\u{202e}"), escape_text("\u{202E}"));
    }

    #[test]
    fn scripts_are_opcode_lists() {
        assert_eq!(display(&Value::Script(vec![0x51])), "OP_PUSHNUM_1");
    }

    #[test]
    fn times_are_utc() {
        assert_eq!(display(&Value::Time(0)), "1970-01-01T00:00:00Z");
        assert_eq!(display(&Value::Time(1_500_000_000_000)), "2017-07-14T02:40:00Z");
        assert_eq!(display(&Value::Time(1_500_000_000_123)), "2017-07-14T02:40:00.123Z");
        assert_eq!(display(&Value::Time(951_782_400_000)), "2000-02-29T00:00:00Z");
        assert_eq!(display(&Value::Time(-1)), "1969-12-31T23:59:59.999Z");
    }
}

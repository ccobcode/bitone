//! 金额格式化.金额全程是字节或十进制字符串,不经过浮点.

use crate::Error;

pub fn decimal_from_be(bytes: &[u8]) -> String {
    let mut digits = vec![0u8];
    for &byte in bytes {
        let mut carry = byte as u32;
        for digit in digits.iter_mut() {
            let value = *digit as u32 * 256 + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    digits.iter().rev().map(|d| char::from(b'0' + d)).collect()
}

/// 把最小单位的十进制字符串换算成带小数的形式,去掉末尾的 0.
pub fn format_units(raw: &str, decimals: u8) -> String {
    let decimals = decimals as usize;
    let raw = raw.trim_start_matches('0');
    let padded = format!("{raw:0>width$}", width = decimals + 1);
    let (whole, fraction) = padded.split_at(padded.len() - decimals);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// 十进制字符串转大端字节,去掉前导 0.
pub fn be_from_decimal(text: &str) -> Result<Vec<u8>, Error> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidParams);
    }
    let mut bytes: Vec<u8> = vec![];
    for digit in text.bytes().map(|b| (b - b'0') as u32) {
        let mut carry = digit;
        for byte in bytes.iter_mut().rev() {
            let value = *byte as u32 * 10 + carry;
            *byte = value as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    if bytes.len() > 32 {
        return Err(Error::InvalidParams);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_values_do_not_lose_precision() {
        assert_eq!(decimal_from_be(&[]), "0");
        assert_eq!(decimal_from_be(&[0, 0, 1, 0]), "256");
        assert_eq!(
            decimal_from_be(&[0xff; 32]),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    #[test]
    fn formats_units_exactly() {
        assert_eq!(format_units("1250000000", 6), "1250");
        assert_eq!(format_units("1", 18), "0.000000000000000001");
        assert_eq!(format_units("1500000000000000000", 18), "1.5");
        assert_eq!(format_units("0", 8), "0");
        assert_eq!(format_units("123", 0), "123");
    }
}

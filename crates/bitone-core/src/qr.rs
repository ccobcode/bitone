//! 二维码模块矩阵。两端都用它生成二维码，只负责画出来。

use crate::Error;
use qrcode::{Color, EcLevel, QrCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    pub size: u32,
    /// 按行排列，`true` 是深色模块。
    pub modules: Vec<bool>,
}

fn build(bytes: &[u8], level: EcLevel) -> Result<QrMatrix, Error> {
    let code = QrCode::with_error_correction_level(bytes, level).map_err(|_| Error::QrEncode)?;
    Ok(QrMatrix {
        size: code.width() as u32,
        modules: code.to_colors().into_iter().map(|c| c == Color::Dark).collect(),
    })
}

/// UR 帧。大写后走字母数字模式，容量更大。动态二维码靠喷泉码容错，所以纠错等级用 L。
pub fn matrix(part: &str) -> Result<QrMatrix, Error> {
    build(part.to_ascii_uppercase().as_bytes(), EcLevel::L)
}

/// 普通文本，比如给付款方扫的收款地址。内容不变，Base58 地址区分大小写。
pub fn text_matrix(text: &str) -> Result<QrMatrix, Error> {
    build(text.as_bytes(), EcLevel::M)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_keeps_its_case() {
        let address = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";
        assert_ne!(text_matrix(address), text_matrix(&address.to_ascii_uppercase()));
    }

    #[test]
    fn frames_are_case_insensitive() {
        assert_eq!(matrix("ur:bitone-address/1-1/abc"), matrix("UR:BITONE-ADDRESS/1-1/ABC"));
    }
}

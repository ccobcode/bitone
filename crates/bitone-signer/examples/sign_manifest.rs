//! 给内置清单签名，生成 `bitone-manifest` 清单包。见 docs/04-security.md 第 2.9 节。
//!
//!   sign_manifest <清单 JSON> <32 字节私钥的十六进制文件> <输出文件>
//!
//! 私钥必须离线保管，这个工具只在离线环境运行。包的格式是 64 字节签名加 JSON 原文。

use bitone_core::manifest::Manifest;
use ed25519_dalek::{Signer, SigningKey};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [manifest, key, output] = args.as_slice() else {
        eprintln!("用法：sign_manifest <清单 JSON> <私钥文件> <输出文件>");
        std::process::exit(2);
    };
    let body = std::fs::read(manifest).expect("读取清单");
    serde_json::from_slice::<Manifest>(&body).expect("清单格式有效");
    let hex = std::fs::read_to_string(key).expect("读取私钥");
    let hex = hex.trim();
    let secret: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("私钥是十六进制"))
        .collect();
    let secret: [u8; 32] = secret.try_into().expect("私钥是 32 字节");
    let signature = SigningKey::from_bytes(&secret).sign(&body);
    let mut package = signature.to_bytes().to_vec();
    package.extend_from_slice(&body);
    std::fs::write(output, package).expect("写入清单包");
}

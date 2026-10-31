//! 开发用的冷钱包：用测试助记词核对并签名，不要用在真实资金上。
//!
//! 网页端的端到端测试用它代替手机：
//!   cold_sign review <请求十六进制>    打印核对结果
//!   cold_sign sign <请求十六进制>      打印签名结果的 UR 帧，一行一帧，循环三遍
//!   cold_sign accounts                 打印观察钱包的 UR 帧

use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::protocol::{DEFAULT_FRAGMENT_BYTES, FrameEncoder, UR_ACCOUNTS};
use bitone_core::request::SignRequest;
use bitone_core::tx::{self, HighestChange, ReviewContext};
use bitone_signer::keys::Wallet;

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("十六进制有效"))
        .collect()
}

fn print_frames(encoder: &mut FrameEncoder) {
    for _ in 0..encoder.fragment_count() * 3 {
        println!("{}", encoder.next_part().expect("编码成功"));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wallet = Wallet::new(&[0u8; 16], "").expect("测试助记词有效");
    let keys = PublicKeys::decode(&wallet.public_keys(false).expect("派生成功")).expect("公钥有效");
    match args.first().map(String::as_str) {
        Some("accounts") => {
            let message = wallet.export(false).expect("派生成功").encode();
            let mut encoder = FrameEncoder::new(&message, UR_ACCOUNTS, DEFAULT_FRAGMENT_BYTES).expect("编码成功");
            print_frames(&mut encoder);
        }
        Some("review") => {
            let request = SignRequest::decode(&unhex(&args[1])).expect("请求有效");
            let ctx = ReviewContext {
                keys: &keys,
                manifest: Manifest::builtin(),
                highest_change: HighestChange::default(),
            };
            let review = tx::review(&request, &ctx);
            println!("{}", serde_json::to_string_pretty(&review).expect("序列化成功"));
        }
        Some("sign") => {
            let request = SignRequest::decode(&unhex(&args[1])).expect("请求有效");
            let (response, _) = wallet
                .sign(&request, &keys, Manifest::builtin(), HighestChange::default())
                .expect("这个请求不能签名");
            let mut encoder = response.frames(DEFAULT_FRAGMENT_BYTES).expect("编码成功");
            print_frames(&mut encoder);
        }
        _ => eprintln!("用法：cold_sign accounts|review|sign [请求十六进制]"),
    }
}

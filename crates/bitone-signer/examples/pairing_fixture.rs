//! 打印测试助记词的观察钱包二维码帧。网页端测试用它验证跨语言解码。

use bitone_core::protocol::{DEFAULT_FRAGMENT_BYTES, FrameEncoder, UR_ACCOUNTS};
use bitone_signer::keys::Wallet;

fn main() {
    let wallet = Wallet::new(&[0u8; 16], "").expect("测试助记词有效");
    let message = wallet.export(false).expect("派生成功").encode();
    let mut encoder = FrameEncoder::new(&message, UR_ACCOUNTS, DEFAULT_FRAGMENT_BYTES).expect("编码成功");
    for _ in 0..encoder.fragment_count() * 3 {
        println!("{}", encoder.next_part().expect("编码成功"));
    }
}

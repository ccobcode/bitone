#![no_main]

use alloy_dyn_abi::JsonAbiExt;
use alloy_json_abi::Function;
use bitone_core::Network;
use bitone_core::address::evm_text;
use bitone_core::manifest::Manifest;
use bitone_core::policy::CallShape;
use bitone_core::tx::abi::{Style, classify};
use libfuzzer_sys::fuzz_target;

/// 清单内的 USDT,金额按清单的小数位读.
const CONTRACT: [u8; 20] = [
    0xda, 0xc1, 0x7f, 0x95, 0x8d, 0x2e, 0xe5, 0x23, 0xa2, 0x20, 0x62, 0x06, 0x99, 0x45, 0x97, 0xc1, 0x3d, 0x83, 0x1e,
    0xc7,
];
const SELECTOR_LEN: usize = 4;

// 调用数据没有树.解成标准函数时,重新编码必须等于参数区,这同时说明字节用完了.
fuzz_target!(|data: &[u8]| {
    let style = Style {
        chain: Network::Ethereum.chain(),
        network: Network::Ethereum.id(),
        manifest: Manifest::builtin(),
        format: &evm_text,
    };
    let classified = classify(&style, &CONTRACT, data, false);
    if let CallShape::Standard(signature) = &classified.fact.shape {
        let function = Function::parse(signature).expect("标准函数的规范签名");
        let args = &data[SELECTOR_LEN..];
        let values = function.abi_decode_input(args).expect("已经按标准函数解开");
        let rebuilt = function.abi_encode_input_raw(&values).expect("解出的值能编码");
        assert_eq!(rebuilt, args, "C2");
    }
});

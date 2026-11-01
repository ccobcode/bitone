#![no_main]

use bitone_core::Network;
use bitone_core::decode::solana::{Instruction, Message, SolanaWalker, Version};
use bitone_core::request::KIND_TRANSACTION;
use bitone_fuzz::{exercise, program_ids};
use libfuzzer_sys::fuzz_target;

const WALKER: SolanaWalker = SolanaWalker { coin: None, hint: None };
const PAYER: [u8; 32] = [0x77; 32];

// 第一字节选程序,第二字节是账户数,其余是指令数据,让遍历器读 IDL.
fuzz_target!(|bytes: &[u8]| {
    let [selector, accounts, data @ ..] = bytes else {
        return;
    };
    let programs = program_ids();
    let program = programs[usize::from(*selector) % programs.len()];
    let accounts = usize::from(*accounts % 32);
    let mut keys = vec![PAYER];
    keys.extend((0..accounts).map(|index| [index as u8 + 1; 32]));
    keys.push(program);
    let message = Message {
        version: Version::Legacy,
        required_signatures: 1,
        readonly_signed: 0,
        readonly_unsigned: 0,
        instructions: vec![Instruction {
            program: keys.len() as u8 - 1,
            accounts: (1..=accounts as u8).collect(),
            data: data.to_vec(),
        }],
        keys,
        blockhash: [9; 32],
        lookups: Vec::new(),
    };
    exercise(&WALKER, Network::Solana, KIND_TRANSACTION, &message.encode());
});

#![no_main]

use bitone_core::Network;
use bitone_core::chains::solana::decode::SolanaWalker;
use bitone_core::request::KIND_TRANSACTION;
use bitone_fuzz::exercise;
use libfuzzer_sys::fuzz_target;

const WALKER: SolanaWalker = SolanaWalker { coin: None, hint: None };

fuzz_target!(|payload: &[u8]| {
    exercise(&WALKER, Network::Solana, KIND_TRANSACTION, payload);
});

#![no_main]

use bitone_core::Network;
use bitone_core::chains::tron::decode::TronWalker;
use bitone_core::request::KIND_TRANSACTION;
use bitone_fuzz::exercise;
use libfuzzer_sys::fuzz_target;

const WALKER: TronWalker<'static> = TronWalker { coin: None, call: &[] };

fuzz_target!(|payload: &[u8]| {
    exercise(&WALKER, Network::Tron, KIND_TRANSACTION, payload);
});

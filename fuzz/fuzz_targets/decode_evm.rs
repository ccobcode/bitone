#![no_main]

use bitone_core::Network;
use bitone_core::decode::EvmWalker;
use bitone_core::request::KIND_TRANSACTION;
use bitone_fuzz::exercise;
use libfuzzer_sys::fuzz_target;

const WALKER: EvmWalker<'static> = EvmWalker {
    coin: None,
    chain_id: None,
    call: &[],
};

fuzz_target!(|payload: &[u8]| {
    exercise(&WALKER, Network::Ethereum, KIND_TRANSACTION, payload);
});

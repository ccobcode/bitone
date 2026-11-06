#![no_main]

use bitone_core::Network;
use bitone_core::chains::bitcoin::PsbtWalker;
use bitone_core::request::KIND_TRANSACTION;
use bitone_fuzz::exercise;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|payload: &[u8]| {
    exercise(&PsbtWalker, Network::Bitcoin, KIND_TRANSACTION, payload);
});

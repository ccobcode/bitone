#![no_main]

use bitone_core::Network;
use bitone_core::address::evm_text;
use bitone_core::decode::{TypedFlavor, TypedWalker, parse_typed};
use bitone_core::request::KIND_TYPED_DATA;
use bitone_fuzz::exercise;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|payload: &[u8]| {
    let Ok(parsed) = parse_typed(payload, TypedFlavor::Eip712) else {
        return;
    };
    let walker = TypedWalker {
        parsed: &parsed,
        chain_id: Network::Ethereum.evm_chain_id(),
        format: &evm_text,
        labels: None,
    };
    exercise(&walker, Network::Ethereum, KIND_TYPED_DATA, payload);
});

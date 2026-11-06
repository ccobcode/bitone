//! 模糊测试目标共用的检查.
//!
//! `decode` 里 C1,C2 不成立时只是返回 `Err`,目标看不出来.所以解码成功后在 `decode` 之外重新算一遍,
//! 再走一遍冷钱包核对请求的路径.解码器和核对都不允许 panic.

use bitone_core::Network;
use bitone_core::address::parse_solana_address;
use bitone_core::decode::{Decoded, Decoder, Extent, Input, Span, Value, Walked, decode};
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::path::account_specs;
use bitone_core::protocol::{AccountsExport, VERSION};
use bitone_core::request::SignRequest;
use bitone_core::tx::{self, HighestChange, ReviewContext};
use serde::Deserialize;
use std::sync::OnceLock;

const IDL: &str = include_str!("../../crates/bitone-core/assets/solana/idl.json");
/// 向量里只有这个指纹能让 PSBT 通过 `prepare`,其余的是多签或路径不符,走不到策略.
const FINGERPRINT: [u8; 4] = [0x27, 0x56, 0x9c, 0x50];

#[derive(Deserialize)]
struct Program {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct Idl {
    programs: Vec<Program>,
}

/// IDL 里全部程序的地址.
pub fn program_ids() -> &'static [[u8; 32]] {
    static IDS: OnceLock<Vec<[u8; 32]>> = OnceLock::new();
    IDS.get_or_init(|| {
        let idl: Idl = serde_json::from_str(IDL).expect("内置 IDL 有效");
        let ids = idl.programs.iter().flat_map(|program| &program.ids);
        ids.map(|id| parse_solana_address(id).expect("程序地址是 32 字节的 base58"))
            .collect()
    })
}

/// C1:有字节范围的节点加结构字节恰好覆盖载荷,不重不漏.
fn assert_covered(decoded: &Decoded, payload_len: usize) {
    let visits = decoded.visits();
    let own = visits.iter().filter_map(|visit| match visit.node.source.extent() {
        Some(Extent::Own(span)) => Some(span),
        Some(Extent::Container | Extent::Inherited) | None => None,
    });
    let framing = visits.iter().flat_map(|visit| visit.node.framing.iter().copied());
    let mut spans: Vec<Span> = own.chain(framing).filter(|span| span.len > 0).collect();
    spans.sort_unstable_by_key(|span| span.offset);
    let end = spans
        .iter()
        .try_fold(0, |end, span| (u64::from(span.offset) == end).then(|| span.end()));
    assert_eq!(end, Some(payload_len as u64), "C1");
}

/// C2:重新编码逐字节相同,不能一项都不比.格式不认识时不走 C1,C2,整个载荷是一个 `opaque` 节点.
fn assert_rebuilt(decoder: &impl Decoder, input: &Input<'_>, decoded: &Decoded) {
    match decoder.walk(input).expect("解码成功过,再走一遍也要成功") {
        Some(Walked { round_trips, .. }) => {
            assert!(!round_trips.is_empty(), "C2");
            round_trips
                .iter()
                .for_each(|check| assert_eq!(check.original, check.rebuilt, "C2"));
        }
        None => assert!(matches!(decoded.root().value, Value::Opaque(_))),
    }
}

/// 没有保存公钥,签名账户是空的,所以任何交易都不会因为发起方不是本机而提前结束.
/// 策略和 `tx/summary.rs` 都在这条路径上,只有 `decode` 走不到它们.
fn review(network: Network, kind: u8, payload: &[u8]) {
    let keys = PublicKeys::new(AccountsExport {
        version: VERSION,
        fingerprint: FINGERPRINT,
        accounts: Vec::new(),
    });
    let context = ReviewContext {
        keys: &keys,
        manifest: Manifest::builtin(),
        highest_change: HighestChange::default(),
    };
    let specs = account_specs(network.mode(), false).into_iter();
    let request = SignRequest {
        version: VERSION,
        id: [0; 16],
        fingerprint: FINGERPRINT,
        network: network.id().to_string(),
        kind,
        paths: specs
            .filter(|spec| spec.chain == network.chain())
            .map(|spec| spec.path.to_string())
            .collect(),
        payload: payload.to_vec(),
        hint: None,
        chain: None,
    };
    tx::review(&request, &context);
}

/// 一次输入的全部检查.
pub fn exercise(decoder: &impl Decoder, network: Network, kind: u8, payload: &[u8]) {
    let input = Input {
        chain: network.chain(),
        mode: Some(network.mode()),
        network: network.id(),
        kind,
        paths: &[],
        payload,
        signer: None,
    };
    if let Ok(decoded) = decode(decoder, &input) {
        assert_covered(&decoded, payload.len());
        assert_rebuilt(decoder, &input, &decoded);
    }
    review(network, kind, payload);
}

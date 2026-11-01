use super::fixture::{PARTIAL, PAYLOAD, Records, Tampered, UNKNOWN_FORMAT, input};
use super::*;

fn signed(span: Span) -> Source {
    Source::Signed(Extent::Own(span))
}

fn reason<F: Fn(&mut Walked)>(tamper: F) -> Reject {
    decode(&Tampered(tamper), &input(&PAYLOAD)).unwrap_err().reason
}

#[test]
fn an_honest_decoder_passes() {
    let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
    assert_eq!((decoded.chain(), decoded.network()), (Chain::Evm, "ethereum"));
    assert_eq!(decoded.node_count(), 4);
    let names: Vec<_> = decoded.visits().iter().map(|v| v.node.name.clone()).collect();
    assert_eq!(names, ["records", "network", "amount", "memo"]);
}

#[test]
fn malformed_bytes_in_a_known_format_fail_with_the_digest() {
    let failure = decode(&Records, &input(&[1, 5])).unwrap_err();
    assert_eq!(failure.reason, Reject::Malformed);
    assert_eq!(failure.digest, check::payload_digest(&[1, 5]));
    assert!(failure.to_string().contains(&failure.digest));
}

// 不认识的内容不是失败

#[test]
fn an_unknown_format_is_an_opaque_root() {
    let payload = [UNKNOWN_FORMAT, 1, 2, 3];
    let decoded = decode(&Records, &input(&payload)).unwrap();
    let root = decoded.root();
    assert_eq!(root.value, Value::Opaque(payload.to_vec()));
    assert_eq!(root.source, Source::Signed(Extent::Own(Span::new(0, 4))));
    assert_eq!(decoded.node_count(), 2);
    assert!(decoded.actions().is_empty());
    assert!(decoded.check_rendered(2).is_ok());
}

#[test]
fn an_unknown_format_skips_the_checks() {
    let decoded = decode(&Tampered(|_: &mut Walked| unreachable!()), &input(&[UNKNOWN_FORMAT]));
    assert!(decoded.is_ok());
}

#[test]
fn an_empty_unknown_payload_is_still_a_tree() {
    struct NothingKnown;
    impl Decoder for NothingKnown {
        fn walk(&self, _: &Input<'_>) -> Result<Option<Walked>, Reject> {
            Ok(None)
        }
    }
    let decoded = decode(&NothingKnown, &input(&[])).unwrap();
    assert_eq!(decoded.root().value, Value::Opaque(Vec::new()));
}

#[test]
fn known_outer_fields_keep_their_names_around_an_opaque_rest() {
    let decoded = decode(&Records, &input(&PARTIAL)).unwrap();
    let visits = decoded.visits();
    assert_eq!(visits[2].node.name, "amount");
    assert_eq!(visits[3].node.value, Value::Opaque(vec![0xaa, 0xbb]));
    assert_eq!(visits[3].span, Some(Span::new(6, 2)));
}

#[test]
fn a_partial_decode_still_has_to_cover_every_byte() {
    let decoded = decode(
        &Tampered(|walked: &mut Walked| {
            walked.root.children.pop();
        }),
        &input(&PARTIAL),
    );
    assert_eq!(decoded.unwrap_err().reason, Reject::Gap { offset: 4 });
}

// C1

#[test]
fn trailing_bytes_are_a_gap() {
    let reason = reason(|walked| {
        walked.root.children.pop();
    });
    assert_eq!(reason, Reject::Gap { offset: 4 });
}

#[test]
fn a_skipped_tag_is_a_gap() {
    let reason = reason(|walked| walked.root.children[0].framing.clear());
    assert_eq!(reason, Reject::Gap { offset: 0 });
}

#[test]
fn claiming_the_same_byte_twice_is_an_overlap() {
    let reason = reason(|walked| walked.root.children[1].framing.push(Span::new(2, 1)));
    assert_eq!(reason, Reject::Overlap { offset: 2 });
}

#[test]
fn a_range_past_the_payload_is_refused() {
    let reason = reason(|walked| walked.root.children[1].source = signed(Span::new(6, 4)));
    assert_eq!(reason, Reject::OutOfBounds);
}

#[test]
fn a_range_overflowing_u32_is_refused_not_wrapped() {
    let reason = reason(|walked| walked.root.children[1].source = signed(Span::new(u32::MAX, u32::MAX)));
    assert_eq!(reason, Reject::OutOfBounds);
}

#[test]
fn a_context_leaf_counts_like_a_signed_leaf() {
    let decoded = decode(
        &Tampered(|walked: &mut Walked| {
            walked.root.children[1].source = Source::Context(Extent::Own(Span::new(6, 3)));
        }),
        &input(&PAYLOAD),
    );
    assert!(decoded.is_ok());
}

#[test]
fn a_derived_node_cannot_cover_bytes() {
    let reason = reason(|walked| walked.root.children[1].source = Source::Derived);
    assert_eq!(reason, Reject::BadExtent);
}

#[test]
fn a_leaf_cannot_be_a_container() {
    let reason = reason(|walked| walked.root.children[0].source = Source::Signed(Extent::Container));
    assert_eq!(reason, Reject::BadExtent);
}

#[test]
fn inheriting_needs_an_owner() {
    let reason = reason(|walked| walked.root.children[0].source = Source::Signed(Extent::Inherited));
    assert_eq!(reason, Reject::BadExtent);
}

#[test]
fn an_empty_payload_needs_no_ranges() {
    assert_eq!(decode(&Records, &input(&[])).unwrap().node_count(), 2);
}

#[test]
fn zero_length_fields_cover_nothing() {
    assert!(decode(&Records, &input(&[1, 0])).is_ok());
}

#[test]
fn deep_trees_are_refused_before_any_walk() {
    let reason = reason(|walked| {
        let mut chain = Node::new("leaf", Value::Struct, Source::Derived);
        for _ in 0..=MAX_DEPTH {
            chain = Node::new("group", Value::Struct, Source::Derived).children(vec![chain]);
        }
        walked.root.children.push(chain);
    });
    assert_eq!(reason, Reject::TooDeep);
}

// 官方编解码器

const SEALED: [u8; 4] = [3, 2, 7, 8];

#[test]
fn fields_inside_an_official_codec_inherit_the_range() {
    let decoded = decode(&Records, &input(&SEALED)).unwrap();
    let spans: Vec<_> = decoded.visits().iter().map(|v| (v.node.name.clone(), v.span)).collect();
    let content = Some(Span::new(2, 2));
    assert_eq!(spans[0], ("records".to_string(), Some(Span::new(0, 4))));
    assert_eq!(spans[2], ("sealed".to_string(), content));
    assert_eq!(spans[3], ("first".to_string(), content));
    assert_eq!(spans[4], ("second".to_string(), content));
}

#[test]
fn a_field_inside_an_official_codec_cannot_claim_its_own_range() {
    let decoded = decode(
        &Tampered(|walked: &mut Walked| {
            walked.root.children[0].children[0].source = signed(Span::new(2, 1));
        }),
        &input(&SEALED),
    );
    assert_eq!(decoded.unwrap_err().reason, Reject::BadExtent);
}

// C2

#[test]
fn a_full_partition_still_needs_the_round_trip() {
    let reason = reason(|walked| walked.round_trips[0].rebuilt[0] ^= 1);
    assert_eq!(reason, Reject::RoundTrip);
}

#[test]
fn every_round_trip_counts() {
    let reason = reason(|walked| {
        walked.round_trips.push(RoundTrip {
            original: vec![1],
            rebuilt: vec![2],
        });
    });
    assert_eq!(reason, Reject::RoundTrip);
}

#[test]
fn a_decoder_cannot_skip_the_round_trip() {
    let reason = reason(|walked| walked.round_trips.clear());
    assert_eq!(reason, Reject::NoRoundTrip);
}

// 信封

#[test]
fn network_and_paths_enter_the_tree_as_derived() {
    let paths = ["m/44'/60'/0'/0/0".to_string()];
    let decoded = decode(
        &Records,
        &Input {
            paths: &paths,
            ..input(&PAYLOAD)
        },
    )
    .unwrap();
    let visits = decoded.visits();
    assert_eq!(visits[1].node.value, Value::Text("ethereum".to_string()));
    assert_eq!(visits[2].node.value, Value::Text(paths[0].clone()));
    assert!(
        visits[1..3]
            .iter()
            .all(|v| v.node.source == Source::Derived && v.span.is_none())
    );
}

// 遍历

#[test]
fn visits_are_preorder_with_parents_and_depths() {
    let decoded = decode(&Records, &input(&SEALED)).unwrap();
    let shape: Vec<_> = decoded.visits().iter().map(|v| (v.id, v.parent, v.depth)).collect();
    assert_eq!(
        shape,
        [
            (0, None, 0),
            (1, Some(0), 1),
            (2, Some(0), 1),
            (3, Some(2), 2),
            (4, Some(2), 2),
        ]
    );
}

#[test]
fn actions_carry_the_schema_name_and_the_node_id() {
    let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
    let actions = decoded.actions();
    assert_eq!(actions.len(), 1);
    assert_eq!((actions[0].0, actions[0].2), (2, "transfer"));
}

// C4

#[test]
fn rendering_a_different_number_of_rows_fails() {
    let decoded = decode(&Records, &input(&PAYLOAD)).unwrap();
    assert!(decoded.check_rendered(4).is_ok());
    let failure = decoded.check_rendered(3).unwrap_err();
    assert_eq!(failure.reason, Reject::Rendered);
    assert_eq!(failure.digest, decoded.payload_digest());
    assert!(decoded.check_rendered(5).is_err());
}

#[test]
fn a_network_this_build_does_not_know_keeps_its_raw_name() {
    let decoded = decode(
        &Records,
        &Input {
            chain: Chain::Tron,
            mode: None,
            network: "local\u{202E}debug",
            ..input(&PAYLOAD)
        },
    )
    .unwrap();
    assert_eq!(decoded.network(), "local\u{202E}debug");
    assert_eq!((decoded.chain(), decoded.mode()), (Chain::Tron, None));
    assert_eq!(display(&decoded.visits()[1].node.value), "local\\u{202e}debug");
}

// 模型

#[test]
fn each_source_has_the_table_properties() {
    let cases = [
        (signed(Span::new(0, 1)), SourceKind::Signed, true, true),
        (Source::Context(Extent::Container), SourceKind::Context, true, true),
        (Source::Derived, SourceKind::Derived, false, true),
        (Source::Manifest, SourceKind::Manifest, false, true),
        (Source::Descriptor, SourceKind::Descriptor, false, false),
        (Source::Hint, SourceKind::Hint, false, false),
    ];
    for (source, kind, ranged, evidence) in cases {
        assert_eq!(source.kind(), kind);
        assert_eq!(source.extent().is_some(), ranged);
        assert_eq!(kind.is_evidence(), evidence);
    }
}

#[test]
fn groups_have_no_value_string() {
    assert_eq!(Value::Struct.kind(), Kind::Struct);
    assert_eq!(Value::List.kind(), Kind::List);
    assert!(Value::Struct.is_group() && Value::List.is_group());
    assert!(!Value::Opaque(vec![1]).is_group());
    assert_eq!(display(&Value::List), "");
}

//! Vote 没有官方 IDL,核心库按 `solana-vote-interface` 的官方枚举解码.这里用同一个 crate 的构建函数生成指令,
//! 走完整的 Solana 消息遍历,核对种类名,账户数,字段和第一屏.

use bitone_core::chains::solana::decode::{Instruction, Message, SolanaWalker, Version};
use bitone_core::decode::{Coin, Decoded, Input, Node, Value, decode, display};
use bitone_core::{Chain, Mode};
use solana_hash::Hash;
use solana_instruction::Instruction as VoteCall;
use solana_pubkey::Pubkey;
use solana_vote_interface::instruction::{
    CommissionKind, CreateVoteAccountConfig, authorize, authorize_checked, authorize_checked_with_seed,
    authorize_with_seed, create_account_with_config, create_account_with_config_v2, deposit_delegator_rewards,
    tower_sync, update_commission, update_commission_bps, update_commission_collector, update_validator_identity, vote,
    withdraw,
};
use solana_vote_interface::state::{TowerSync, Vote, VoteAuthorize, VoteInit, VoteInitV2};

const VOTE_PROGRAM: &str = "Vote111111111111111111111111111111111111111";
const PAYER: [u8; 32] = [0x77; 32];
const WALKER: SolanaWalker = SolanaWalker {
    coin: Some(Coin {
        unit: "SOL",
        decimals: 9,
    }),
    hint: None,
};

fn key(seed: u8) -> Pubkey {
    Pubkey::new_from_array([seed; 32])
}

/// 账户表按第一次出现的顺序,签名和可写标记写在表头,和节点的排列一致.
fn message(instruction: &VoteCall) -> Message {
    let metas = &instruction.accounts;
    let order = |signer: bool, writable: bool| {
        metas
            .iter()
            .filter(move |meta| meta.is_signer == signer && meta.is_writable == writable)
            .map(|meta| meta.pubkey.to_bytes())
    };
    let mut signers = vec![PAYER];
    signers.extend(order(true, true));
    let writable_signers = signers.len();
    signers.extend(order(true, false));
    let required = signers.len();
    let mut keys = signers;
    keys.extend(order(false, true));
    keys.extend(order(false, false));
    let mut unique: Vec<[u8; 32]> = Vec::new();
    keys.into_iter().for_each(|item| {
        if !unique.contains(&item) {
            unique.push(item);
        }
    });
    let program: [u8; 32] = instruction.program_id.to_bytes();
    unique.push(program);
    let index = |item: &[u8; 32]| unique.iter().position(|known| known == item).unwrap() as u8;
    let compiled = Instruction {
        program: index(&program),
        accounts: metas.iter().map(|meta| index(&meta.pubkey.to_bytes())).collect(),
        data: instruction.data.clone(),
    };
    let readonly_unsigned = 1 + order(false, false)
        .map(|item| unique.iter().position(|known| *known == item))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    Message {
        version: Version::Legacy,
        required_signatures: required as u8,
        readonly_signed: (required - writable_signers) as u8,
        readonly_unsigned: readonly_unsigned as u8,
        keys: unique,
        blockhash: [9; 32],
        instructions: vec![compiled],
        lookups: Vec::new(),
    }
}

fn run(instruction: &VoteCall) -> Decoded {
    let payload = message(instruction).encode();
    decode(
        &WALKER,
        &Input {
            chain: Chain::Solana,
            mode: Some(Mode::Mainnet),
            network: "solana",
            kind: 0,
            paths: &[],
            payload: &payload,
            signer: None,
        },
    )
    .unwrap()
}

/// 名字是 `path` 的节点,`path` 从指令数据的分支节点算起.
fn shown(decoded: &Decoded, path: &[&str]) -> String {
    let visits = decoded.visits();
    let data = visits
        .iter()
        .find(|visit| visit.node.name == "data" && visit.node.children.len() == 1)
        .expect("有指令数据");
    let mut node: &Node = &data.node.children[0];
    for name in path {
        node = node
            .children
            .iter()
            .find(|child| child.name == *name)
            .unwrap_or_else(|| panic!("{path:?}"));
    }
    display(&node.value)
}

fn check(name: &str, instruction: &VoteCall) -> Decoded {
    assert_eq!(instruction.program_id.to_string(), VOTE_PROGRAM);
    let decoded = run(instruction);
    let kinds: Vec<&str> = decoded.actions().iter().map(|(_, _, kind)| *kind).collect();
    assert_eq!(kinds, [["vote.", name].concat()], "{name}");
    let visits = decoded.visits();
    let opaque = visits.iter().find(|visit| matches!(visit.node.value, Value::Opaque(_)));
    assert!(opaque.is_none(), "{name} 有 opaque 节点");
    let accounts = visits
        .iter()
        .find(|visit| visit.node.name == "accounts")
        .expect("有账户列表");
    assert_eq!(accounts.node.children.len(), instruction.accounts.len(), "{name}");
    decoded
}

fn address(seed: u8) -> String {
    key(seed).to_string()
}

fn bls() -> VoteAuthorize {
    VoteAuthorize::VoterWithBLS(solana_vote_interface::state::VoterWithBLSArgs {
        bls_pubkey: [5; 48],
        bls_proof_of_possession: [6; 96],
    })
}

#[test]
fn authorize_matches_the_interface() {
    for (kind, label) in [
        (VoteAuthorize::Voter, "Voter"),
        (VoteAuthorize::Withdrawer, "Withdrawer"),
    ] {
        let decoded = check("Authorize", &authorize(&key(1), &key(2), &key(3), kind));
        assert_eq!(shown(&decoded, &["0"]), address(3));
        assert_eq!(shown(&decoded, &["1"]), label);
    }
    let decoded = check("Authorize", &authorize(&key(1), &key(2), &key(3), bls()));
    assert_eq!(shown(&decoded, &["1", "0", "bls_pubkey"]), "05".repeat(48));
    assert_eq!(shown(&decoded, &["1", "0", "bls_proof_of_possession"]), "06".repeat(96));
}

#[test]
fn authorize_checked_matches_the_interface() {
    let decoded = check(
        "AuthorizeChecked",
        &authorize_checked(&key(1), &key(2), &key(3), VoteAuthorize::Withdrawer),
    );
    assert_eq!(shown(&decoded, &["0"]), "Withdrawer");
}

#[test]
fn seeded_authorizations_match_the_interface() {
    let seeded = authorize_with_seed(&key(1), &key(2), &key(3), "seed", &key(4), VoteAuthorize::Voter);
    let decoded = check("AuthorizeWithSeed", &seeded);
    assert_eq!(shown(&decoded, &["0", "authorization_type"]), "Voter");
    assert_eq!(
        shown(&decoded, &["0", "current_authority_derived_key_owner"]),
        address(3)
    );
    assert_eq!(shown(&decoded, &["0", "current_authority_derived_key_seed"]), "seed");
    assert_eq!(shown(&decoded, &["0", "new_authority"]), address(4));
    let checked = authorize_checked_with_seed(&key(1), &key(2), &key(3), "seed", &key(4), VoteAuthorize::Withdrawer);
    let decoded = check("AuthorizeCheckedWithSeed", &checked);
    assert_eq!(shown(&decoded, &["0", "current_authority_derived_key_seed"]), "seed");
}

#[test]
fn the_identity_and_the_commission_match_the_interface() {
    check(
        "UpdateValidatorIdentity",
        &update_validator_identity(&key(1), &key(2), &key(3)),
    );
    let decoded = check("UpdateCommission", &update_commission(&key(1), &key(2), 42));
    assert_eq!(shown(&decoded, &["0"]), "42");
    let decoded = check(
        "UpdateCommissionCollector",
        &update_commission_collector(&key(1), &key(2), &key(3), CommissionKind::BlockRevenue),
    );
    assert_eq!(shown(&decoded, &["0"]), "BlockRevenue");
    let decoded = check(
        "UpdateCommissionBps",
        &update_commission_bps(&key(1), &key(2), CommissionKind::InflationRewards, 750),
    );
    assert_eq!(shown(&decoded, &["commission_bps"]), "750");
    assert_eq!(shown(&decoded, &["kind"]), "InflationRewards");
}

#[test]
fn withdrawals_and_deposits_match_the_interface() {
    let decoded = check("Withdraw", &withdraw(&key(1), &key(2), 1_500_000_000, &key(3)));
    assert_eq!(shown(&decoded, &["0"]), "1500000000");
    let decoded = check(
        "DepositDelegatorRewards",
        &deposit_delegator_rewards(&key(1), &key(2), 2_000_000_000),
    );
    assert_eq!(shown(&decoded, &["deposit"]), "2000000000");
}

#[test]
fn initialize_account_matches_the_interface() {
    let init = VoteInit {
        node_pubkey: key(4),
        authorized_voter: key(5),
        authorized_withdrawer: key(6),
        commission: 9,
    };
    let instructions = create_account_with_config(&key(1), &key(2), &init, 1, CreateVoteAccountConfig::default());
    let decoded = check("InitializeAccount", instructions.last().unwrap());
    assert_eq!(shown(&decoded, &["0", "node_pubkey"]), address(4));
    assert_eq!(shown(&decoded, &["0", "authorized_withdrawer"]), address(6));
    assert_eq!(shown(&decoded, &["0", "commission"]), "9");
}

#[test]
fn initialize_account_v2_matches_the_interface() {
    let init = VoteInitV2 {
        node_pubkey: key(4),
        authorized_voter: key(5),
        authorized_voter_bls_pubkey: [7; 48],
        authorized_voter_bls_proof_of_possession: [8; 96],
        authorized_withdrawer: key(6),
        inflation_rewards_commission_bps: 100,
        block_revenue_commission_bps: 200,
    };
    let instructions = create_account_with_config_v2(
        &key(1),
        &key(2),
        &init,
        &key(9),
        &key(10),
        1,
        CreateVoteAccountConfig::default(),
    );
    let initialize = instructions
        .iter()
        .find(|instruction| instruction.program_id.to_string() == VOTE_PROGRAM)
        .unwrap();
    let decoded = check("InitializeAccountV2", initialize);
    assert_eq!(shown(&decoded, &["0", "authorized_voter_bls_pubkey"]), "07".repeat(48));
    assert_eq!(shown(&decoded, &["0", "inflation_rewards_commission_bps"]), "100");
    assert_eq!(shown(&decoded, &["0", "block_revenue_commission_bps"]), "200");
}

/// 共识投票的紧凑编码按官方类型读出,不是 `opaque`.
#[test]
fn consensus_votes_decode_by_the_official_types() {
    let decoded = check("Vote", &vote(&key(1), &key(2), Vote::new(vec![1], Hash::default())));
    assert_eq!(shown(&decoded, &["0", "slots", "item"]), "1");
    let decoded = check(
        "TowerSync",
        &tower_sync(
            &key(1),
            &key(2),
            TowerSync::new_from_slot(5, Hash::new_from_array([4; 32])),
        ),
    );
    assert_eq!(shown(&decoded, &["0", "hash"]), "04".repeat(32));
}

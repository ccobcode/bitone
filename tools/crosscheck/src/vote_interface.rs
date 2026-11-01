//! schemas/solana/vote.json 是手写的,这里和 `solana-vote-interface` 比较.
//! 指令由官方的构建函数生成,字节和账户标记都来自 interface crate,见 docs/10-tx-decoding.md 第 5.3 节.

use bitone_core::decode::solana::{Instruction, Message, SolanaWalker, Version};
use bitone_core::decode::{Coin, Decoded, Input, Node, Value, decode, display};
use bitone_core::{Chain, Mode};
use serde_json::Value as Json;
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

const SCHEMA: &str = include_str!("../../../schemas/solana/vote.json");
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

fn shown(decoded: &Decoded, name: &str) -> Vec<String> {
    let visits = decoded.visits();
    let rows = visits.iter().filter(|visit| visit.node.name == name);
    rows.map(|visit| display(&visit.node.value)).collect()
}

fn accounts_of(decoded: &Decoded) -> Vec<&Node> {
    let visits = decoded.visits();
    let list = visits
        .iter()
        .find(|visit| visit.node.name == "accounts")
        .expect("有账户列表");
    list.node.children.iter().collect()
}

/// IDL 里这条指令的账户标记,和官方构建函数给出的逐个相同.
fn idl_accounts(name: &str) -> Vec<(String, bool, bool)> {
    let root: Json = serde_json::from_str(SCHEMA).unwrap();
    let instructions = root["program"]["instructions"].as_array().unwrap();
    let found = instructions.iter().find(|item| item["name"] == name).unwrap();
    let accounts = found["accounts"].as_array().unwrap();
    accounts
        .iter()
        .map(|account| {
            (
                account["name"].as_str().unwrap().to_string(),
                account["isSigner"] == Json::Bool(true),
                account["isWritable"] == Json::Bool(true),
            )
        })
        .collect()
}

fn check(name: &str, instruction: &VoteCall) -> Decoded {
    let decoded = run(instruction);
    assert_eq!(shown(&decoded, "instruction_name"), [name]);
    let kinds: Vec<&str> = decoded.actions().iter().map(|(_, _, kind)| *kind).collect();
    assert_eq!(kinds, [["vote.", name].concat()], "{name}");
    let visits = decoded.visits();
    let opaque = visits.iter().find(|visit| matches!(visit.node.value, Value::Opaque(_)));
    assert!(opaque.is_none(), "{name} 有 opaque 节点");
    let expected = idl_accounts(name);
    let rows = accounts_of(&decoded);
    assert_eq!(rows.len(), expected.len(), "{name}");
    assert_eq!(instruction.accounts.len(), expected.len(), "{name}");
    for ((row, meta), (account, signer, writable)) in rows.iter().zip(&instruction.accounts).zip(&expected) {
        assert_eq!(&row.name, account, "{name}");
        assert_eq!(meta.is_signer, *signer, "{name} 的 {account}");
        assert_eq!(meta.is_writable, *writable, "{name} 的 {account}");
    }
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
        (VoteAuthorize::Voter, "voter"),
        (VoteAuthorize::Withdrawer, "withdrawer"),
    ] {
        let decoded = check("authorize", &authorize(&key(1), &key(2), &key(3), kind));
        assert_eq!(shown(&decoded, "newAuthority"), [address(3)]);
        assert_eq!(shown(&decoded, "authorizationType"), [label]);
    }
    let decoded = check("authorize", &authorize(&key(1), &key(2), &key(3), bls()));
    assert_eq!(shown(&decoded, "blsPubkey"), ["05".repeat(48)]);
    assert_eq!(shown(&decoded, "blsProofOfPossession"), ["06".repeat(96)]);
}

#[test]
fn authorize_checked_matches_the_interface() {
    let decoded = check(
        "authorizeChecked",
        &authorize_checked(&key(1), &key(2), &key(3), VoteAuthorize::Withdrawer),
    );
    assert_eq!(shown(&decoded, "authorizationType"), ["withdrawer"]);
}

#[test]
fn seeded_authorizations_match_the_interface() {
    let seeded = authorize_with_seed(&key(1), &key(2), &key(3), "seed", &key(4), VoteAuthorize::Voter);
    let decoded = check("authorizeWithSeed", &seeded);
    assert_eq!(shown(&decoded, "authorizationType"), ["voter"]);
    assert_eq!(shown(&decoded, "currentAuthorityDerivedKeyOwner"), [address(3)]);
    assert_eq!(shown(&decoded, "currentAuthorityDerivedKeySeed"), ["seed"]);
    assert_eq!(shown(&decoded, "newAuthority"), [address(4)]);
    let checked = authorize_checked_with_seed(&key(1), &key(2), &key(3), "seed", &key(4), VoteAuthorize::Withdrawer);
    let decoded = check("authorizeCheckedWithSeed", &checked);
    assert_eq!(shown(&decoded, "currentAuthorityDerivedKeySeed"), ["seed"]);
}

#[test]
fn the_identity_and_the_commission_match_the_interface() {
    check(
        "updateValidatorIdentity",
        &update_validator_identity(&key(1), &key(2), &key(3)),
    );
    let decoded = check("updateCommission", &update_commission(&key(1), &key(2), 42));
    assert_eq!(shown(&decoded, "commission"), ["42"]);
    let decoded = check(
        "updateCommissionCollector",
        &update_commission_collector(&key(1), &key(2), &key(3), CommissionKind::BlockRevenue),
    );
    assert_eq!(shown(&decoded, "commissionKind"), ["blockRevenue"]);
    let decoded = check(
        "updateCommissionBps",
        &update_commission_bps(&key(1), &key(2), CommissionKind::InflationRewards, 750),
    );
    assert_eq!(shown(&decoded, "commissionBps"), ["750"]);
    assert_eq!(shown(&decoded, "commissionKind"), ["inflationRewards"]);
}

#[test]
fn withdrawals_and_deposits_match_the_interface() {
    let decoded = check("withdraw", &withdraw(&key(1), &key(2), 1_500_000_000, &key(3)));
    assert_eq!(shown(&decoded, "lamports"), ["1.5 SOL"]);
    let decoded = check(
        "depositDelegatorRewards",
        &deposit_delegator_rewards(&key(1), &key(2), 2_000_000_000),
    );
    assert_eq!(shown(&decoded, "deposit"), ["2 SOL"]);
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
    let decoded = check("initializeAccount", instructions.last().unwrap());
    assert_eq!(shown(&decoded, "nodePubkey"), [address(4)]);
    assert_eq!(shown(&decoded, "authorizedWithdrawer"), [address(6)]);
    assert_eq!(shown(&decoded, "commission"), ["9"]);
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
    let decoded = check("initializeAccountV2", initialize);
    assert_eq!(shown(&decoded, "authorizedVoterBlsPubkey"), ["07".repeat(48)]);
    assert_eq!(
        shown(&decoded, "authorizedVoterBlsProofOfPossession"),
        ["08".repeat(96)]
    );
    assert_eq!(shown(&decoded, "inflationRewardsCommissionBps"), ["100"]);
    assert_eq!(shown(&decoded, "blockRevenueCommissionBps"), ["200"]);
}

/// 8 条共识投票指令不进 IDL,命中时按 `opaque` 显示.
#[test]
fn consensus_votes_are_outside_the_idl() {
    let cases = [
        vote(&key(1), &key(2), Vote::new(vec![1], Hash::default())),
        tower_sync(&key(1), &key(2), TowerSync::new_from_slot(5, Hash::default())),
    ];
    for instruction in cases {
        assert_eq!(instruction.program_id.to_string(), VOTE_PROGRAM);
        let decoded = run(&instruction);
        let kinds: Vec<&str> = decoded.actions().iter().map(|(_, _, kind)| *kind).collect();
        assert_eq!(kinds, ["instruction-unknown"]);
    }
}

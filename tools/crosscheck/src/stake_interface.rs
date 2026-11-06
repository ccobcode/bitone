//! `vectors/stake.json` 由官方的构建函数生成.旧布局是 `solana-stake-interface` 4.4.0,它是最后一个带系统变量的版本,
//! 现在的布局是 5.1.1.账户名来自调用时传入的角色,所以这份向量不依赖 schemas/solana/.
//! 文件和构建函数不一致时测试失败,`WRITE_STAKE_VECTORS=1 cargo test` 重写文件.

use crate::sha256;
use crate::stake_vectors::{Case, FILE, Vectors, load};
use bitcoin::hex::DisplayHex;
use bitone_core::chains::solana::decode::{Instruction, Message, Version};
use solana_pubkey::Pubkey;
use std::fs;
use std::path::Path;

const GENERATOR: &str = "tools/crosscheck/src/stake_interface.rs";
const LOCK: &str = "vectors.lock";
const WRITE: &str = "WRITE_STAKE_VECTORS";
/// 测试助记词在 `m/44'/501'/0'/0'` 的地址,签名测试用它当手续费支付者和权限账户.
const PAYER: &str = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";
const SEED: &str = "seed";
const LAMPORTS: u64 = 2_000_000_000;
const OLDER_BUILDER: &str = "solana-stake-interface 4.4.0";
const CURRENT_BUILDER: &str = "solana-stake-interface 5.1.1";
const SYSVARS: [(&str, &str); 4] = [
    ("clockSysvar", "SysvarC1ock11111111111111111111111111111111"),
    ("rentSysvar", "SysvarRent111111111111111111111111111111111"),
    ("stakeHistorySysvar", "SysvarStakeHistory1111111111111111111111111"),
    ("stakeConfig", "StakeConfig11111111111111111111111111111111"),
];

fn key(seed: u8) -> Pubkey {
    Pubkey::new_from_array([seed; 32])
}

fn payer() -> Pubkey {
    PAYER.parse().unwrap()
}

/// 两个版本的 `Instruction` 不是同一个类型,这里只留下字节.
struct Call {
    program: [u8; 32],
    accounts: Vec<([u8; 32], bool, bool)>,
    data: Vec<u8>,
}

macro_rules! call {
    ($instruction:expr) => {{
        let instruction = $instruction;
        Call {
            program: instruction.program_id.to_bytes(),
            accounts: instruction
                .accounts
                .iter()
                .map(|meta| (meta.pubkey.to_bytes(), meta.is_signer, meta.is_writable))
                .collect(),
            data: instruction.data.clone(),
        }
    }};
}

/// 手续费支付者在最前,账户表按签名,可写排序,程序在最后,和节点的排列一致.
fn message(call: &Call) -> Message {
    let mut metas = vec![(payer().to_bytes(), true, true)];
    for (key, signer, writable) in &call.accounts {
        match metas.iter_mut().find(|meta| meta.0 == *key) {
            Some(meta) => {
                meta.1 |= signer;
                meta.2 |= writable;
            }
            None => metas.push((*key, *signer, *writable)),
        }
    }
    metas.push((call.program, false, false));
    metas.sort_by_key(|meta| (!meta.1, !meta.2));
    let keys: Vec<[u8; 32]> = metas.iter().map(|meta| meta.0).collect();
    let index = |key: &[u8; 32]| keys.iter().position(|item| item == key).unwrap() as u8;
    let count = |signer: bool, writable: bool| {
        metas
            .iter()
            .filter(|meta| meta.1 == signer && meta.2 == writable)
            .count() as u8
    };
    Message {
        version: Version::Legacy,
        required_signatures: count(true, true) + count(true, false),
        readonly_signed: count(true, false),
        readonly_unsigned: count(false, false),
        instructions: vec![Instruction {
            program: index(&call.program),
            accounts: call.accounts.iter().map(|meta| index(&meta.0)).collect(),
            data: call.data.clone(),
        }],
        keys: keys.clone(),
        blockhash: [9; 32],
        lookups: Vec::new(),
    }
}

/// `roles` 是调用时传入的地址和它的角色.系统变量由构建函数自己填写.
fn case(instruction: &str, builder: &str, call: &Call, roles: &[(Pubkey, &str)], counterparty: Option<Pubkey>) -> Case {
    let named = |bytes: &[u8; 32]| {
        let address = Pubkey::new_from_array(*bytes).to_string();
        let role = roles
            .iter()
            .find(|(key, _)| key.to_bytes() == *bytes)
            .map(|(_, name)| *name);
        let sysvar = SYSVARS.iter().find(|(_, id)| *id == address).map(|(name, _)| *name);
        let name = role
            .or(sysvar)
            .unwrap_or_else(|| panic!("{instruction}:{address} 没有角色"));
        (name.to_string(), address)
    };
    Case {
        instruction: instruction.to_string(),
        layout: if builder == OLDER_BUILDER { "older" } else { "current" }.to_string(),
        builder: builder.to_string(),
        message: message(call).encode().to_lower_hex_string(),
        accounts: call.accounts.iter().map(|meta| named(&meta.0)).collect(),
        counterparty: counterparty.map(|key| key.to_string()),
    }
}

/// 两个版本的函数同名,类型各是各的,所以同一段调用展开两次.
macro_rules! cases {
    ($name:ident, $interface:ident, $builder:expr) => {
        fn $name() -> Vec<Case> {
            use $interface::instruction::{
                authorize, authorize_checked, authorize_checked_with_seed, authorize_with_seed, deactivate_stake,
                delegate_stake, initialize, initialize_checked, merge, withdraw,
            };
            use $interface::state::{Authorized, Lockup, StakeAuthorize};
            let (stake, authority, next, custodian) = (key(1), payer(), key(3), key(4));
            let (vote, recipient, source, owner) = (key(5), key(6), key(7), key(8));
            let kind = StakeAuthorize::Withdrawer;
            let keys = Authorized {
                staker: key(9),
                withdrawer: next,
            };
            let lockup = Lockup::default();
            let seed = SEED.to_string();
            let plain = [(stake, "stake"), (authority, "authority")];
            let with_lockup = [
                (stake, "stake"),
                (authority, "authority"),
                (custodian, "lockupAuthority"),
            ];
            let with_new = [(stake, "stake"), (authority, "authority"), (next, "newAuthority")];
            let with_both = [
                (stake, "stake"),
                (authority, "authority"),
                (next, "newAuthority"),
                (custodian, "lockupAuthority"),
            ];
            let seeded = [
                (stake, "stake"),
                (authority, "base"),
                (custodian, "lockupAuthority"),
            ];
            let seeded_new = [(stake, "stake"), (authority, "base"), (next, "newAuthority")];
            let seeded_both = [
                (stake, "stake"),
                (authority, "base"),
                (next, "newAuthority"),
                (custodian, "lockupAuthority"),
            ];
            let delegated = [(stake, "stake"), (vote, "vote"), (authority, "stakeAuthority")];
            let withdrawn = [
                (stake, "stake"),
                (recipient, "recipient"),
                (authority, "withdrawAuthority"),
                (custodian, "lockupAuthority"),
            ];
            let merged = [
                (stake, "destinationStake"),
                (source, "sourceStake"),
                (authority, "stakeAuthority"),
            ];
            let checked = [
                (stake, "stake"),
                (key(9), "stakeAuthority"),
                (next, "withdrawAuthority"),
            ];
            let builder = $builder;
            vec![
                case(
                    "initialize",
                    builder,
                    &call!(initialize(&stake, &keys, &lockup)),
                    &[(stake, "stake")],
                    Some(next),
                ),
                case(
                    "initializeChecked",
                    builder,
                    &call!(initialize_checked(&stake, &keys)),
                    &checked,
                    Some(next),
                ),
                case(
                    "authorize",
                    builder,
                    &call!(authorize(&stake, &authority, &next, kind, None)),
                    &plain,
                    Some(next),
                ),
                case(
                    "authorize",
                    builder,
                    &call!(authorize(&stake, &authority, &next, kind, Some(&custodian))),
                    &with_lockup,
                    Some(next),
                ),
                case(
                    "authorizeChecked",
                    builder,
                    &call!(authorize_checked(&stake, &authority, &next, kind, None)),
                    &with_new,
                    Some(next),
                ),
                case(
                    "authorizeChecked",
                    builder,
                    &call!(authorize_checked(
                        &stake,
                        &authority,
                        &next,
                        kind,
                        Some(&custodian)
                    )),
                    &with_both,
                    Some(next),
                ),
                case(
                    "authorizeWithSeed",
                    builder,
                    &call!(authorize_with_seed(
                        &stake,
                        &authority,
                        seed.clone(),
                        &owner,
                        &next,
                        kind,
                        Some(&custodian)
                    )),
                    &seeded,
                    Some(next),
                ),
                case(
                    "authorizeCheckedWithSeed",
                    builder,
                    &call!(authorize_checked_with_seed(
                        &stake,
                        &authority,
                        seed.clone(),
                        &owner,
                        &next,
                        kind,
                        None
                    )),
                    &seeded_new,
                    Some(next),
                ),
                case(
                    "authorizeCheckedWithSeed",
                    builder,
                    &call!(authorize_checked_with_seed(
                        &stake,
                        &authority,
                        seed.clone(),
                        &owner,
                        &next,
                        kind,
                        Some(&custodian)
                    )),
                    &seeded_both,
                    Some(next),
                ),
                case(
                    "delegateStake",
                    builder,
                    &call!(delegate_stake(&stake, &authority, &vote)),
                    &delegated,
                    Some(vote),
                ),
                case(
                    "withdraw",
                    builder,
                    &call!(withdraw(
                        &stake,
                        &authority,
                        &recipient,
                        LAMPORTS,
                        Some(&custodian)
                    )),
                    &withdrawn,
                    Some(recipient),
                ),
                case(
                    "deactivate",
                    builder,
                    &call!(deactivate_stake(&stake, &authority)),
                    &[(stake, "stake"), (authority, "stakeAuthority")],
                    None,
                ),
                case(
                    "merge",
                    builder,
                    &call!(merge(&stake, &source, &authority).remove(0)),
                    &merged,
                    Some(stake),
                ),
            ]
        }
    };
}

cases!(older_cases, stake_older, OLDER_BUILDER);
cases!(current_cases, stake_current, CURRENT_BUILDER);

fn vectors_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors")
}

fn expected() -> Vectors {
    let mut cases = older_cases();
    cases.extend(current_cases());
    Vectors {
        generator: GENERATOR.to_string(),
        cases,
    }
}

fn write(vectors: &Vectors) {
    let dir = vectors_dir();
    let text = serde_json::to_string_pretty(vectors).unwrap() + "\n";
    fs::write(dir.join(FILE), &text).unwrap();
    let lock = fs::read_to_string(dir.join(LOCK)).unwrap();
    let suffix = [" ", FILE].concat();
    let mut lines: Vec<String> = lock
        .lines()
        .filter(|line| !line.ends_with(&suffix))
        .map(str::to_string)
        .collect();
    lines.push([sha256(text.as_bytes()).as_str(), suffix.as_str()].concat());
    fs::write(dir.join(LOCK), lines.join("\n") + "\n").unwrap();
}

#[test]
fn the_vectors_come_from_the_official_builders() {
    let vectors = expected();
    if std::env::var_os(WRITE).is_some() {
        write(&vectors);
    }
    assert_eq!(load(&vectors_dir()).unwrap(), vectors);
}

/// 两个版本的差别只有系统变量,这正是解码器要分辨的.
#[test]
fn the_older_layout_only_adds_sysvars() {
    let is_sysvar = |name: &str| SYSVARS.iter().any(|(sysvar, _)| *sysvar == name);
    for (older, current) in older_cases().iter().zip(current_cases().iter()) {
        let kept: Vec<_> = older.accounts.iter().filter(|(name, _)| !is_sysvar(name)).collect();
        // `initialize` 的两个权限在数据里,账户上没有差别.
        assert_eq!(
            kept,
            current.accounts.iter().collect::<Vec<_>>(),
            "{}",
            older.instruction
        );
        assert!(older.accounts.len() > current.accounts.len(), "{}", older.instruction);
    }
}

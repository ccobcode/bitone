//! 本地节点验证用。走完构建、核对、签名、核验，标准输出打印可以广播的交易和哈希，
//! 标准错误打印核对结果。见 scripts/localnet-solana.sh 和 scripts/localnet-tron.sh。
//! 用的是公开的测试助记词，只能用在本地节点上。

use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitone_core::address::{parse_solana_address, solana_address, tron_address};
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::{Asset, Manifest};
use bitone_core::protocol::VERSION;
use bitone_core::request::{KIND_TRANSACTION, SignRequest};
use bitone_core::tx::{self, HighestChange, solana, tron};
use bitone_core::{Chain, DerivationPath, Network};
use bitone_signer::keys::Wallet;
use sha2::{Digest, Sha256};

const SOLANA: Network = Network::SolanaDevnet;
const SOLANA_PATH: &str = "m/44'/1'/0'/0'";
const TRON: Network = Network::TronNile;
const TRON_PATH: &str = "m/44'/1'/0'/0/0";

fn wallet() -> (Wallet, PublicKeys) {
    let wallet = Wallet::new(&[0u8; 16], "").expect("测试助记词有效");
    let keys = PublicKeys::decode(&wallet.public_keys(false).expect("派生成功")).expect("公钥有效");
    (wallet, keys)
}

fn path(text: &str) -> DerivationPath {
    DerivationPath::parse(text).expect("路径有效")
}

fn solana_payer() -> String {
    let (_, keys) = wallet();
    solana_address(&keys.ed25519(&path(SOLANA_PATH)).expect("有这个账户")).expect("公钥有效")
}

fn tron_owner() -> String {
    let (_, keys) = wallet();
    tron_address(&keys.secp256k1(&path(TRON_PATH)).expect("有这个账户")).expect("公钥有效")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex_vec(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("十六进制"))
        .collect()
}

fn unhex<const N: usize>(text: &str) -> [u8; N] {
    unhex_vec(text).try_into().expect("长度正确")
}

fn number<T: std::str::FromStr>(text: &str) -> T {
    text.parse().ok().expect("数字")
}

/// 把本地合约加进清单，这样核对结果和主网上的代币一样。
fn manifest_with(network: Network, contract: &str, decimals: u8, standard: &str) -> Manifest {
    let mut manifest = Manifest::builtin().clone();
    manifest.assets.push(Asset {
        network: network.id().to_string(),
        contract: contract.to_string(),
        symbol: "TEST".to_string(),
        name: "Local Test Token".to_string(),
        decimals,
        icon: "test".to_string(),
        standard: standard.to_string(),
        min_fraction: 2,
    });
    manifest
}

fn submit(network: Network, signing_path: &str, payload: Vec<u8>, hint: Option<&str>, manifest: &Manifest) {
    let (wallet, keys) = wallet();
    let request = SignRequest {
        version: VERSION,
        id: [1; 16],
        fingerprint: keys.fingerprint,
        network: network.id().to_string(),
        kind: KIND_TRANSACTION,
        paths: vec![signing_path.to_string()],
        payload,
        hint: hint.map(str::to_string),
    };
    let (response, review) = wallet
        .sign(&request, &keys, manifest, HighestChange::default())
        .expect("核对通过");
    let derivation = path(signing_path);
    let signer = match network.chain() {
        Chain::Tron => keys.secp256k1(&derivation).expect("有这个账户").to_vec(),
        _ => keys.ed25519(&derivation).expect("有这个账户").to_vec(),
    };
    let verified = tx::verify(&request, &response, &signer).expect("签名有效");
    let amount = review
        .amount
        .map(|a| format!("{} {}", a.text, a.symbol.unwrap_or_default()));
    eprintln!(
        "{} {:?} {} {}",
        review.title,
        review.confirm,
        amount.unwrap_or_default(),
        review.counterparty.unwrap_or_default()
    );
    println!("{} {}", hex(&verified.raw), verified.hash);
}

fn solana_params<'a>(from: &'a str, to: &'a str, amount: u64, nonce_value: [u8; 32]) -> solana::TransferParams<'a> {
    solana::TransferParams {
        network: SOLANA,
        from,
        to,
        amount,
        mint: None,
        decimals: 0,
        nonce_value,
        priority_micro_lamports: 1000,
        compute_units: 100_000,
        create_recipient_ata: false,
    }
}

fn tron_params<'a>(from: &'a str, to: &'a str, amount: &'a str, block: &[&str]) -> tron::TransferParams<'a> {
    tron::TransferParams {
        network: TRON,
        from,
        to,
        amount,
        contract: None,
        ref_block_bytes: unhex(block[0]),
        ref_block_hash: unhex(block[1]),
        expiration_ms: number(block[2]),
        timestamp_ms: number(block[3]),
        fee_limit: 0,
    }
}

/// 用节点自带的测试账户给本钱包打款。这笔不经过核对，只是准备数据。
fn tron_sign_with_key(key: &str, raw_data: &str) {
    let key = SecretKey::from_slice(&unhex::<32>(key)).expect("私钥有效");
    let raw_data: Vec<u8> = unhex_vec(raw_data);
    let digest: [u8; 32] = Sha256::digest(&raw_data).into();
    let (recovery, compact) = Secp256k1::new()
        .sign_ecdsa_recoverable(&Message::from_digest(digest), &key)
        .serialize_compact();
    println!("{}", hex(&transaction(&raw_data, &compact, recovery.to_i32() as u8)));
}

fn transaction(raw_data: &[u8], compact: &[u8], recovery: u8) -> Vec<u8> {
    let mut out = vec![0x0a];
    let mut len = raw_data.len();
    while len >= 0x80 {
        out.push(len as u8 | 0x80);
        len >>= 7;
    }
    out.push(len as u8);
    out.extend_from_slice(raw_data);
    out.extend_from_slice(&[0x12, 65]);
    out.extend_from_slice(compact);
    out.push(recovery);
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let builtin = Manifest::builtin();
    match args.as_slice() {
        ["solana-address"] => println!("{}", solana_payer()),
        ["tron-address"] => println!("{}", tron_owner()),
        ["solana-create-nonce", blockhash, lamports] => {
            let blockhash = parse_solana_address(blockhash).expect("blockhash 有效");
            let payload = solana::build_create_nonce(&solana_payer(), blockhash, number(lamports)).expect("参数有效");
            submit(SOLANA, SOLANA_PATH, payload, None, builtin);
        }
        ["solana-transfer", to, amount, nonce_value] => {
            let payer = solana_payer();
            let nonce_value = parse_solana_address(nonce_value).expect("nonce 值有效");
            let params = solana_params(&payer, to, number(amount), nonce_value);
            submit(
                SOLANA,
                SOLANA_PATH,
                solana::build_transfer(&params).expect("参数有效"),
                None,
                builtin,
            );
        }
        ["solana-token", to, amount, mint, decimals, nonce_value, create_ata] => {
            let payer = solana_payer();
            let nonce_value = parse_solana_address(nonce_value).expect("nonce 值有效");
            let params = solana::TransferParams {
                mint: Some(mint),
                decimals: number(decimals),
                create_recipient_ata: *create_ata == "1",
                ..solana_params(&payer, to, number(amount), nonce_value)
            };
            let manifest = manifest_with(SOLANA, mint, params.decimals, "SPL");
            submit(
                SOLANA,
                SOLANA_PATH,
                solana::build_transfer(&params).expect("参数有效"),
                Some(to),
                &manifest,
            );
        }
        ["tron-transfer", to, amount, block @ ..] if block.len() == 4 => {
            let owner = tron_owner();
            let params = tron_params(&owner, to, amount, block);
            submit(
                TRON,
                TRON_PATH,
                tron::build_transfer(&params).expect("参数有效"),
                None,
                builtin,
            );
        }
        ["tron-token", to, amount, contract, fee_limit, block @ ..] if block.len() == 4 => {
            let owner = tron_owner();
            let params = tron::TransferParams {
                contract: Some(contract),
                fee_limit: number(fee_limit),
                ..tron_params(&owner, to, amount, block)
            };
            let manifest = manifest_with(TRON, contract, 6, "TRC-20");
            submit(
                TRON,
                TRON_PATH,
                tron::build_transfer(&params).expect("参数有效"),
                None,
                &manifest,
            );
        }
        // 节点自己构建的交易，比如部署合约，也走一遍同样的核对和签名。
        ["tron-sign", raw_data] => submit(TRON, TRON_PATH, unhex_vec(raw_data), None, builtin),
        ["tron-sign-with-key", key, raw_data] => tron_sign_with_key(key, raw_data),
        _ => panic!("未知命令：{args:?}"),
    }
}

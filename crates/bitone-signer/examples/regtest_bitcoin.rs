//! 用真实节点验证 Bitcoin 交易.由 scripts/regtest-bitcoin.sh 启动,容器名从环境变量传入.
//!
//! 冷钱包的请求按 Testnet4 走测试网路径,或按主网走旧路径,scriptPubKey 和 regtest 通用,
//! 只有地址前缀不同,所以节点这边用 bcrt1 地址打币和收款.

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::psbt::{Psbt, PsbtSighashType};
use bitcoin::secp256k1::PublicKey;
use bitcoin::{Address, Network as BitcoinNetwork, ScriptBuf, Transaction};
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::path::classify;
use bitone_core::protocol::VERSION;
use bitone_core::request::{KIND_TRANSACTION, SignRequest};
use bitone_core::review::Confirm;
use bitone_core::tx::bitcoin::{Payment, Selection, Utxo, build_psbt, script_for, select_coins};
use bitone_core::tx::{self, HighestChange, ReviewContext};
use bitone_core::{AccountKind, DerivationPath, Network};
use bitone_signer::keys::Wallet;
use serde_json::Value;
use std::process::Command;

const BTC: u64 = 100_000_000;
const FEE_RATE: u64 = 2;

struct Harness {
    container: String,
    wallet: Wallet,
    keys: PublicKeys,
    /// 节点钱包的收款地址,下面的外部收款方.
    peer: ScriptBuf,
}

impl Harness {
    fn cli(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("docker")
            .args([
                "exec",
                &self.container,
                "bitcoin-cli",
                "-regtest",
                "-rpcuser=bitone",
                "-rpcpassword=bitone",
            ])
            .args(args)
            .output()
            .expect("docker 可用");
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() {
            Ok(text)
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }

    fn node(&self, args: &[&str]) -> String {
        self.cli(args).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    fn wallet_rpc(&self, args: &[&str]) -> String {
        let mut full = vec!["-rpcwallet=miner"];
        full.extend_from_slice(args);
        self.node(&full)
    }

    fn mine(&self, blocks: u32) {
        let address = self.wallet_rpc(&["getnewaddress"]);
        self.node(&["generatetoaddress", &blocks.to_string(), &address]);
    }

    fn script(&self, path: &str) -> ScriptBuf {
        let parsed = DerivationPath::parse(path).expect("路径有效");
        let key = PublicKey::from_slice(&self.keys.secp256k1(&parsed).expect("账户已导出")).expect("公钥有效");
        script_for(classify(&parsed).expect("白名单").kind, &key).expect("比特币路径")
    }

    /// 打一笔钱到路径对应的地址,确认后返回 UTXO.
    fn fund(&self, path: &str, sats: u64) -> Utxo {
        let script = self.script(path);
        let address = Address::from_script(&script, BitcoinNetwork::Regtest)
            .expect("标准脚本")
            .to_string();
        let amount = format!("{}.{:08}", sats / BTC, sats % BTC);
        let txid = self.wallet_rpc(&["sendtoaddress", &address, &amount]);
        self.mine(1);
        let raw = self.node(&["getrawtransaction", &txid]);
        let prev: Transaction = deserialize(&Vec::from_hex(&raw).expect("十六进制")).expect("交易有效");
        let vout = prev
            .output
            .iter()
            .position(|out| out.script_pubkey == script)
            .expect("打币输出");
        Utxo {
            txid,
            vout: vout as u32,
            amount: sats,
            path: path.into(),
            prev_tx: Some(serialize(&prev)),
        }
    }

    /// 全 Taproot 的交易不需要前序交易.
    fn fund_taproot(&self, path: &str, sats: u64) -> Utxo {
        Utxo {
            prev_tx: None,
            ..self.fund(path, sats)
        }
    }

    fn peer_address(&self, network: Network) -> String {
        let target = if network == Network::Bitcoin {
            BitcoinNetwork::Bitcoin
        } else {
            BitcoinNetwork::Testnet4
        };
        Address::from_script(&self.peer, target).expect("标准脚本").to_string()
    }
}

struct Scenario<'a> {
    name: &'a str,
    network: Network,
    utxos: Vec<Utxo>,
    amount: u64,
    change_path: &'a str,
    change_kind: AccountKind,
    sighash: Option<u32>,
}

fn broadcast(h: &Harness, s: Scenario) -> Result<(), String> {
    let Selection { inputs, fee, change } =
        select_coins(&s.utxos, s.amount, FEE_RATE, s.change_kind).map_err(|e| e.to_string())?;
    let payments = [Payment {
        address: h.peer_address(s.network),
        amount: s.amount,
    }];
    let change_output = (change > 0).then_some((s.change_path, change));
    let mut payload = build_psbt(s.network, &h.keys, &inputs, &payments, change_output).map_err(|e| e.to_string())?;
    if let Some(value) = s.sighash {
        let mut psbt = Psbt::deserialize(&payload).map_err(|e| e.to_string())?;
        psbt.inputs
            .iter_mut()
            .for_each(|input| input.sighash_type = Some(PsbtSighashType::from_u32(value)));
        payload = psbt.serialize();
    }
    let request = SignRequest {
        version: VERSION,
        id: [7; 16],
        fingerprint: h.keys.fingerprint,
        network: s.network.id().into(),
        kind: KIND_TRANSACTION,
        paths: Vec::new(),
        payload,
        hint: None,
    };

    let ctx = ReviewContext {
        keys: &h.keys,
        manifest: Manifest::builtin(),
        highest_change: HighestChange::default(),
    };
    let review = tx::review(&request, &ctx);
    if review.confirm != Confirm::Tap {
        return Err(format!("核对没有放行:{:?} {:?}", review.forbidden, review.alerts));
    }
    let (response, _) = h
        .wallet
        .sign(&request, &h.keys, Manifest::builtin(), HighestChange::default())
        .map_err(|e| e.to_string())?;
    let verified = tx::verify(&request, &response, &[]).map_err(|e| e.to_string())?;
    let raw = verified.raw.to_lower_hex_string();

    // 改一个字节的交易必须被节点拒绝,否则下面的"接受"说明不了什么.
    let mut broken = verified.raw.clone();
    let tail = broken.len() - 8;
    broken[tail] ^= 1;
    let broken = format!("[\"{}\"]", broken.to_lower_hex_string());
    let rejected: Value = serde_json::from_str(&h.node(&["testmempoolaccept", &broken])).expect("JSON");
    if rejected[0]["allowed"].as_bool() != Some(false) {
        return Err("改过的交易没有被拒绝".to_string());
    }

    let accept: Value = serde_json::from_str(&h.node(&["testmempoolaccept", &format!("[\"{raw}\"]")])).expect("JSON");
    let result = &accept[0];
    if result["allowed"].as_bool() != Some(true) {
        return Err(format!("testmempoolaccept 拒绝:{result}"));
    }
    let vsize = result["vsize"].as_u64().unwrap_or(0);
    let node_fee = result["fees"]["base"]
        .as_f64()
        .map_or(0, |btc| (btc * BTC as f64).round() as u64);
    if node_fee != fee {
        return Err(format!("手续费和节点算的不同:本地 {fee},节点 {node_fee}"));
    }
    let txid = h.node(&["sendrawtransaction", &raw]);
    if txid != verified.hash {
        return Err(format!("txid 不一致:本地 {},节点 {txid}", verified.hash));
    }
    h.mine(1);
    let confirmed: Value = serde_json::from_str(&h.node(&["getrawtransaction", &txid, "1"])).expect("JSON");
    let confirmations = confirmed["confirmations"].as_u64().unwrap_or(0);
    if confirmations == 0 {
        return Err("没有确认".to_string());
    }
    println!(
        "{} 通过\n  txid {txid}\n  输入 {} 个,手续费 {fee} sat,虚拟大小 {vsize} vB,找零 {change} sat\n  testmempoolaccept allowed=true,sendrawtransaction 返回同一 txid,确认数 {confirmations}",
        s.name,
        inputs.len()
    );
    Ok(())
}

fn main() {
    let container = std::env::var("REGTEST_CONTAINER").expect("由 scripts/regtest-bitcoin.sh 启动");
    let wallet = Wallet::new(&[0u8; 16], "").expect("测试助记词有效");
    let keys = PublicKeys::decode(&wallet.public_keys(true).expect("派生成功")).expect("公钥有效");
    let mut h = Harness {
        container,
        wallet,
        keys,
        peer: ScriptBuf::new(),
    };

    h.node(&["createwallet", "miner"]);
    h.mine(110);
    let peer = h.wallet_rpc(&["getnewaddress", "peer", "bech32"]);
    let peer: Address = peer
        .parse::<Address<_>>()
        .expect("地址")
        .require_network(BitcoinNetwork::Regtest)
        .expect("regtest");
    h.peer = peer.script_pubkey();
    println!("节点版本 {}", h.node(&["-version"]));

    const SEGWIT: &str = "m/84'/1'/0'/0/0";
    const SEGWIT_CHANGE: &str = "m/84'/1'/0'/1/0";
    const TAPROOT: &str = "m/86'/1'/0'/0/0";
    const TAPROOT_CHANGE: &str = "m/86'/1'/0'/1/0";

    let scenarios = vec![
        Scenario {
            name: "SegWit 单输入,带找零",
            network: Network::BtcTestnet4,
            utxos: vec![h.fund(SEGWIT, 2 * BTC)],
            amount: BTC / 2,
            change_path: SEGWIT_CHANGE,
            change_kind: AccountKind::SegWit,
            sighash: None,
        },
        Scenario {
            name: "SegWit 单输入,无找零",
            network: Network::BtcTestnet4,
            utxos: vec![h.fund("m/84'/1'/0'/0/1", 50_000)],
            amount: 49_500,
            change_path: SEGWIT_CHANGE,
            change_kind: AccountKind::SegWit,
            sighash: None,
        },
        Scenario {
            name: "Taproot 单输入,带 Taproot 找零",
            network: Network::BtcTestnet4,
            utxos: vec![h.fund(TAPROOT, 2 * BTC)],
            amount: BTC / 2,
            change_path: TAPROOT_CHANGE,
            change_kind: AccountKind::Taproot,
            sighash: None,
        },
        Scenario {
            name: "Taproot 单输入,显式 SIGHASH_ALL",
            network: Network::BtcTestnet4,
            utxos: vec![h.fund("m/86'/1'/0'/0/1", 2 * BTC)],
            amount: BTC / 2,
            change_path: TAPROOT_CHANGE,
            change_kind: AccountKind::Taproot,
            sighash: Some(1),
        },
        Scenario {
            name: "两个 Taproot 输入,都不带前序交易",
            network: Network::BtcTestnet4,
            utxos: vec![
                h.fund_taproot("m/86'/1'/0'/0/3", BTC),
                h.fund_taproot("m/86'/1'/0'/0/4", BTC),
            ],
            amount: 3 * BTC / 2,
            change_path: TAPROOT_CHANGE,
            change_kind: AccountKind::Taproot,
            sighash: None,
        },
        Scenario {
            name: "SegWit 加 Taproot 混合输入,带找零",
            network: Network::BtcTestnet4,
            utxos: vec![h.fund("m/84'/1'/0'/0/2", BTC), h.fund("m/86'/1'/0'/0/2", BTC)],
            amount: 3 * BTC / 2,
            change_path: SEGWIT_CHANGE,
            change_kind: AccountKind::SegWit,
            sighash: None,
        },
        Scenario {
            name: "主网路径的旧式输入",
            network: Network::Bitcoin,
            utxos: vec![h.fund("m/44'/0'/0'/0/0", 2 * BTC)],
            amount: BTC / 2,
            change_path: "m/84'/0'/0'/1/0",
            change_kind: AccountKind::SegWit,
            sighash: None,
        },
        Scenario {
            name: "主网路径的嵌套 SegWit 输入",
            network: Network::Bitcoin,
            utxos: vec![h.fund("m/49'/0'/0'/0/0", 2 * BTC)],
            amount: BTC / 2,
            change_path: "m/49'/0'/0'/1/0",
            change_kind: AccountKind::NestedSegWit,
            sighash: None,
        },
    ];

    let mut failed = 0;
    for scenario in scenarios {
        let name = scenario.name.to_string();
        if let Err(reason) = broadcast(&h, scenario) {
            eprintln!("{name} 失败:{reason}");
            failed += 1;
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
    println!("全部通过");
}

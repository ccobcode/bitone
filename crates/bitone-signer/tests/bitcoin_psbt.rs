//! Bitcoin PSBT 的核对,签名,核验.需要真实的 `Wallet`,所以放在这里.
//! 助记词是 abandon×11 about,下面的伪造 PSBT 都是真的构造出来再序列化的.

use bitcoin::bip32::{DerivationPath as BipPath, Fingerprint, Xpriv};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::psbt::{Input, Psbt, PsbtSighashType};
use bitcoin::secp256k1::{PublicKey, Secp256k1, ecdsa};
use bitcoin::taproot::{TapTree, TaprootBuilder};
use bitcoin::{
    Amount, NetworkKind, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute, transaction,
};
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::path::classify;
use bitone_core::protocol::VERSION;
use bitone_core::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use bitone_core::review::{Confirm, Forbid, Review, Risk};
use bitone_core::tx::bitcoin::{self as btc, Payment, Utxo, build_psbt, script_for};
use bitone_core::tx::{self, HighestChange, ReviewContext};
use bitone_core::{AccountKind, DerivationPath, Error as CoreError, Network};
use bitone_signer::Error;
use bitone_signer::keys::Wallet;
use bitone_signer::mnemonic;
use std::str::FromStr;

const SEGWIT: &str = "m/84'/0'/0'/0/0";
const SEGWIT_2: &str = "m/84'/0'/0'/0/1";
const TAPROOT: &str = "m/86'/0'/0'/0/0";
const TAPROOT_2: &str = "m/86'/0'/0'/0/1";
const NESTED: &str = "m/49'/0'/0'/0/0";
const LEGACY: &str = "m/44'/0'/0'/0/0";
const CHANGE: &str = "m/84'/0'/0'/1/3";
const TAPROOT_CHANGE: &str = "m/86'/0'/0'/1/3";
// BIP-173 的示例地址,和测试钱包无关.
const PEER: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const PEER_2: &str = "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2";
const TEST_PEER: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";

struct Setup {
    wallet: Wallet,
    keys: PublicKeys,
    other: PublicKeys,
}

fn setup() -> Setup {
    let wallet = Wallet::new(&[0u8; 16], "").unwrap();
    let keys = PublicKeys::decode(&wallet.public_keys(true).unwrap()).unwrap();
    let stranger = Wallet::new(&[1u8; 16], "").unwrap();
    let other = PublicKeys::decode(&stranger.public_keys(true).unwrap()).unwrap();
    Setup { wallet, keys, other }
}

fn p(text: &str) -> DerivationPath {
    DerivationPath::parse(text).unwrap()
}

/// 测试钱包在这条路径上的公钥.网页端保存的公钥算不出没有导出的硬化路径.
fn seed_key(path: &str) -> PublicKey {
    let seed = mnemonic::seed(&[0u8; 16], "").unwrap();
    let secp = Secp256k1::new();
    let master = Xpriv::new_master(NetworkKind::Main, seed.as_ref()).unwrap();
    let child = master.derive_priv(&secp, &BipPath::from_str(path).unwrap()).unwrap();
    child.private_key.public_key(&secp)
}

fn public(keys: &PublicKeys, path: &str) -> PublicKey {
    PublicKey::from_slice(&keys.secp256k1(&p(path)).unwrap()).unwrap()
}

fn script(keys: &PublicKeys, path: &str) -> ScriptBuf {
    script_for(classify(&p(path)).unwrap().kind, &public(keys, path)).unwrap()
}

fn funding(pay_to: ScriptBuf, amount: u64, salt: u8) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([salt; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: pay_to,
        }],
    }
}

fn utxo(s: &Setup, path: &str, amount: u64, salt: u8) -> Utxo {
    let prev = funding(script(&s.keys, path), amount, salt);
    Utxo {
        txid: prev.compute_txid().to_string(),
        vout: 0,
        amount,
        path: path.into(),
        prev_tx: Some(serialize(&prev)),
    }
}

fn pay(address: &str, amount: u64) -> Payment {
    Payment {
        address: address.into(),
        amount,
    }
}

fn request(s: &Setup, network: Network, payload: Vec<u8>) -> SignRequest {
    SignRequest {
        version: VERSION,
        id: [9; 16],
        fingerprint: s.keys.fingerprint,
        network: network.id().into(),
        kind: KIND_TRANSACTION,
        paths: Vec::new(),
        payload,
        hint: None,
        chain: None,
    }
}

fn review_with(s: &Setup, network: Network, payload: Vec<u8>, highest_change: u32) -> Review {
    let ctx = ReviewContext {
        keys: &s.keys,
        manifest: Manifest::builtin(),
        highest_change: HighestChange {
            mainnet: highest_change,
            testnet: highest_change,
        },
    };
    let review = tx::review(&request(s, network, payload), &ctx);
    assert_eq!(review.field_count as usize, review.fields.len());
    if review.is_signable() {
        assert!(!review.fields.is_empty());
    }
    review
}

fn review(s: &Setup, payload: Vec<u8>) -> Review {
    review_with(s, Network::Bitcoin, payload, 0)
}

fn forbidden(review: Review) -> Option<Forbid> {
    assert_eq!(review.confirm, Confirm::Refuse);
    review.forbidden
}

/// 改动一份合法的 PSBT.
fn forged(payload: &[u8], change: impl FnOnce(&mut Psbt)) -> Vec<u8> {
    let mut psbt = Psbt::deserialize(payload).unwrap();
    change(&mut psbt);
    psbt.serialize()
}

fn send(s: &Setup, inputs: &[Utxo], change: &str) -> Vec<u8> {
    let payments = [pay(PEER, 400_000)];
    build_psbt(Network::Bitcoin, &s.keys, inputs, &payments, Some((change, 599_000))).unwrap()
}

fn segwit_send(s: &Setup) -> Vec<u8> {
    send(s, &[utxo(s, SEGWIT, 1_000_000, 1)], CHANGE)
}

fn taproot_send(s: &Setup) -> Vec<u8> {
    send(s, &[utxo(s, TAPROOT, 1_000_000, 1)], TAPROOT_CHANGE)
}

fn mixed_send(s: &Setup) -> Vec<u8> {
    send(s, &[utxo(s, SEGWIT, 500_000, 1), utxo(s, TAPROOT, 500_000, 2)], CHANGE)
}

/// 给 PSBT 加一个其他钱包的输入,金额多少都用输出补平.
fn add_foreign(psbt: &mut Psbt, input: Input, prev: &Transaction) {
    psbt.unsigned_tx.input.push(TxIn {
        previous_output: OutPoint {
            txid: prev.compute_txid(),
            vout: 0,
        },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::new(),
    });
    psbt.inputs.push(input);
}

fn foreign_with_prev_tx(prev: &Transaction) -> Input {
    Input {
        non_witness_utxo: Some(prev.clone()),
        ..Input::default()
    }
}

fn foreign_witness_only(prev: &Transaction) -> Input {
    Input {
        witness_utxo: Some(prev.output[0].clone()),
        ..Input::default()
    }
}

fn sighash(value: u32) -> Option<PsbtSighashType> {
    Some(PsbtSighashType::from_u32(value))
}

fn set_path(input: &mut Input, path: &str) {
    let path = BipPath::from_str(path).unwrap();
    for (_, source) in input.bip32_derivation.iter_mut() {
        source.1 = path.clone();
    }
    for (_, (_, source)) in input.tap_key_origins.iter_mut() {
        source.1 = path.clone();
    }
}

fn sign(s: &Setup, network: Network, payload: Vec<u8>) -> Result<(SignRequest, SignResponse, Review), Error> {
    let req = request(s, network, payload);
    let (response, review) = s
        .wallet
        .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())?;
    Ok((req, response, review))
}

mod ownership {
    use super::*;

    #[test]
    fn input_paths_and_fingerprints_come_from_the_psbt() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        assert!(review.alerts.is_empty());
        assert!(review.rows.iter().any(|r| r.label == "派生路径" && r.value == SEGWIT));
        assert!(
            review
                .rows
                .iter()
                .any(|r| r.value == "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu")
        );
    }

    #[test]
    fn foreign_input_is_an_alert_and_not_my_money() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            add_foreign(psbt, foreign_with_prev_tx(&prev), &prev);
            psbt.unsigned_tx.output[0].value = Amount::from_sat(650_000);
        });
        let review = review(&s, payload);
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts, vec!["有 1 个输入不属于本钱包"]);
        let row = review.rows.iter().find(|r| r.label == "其他钱包的输入").unwrap();
        assert_eq!(row.value, "1 个,共 0.0025 BTC");
        assert_eq!(review.rows.iter().filter(|r| r.label == "付款账户").count(), 1);
        assert_eq!(review.amount.unwrap().text, "0.0065");
    }

    #[test]
    fn input_with_another_fingerprint_counts_as_foreign() {
        let s = setup();
        let payload = forged(&mixed_send(&s), |psbt| {
            for (_, (_, source)) in psbt.inputs[1].tap_key_origins.iter_mut() {
                source.0 = Fingerprint::from([1, 2, 3, 4]);
            }
        });
        let review = review(&s, payload);
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(review.alerts, vec!["有 1 个输入不属于本钱包"]);
    }

    #[test]
    fn all_inputs_foreign_is_refused() {
        let s = setup();
        let strip = |psbt: &mut Psbt| {
            psbt.inputs[0].bip32_derivation.clear();
            psbt.inputs[0].tap_key_origins.clear();
        };
        assert_eq!(
            forbidden(review(&s, forged(&segwit_send(&s), strip))),
            Some(Forbid::AddressMismatch)
        );
        assert_eq!(
            forbidden(review(&s, forged(&taproot_send(&s), strip))),
            Some(Forbid::AddressMismatch)
        );
    }

    /// 公钥算得出的路径,算出来不一样就拒绝.
    #[test]
    fn a_derivable_input_path_of_another_address_is_refused() {
        let s = setup();
        for path in ["m/84'/0'/0'/2/0", "m/84'/0'/0'/0/100000", "m/84'/0'/0'/0/1/7"] {
            let payload = forged(&segwit_send(&s), |psbt| set_path(&mut psbt.inputs[0], path));
            assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch), "{path}");
        }
    }

    /// 公钥算不出的路径要等解锁,这时才有结论.
    #[test]
    fn an_unverifiable_input_path_warns_and_is_settled_after_unlocking() {
        let s = setup();
        for path in ["m/84'/0'/1'/0/0", "m/9'/0'/0'/0/0", "m/0"] {
            let payload = forged(&segwit_send(&s), |psbt| set_path(&mut psbt.inputs[0], path));
            let shown = review(&s, payload.clone());
            assert_eq!((shown.risk, shown.confirm), (Risk::Transfer, Confirm::Hold), "{path}");
            assert!(shown.alerts[0].contains(path), "{}", shown.alerts[0]);
            assert_eq!(
                sign(&s, Network::Bitcoin, payload).unwrap_err(),
                Error::Refused,
                "{path}"
            );
        }
    }

    /// 真实的非常见路径:私钥派生得出,网页端的公钥算不出.
    #[test]
    fn any_path_can_spend() {
        let s = setup();
        for path in ["m/84'/0'/7'/0/0", "m/9'/0'/0'", "m/84'/5'/0'/3/1"] {
            let key = seed_key(path);
            let prev = funding(script_for(AccountKind::SegWit, &key).unwrap(), 1_000_000, 9);
            let payload = forged(&segwit_send(&s), |psbt| {
                psbt.unsigned_tx.input[0].previous_output.txid = prev.compute_txid();
                let source = (Fingerprint::from(s.keys.fingerprint), BipPath::from_str(path).unwrap());
                psbt.inputs[0] = Input {
                    non_witness_utxo: Some(prev.clone()),
                    bip32_derivation: [(key, source)].into(),
                    ..Input::default()
                };
            });
            let (req, response, shown) = sign(&s, Network::Bitcoin, payload).unwrap();
            assert_eq!(shown.confirm, Confirm::Hold, "{path}");
            assert!(btc::verify(Network::Bitcoin, &req, &response).is_ok(), "{path}");
        }
    }

    #[test]
    fn a_usual_key_on_another_script_type_warns() {
        let s = setup();
        let key = public(&s.keys, SEGWIT);
        let prev = funding(script_for(AccountKind::Legacy, &key).unwrap(), 1_000_000, 9);
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.input[0].previous_output.txid = prev.compute_txid();
            psbt.inputs[0].non_witness_utxo = Some(prev.clone());
            psbt.inputs[0].witness_utxo = None;
        });
        let shown = review(&s, payload);
        assert_eq!(shown.confirm, Confirm::Hold);
        assert!(shown.alerts[0].contains(SEGWIT), "{}", shown.alerts[0]);
    }

    #[test]
    fn input_path_of_another_network_is_refused() {
        let s = setup();
        let main = forged(&segwit_send(&s), |psbt| {
            set_path(&mut psbt.inputs[0], "m/84'/1'/0'/0/0")
        });
        assert_eq!(forbidden(review(&s, main)), Some(Forbid::PathNetworkMismatch));
        let evm = forged(&segwit_send(&s), |psbt| {
            set_path(&mut psbt.inputs[0], "m/44'/60'/0'/0/0")
        });
        assert_eq!(forbidden(review(&s, evm)), Some(Forbid::PathNetworkMismatch));
    }

    #[test]
    fn colliding_fingerprint_does_not_make_an_input_mine() {
        // 4 字节指纹可以撞出来:这里的公钥和 script 自洽,但路径派生不出它.
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 1_000_000, 1);
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = psbt.inputs[0].bip32_derivation.values().next().unwrap().clone();
            psbt.unsigned_tx.input[0].previous_output.txid = prev.compute_txid();
            psbt.inputs[0] = Input {
                non_witness_utxo: Some(prev.clone()),
                bip32_derivation: [(public(&s.other, SEGWIT), source)].into(),
                ..Input::default()
            };
        });
        assert_eq!(forbidden(review(&s, payload.clone())), Some(Forbid::AddressMismatch));
        assert_eq!(sign(&s, Network::Bitcoin, payload).unwrap_err(), Error::Refused);
    }

    #[test]
    fn path_of_a_different_address_is_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| set_path(&mut psbt.inputs[0], SEGWIT_2));
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn stated_key_must_produce_the_input_script() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = psbt.inputs[0].bip32_derivation.values().next().unwrap().clone();
            psbt.inputs[0].bip32_derivation = [(public(&s.other, SEGWIT), source)].into();
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn two_entries_of_my_fingerprint_on_one_input_are_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = psbt.inputs[0].bip32_derivation.values().next().unwrap().clone();
            psbt.inputs[0]
                .bip32_derivation
                .insert(public(&s.keys, SEGWIT_2), source);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn taproot_path_in_the_ecdsa_map_is_refused() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| {
            let (_, source) = psbt.inputs[0].tap_key_origins.values().next().unwrap().clone();
            psbt.inputs[0].tap_key_origins.clear();
            psbt.inputs[0].bip32_derivation.insert(public(&s.keys, TAPROOT), source);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn nested_input_with_a_wrong_redeem_script_is_refused() {
        let s = setup();
        let inputs = [utxo(&s, NESTED, 1_000_000, 1)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 400_000)], None).unwrap();
        assert_eq!(review(&s, payload.clone()).risk, Risk::Transfer);
        let bad = forged(&payload, |psbt| {
            psbt.inputs[0].redeem_script = Some(script(&s.keys, SEGWIT_2))
        });
        assert_eq!(forbidden(review(&s, bad)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn taproot_input_with_a_merkle_root_or_script_leaves_is_refused() {
        let s = setup();
        let root = forged(&taproot_send(&s), |psbt| {
            psbt.inputs[0].tap_merkle_root = Some(bitcoin::taproot::TapNodeHash::from_byte_array([3; 32]));
        });
        assert_eq!(forbidden(review(&s, root)), Some(Forbid::AddressMismatch));
        let leaves = forged(&taproot_send(&s), |psbt| {
            let (hashes, _) = psbt.inputs[0].tap_key_origins.values_mut().next().unwrap();
            hashes.push(bitcoin::taproot::TapLeafHash::from_byte_array([4; 32]));
        });
        assert_eq!(forbidden(review(&s, leaves)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn other_request_kinds_are_refused() {
        let s = setup();
        let mut req = request(&s, Network::Bitcoin, segwit_send(&s));
        req.kind = 9;
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange::default(),
        };
        assert_eq!(forbidden(tx::review(&req, &ctx)), Some(Forbid::RawHashSigning));
    }

    /// 手续费确认不了,可以签,类别是无法识别.
    fn assert_fee_unconfirmed(review: &Review) {
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(
            review.alerts.iter().any(|alert| alert.contains("手续费无法确认")),
            "{:?}",
            review.alerts
        );
    }

    #[test]
    fn segwit_input_without_the_previous_transaction_is_unknown_and_signable() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.inputs[0].non_witness_utxo = None);
        let shown = review(&s, payload.clone());
        assert_fee_unconfirmed(&shown);
        let fee = shown.fields.iter().find(|row| row.name == "fee").unwrap();
        assert_eq!(fee.risk, Some(Risk::Unknown));
        assert!(fee.note.as_ref().unwrap().contains("手续费无法确认"));
        let (_, response, signed) = sign(&s, Network::Bitcoin, payload).unwrap();
        assert_fee_unconfirmed(&signed);
        assert_eq!(*response.signatures[0].last().unwrap(), 0x01);
    }

    #[test]
    fn legacy_input_without_the_previous_transaction_is_refused() {
        let s = setup();
        let inputs = [utxo(&s, LEGACY, 1_000_000, 1)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 400_000)], None).unwrap();
        let payload = forged(&payload, |psbt| {
            let spent = psbt.inputs[0].non_witness_utxo.take().unwrap().output[0].clone();
            psbt.inputs[0].witness_utxo = Some(spent);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::MissingPrevTx));
    }

    #[test]
    fn input_without_any_utxo_data_is_refused() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| psbt.inputs[0].witness_utxo = None);
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::MissingPrevTx));
    }

    #[test]
    fn previous_transaction_must_hash_to_the_input_txid() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let prev = psbt.inputs[0].non_witness_utxo.as_mut().unwrap();
            prev.lock_time = absolute::LockTime::from_consensus(7);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::PrevTxMismatch));
    }

    #[test]
    fn output_index_must_exist_in_the_previous_transaction() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.input[0].previous_output.vout = 1
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::PrevTxMismatch));
    }

    /// CVE-2020-14199:SegWit 签名只承诺自身输入的金额,用户看到的手续费可以被压低.
    #[test]
    fn witness_utxo_amount_cannot_differ_from_the_previous_transaction() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.inputs[0].witness_utxo.as_mut().unwrap().value = Amount::from_sat(600_000);
        });
        assert_eq!(forbidden(review(&s, payload.clone())), Some(Forbid::PrevTxMismatch));
        assert_eq!(sign(&s, Network::Bitcoin, payload).unwrap_err(), Error::Refused);
    }

    #[test]
    fn witness_utxo_script_cannot_differ_from_the_previous_transaction() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.inputs[0].witness_utxo.as_mut().unwrap().script_pubkey = script(&s.other, SEGWIT);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::PrevTxMismatch));
    }

    #[test]
    fn amount_comes_from_the_previous_transaction_not_witness_utxo() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.inputs[0].witness_utxo = None);
        let review = review(&s, payload);
        assert!(review.is_signable());
        assert_eq!(
            review.rows.iter().find(|r| r.label == "手续费").unwrap().value,
            "0.00001 BTC"
        );
    }

    #[test]
    fn all_taproot_transaction_can_skip_the_previous_transaction() {
        let s = setup();
        let inputs = [utxo(&s, TAPROOT, 500_000, 1), utxo(&s, TAPROOT_2, 500_000, 2)];
        let payload = build_psbt(
            Network::Bitcoin,
            &s.keys,
            &inputs,
            &[pay(PEER, 400_000)],
            Some((CHANGE, 599_000)),
        )
        .unwrap();
        let psbt = Psbt::deserialize(&payload).unwrap();
        assert!(psbt.inputs.iter().all(|i| i.non_witness_utxo.is_none()));
        assert_eq!(review(&s, payload).confirm, Confirm::Tap);
    }

    #[test]
    fn taproot_previous_transaction_is_still_checked_when_present() {
        let s = setup();
        let bad = funding(script(&s.keys, TAPROOT), 1_000_000, 200);
        let payload = forged(&taproot_send(&s), |psbt| psbt.inputs[0].non_witness_utxo = Some(bad));
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::PrevTxMismatch));
    }

    #[test]
    fn mixed_transaction_keeps_the_previous_transaction_requirement() {
        let s = setup();
        let good = mixed_send(&s);
        assert_eq!(review(&s, good.clone()).confirm, Confirm::Tap);
        let payload = forged(&good, |psbt| psbt.inputs[0].non_witness_utxo = None);
        assert_fee_unconfirmed(&review(&s, payload));
    }

    /// 用户有两个 SegWit 币.每一轮把其中一个说成本钱包的 Taproot 输入,金额压低,
    /// 另一个币的 ECDSA 签名照样有效.两轮的签名拼起来,用户看到的手续费是假的.
    /// 在按"混合交易只要求非 Taproot 带前序交易"写的代码上,这笔交易被 regtest 节点接受了.
    /// 手续费不再拦截签名,所以这里必须是无法识别,告警写明手续费无法确认.
    #[test]
    fn segwit_coin_disguised_as_my_taproot_input_cannot_pass_as_plain() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 100_000_000, 1), utxo(&s, SEGWIT_2, 100_000_000, 2)];
        let honest = build_psbt(
            Network::Bitcoin,
            &s.keys,
            &inputs,
            &[pay(PEER, 50_000_000)],
            Some((CHANGE, 59_990_000)),
        )
        .unwrap();
        for disguised in [0, 1] {
            let payload = forged(&honest, |psbt| {
                let internal = public(&s.keys, TAPROOT).x_only_public_key().0;
                let source = (
                    Fingerprint::from(s.keys.fingerprint),
                    BipPath::from_str(TAPROOT).unwrap(),
                );
                let input = &mut psbt.inputs[disguised];
                input.non_witness_utxo = None;
                input.bip32_derivation.clear();
                input.witness_utxo = Some(TxOut {
                    value: Amount::from_sat(10_000_000),
                    script_pubkey: script(&s.keys, TAPROOT),
                });
                input.tap_internal_key = Some(internal);
                input.tap_key_origins.insert(internal, (Vec::new(), source));
            });
            assert_fee_unconfirmed(&review(&s, payload));
        }
    }

    #[test]
    fn taproot_input_next_to_segwit_needs_its_previous_transaction_to_confirm_the_fee() {
        let s = setup();
        let payload = forged(&mixed_send(&s), |psbt| psbt.inputs[1].non_witness_utxo = None);
        assert_fee_unconfirmed(&review(&s, payload));
    }

    #[test]
    fn all_taproot_stays_exempt_only_while_every_signature_covers_all_amounts() {
        let s = setup();
        let inputs = [utxo(&s, TAPROOT, 500_000, 1), utxo(&s, TAPROOT_2, 500_000, 2)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 999_000)], None).unwrap();
        assert_eq!(review(&s, payload.clone()).confirm, Confirm::Tap);
        // 两个 ANYONECANPAY 各管各的金额,同样可以分两轮骗.
        let payload = forged(&payload, |psbt| {
            psbt.inputs[0].sighash_type = sighash(0x81);
            psbt.inputs[1].sighash_type = sighash(0x81);
        });
        assert_fee_unconfirmed(&review(&s, payload));
    }

    #[test]
    fn single_taproot_input_with_anyone_can_pay_is_signable_with_a_hold() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| psbt.inputs[0].sighash_type = sighash(0x81));
        assert_eq!(review(&s, payload).confirm, Confirm::Hold);
    }

    /// 其他钱包的输入把自己声明成 Taproot,本钱包全是 SegWit,同样没有签名能替它作证.
    #[test]
    fn foreign_input_cannot_claim_to_be_taproot_next_to_segwit() {
        let s = setup();
        let prev = funding(script(&s.other, TAPROOT), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            add_foreign(psbt, foreign_witness_only(&prev), &prev)
        });
        assert_fee_unconfirmed(&review(&s, payload));
    }

    #[test]
    fn my_taproot_signature_vouches_for_a_foreign_taproot_input() {
        let s = setup();
        let prev = funding(script(&s.other, TAPROOT), 250_000, 7);
        let payload = forged(&taproot_send(&s), |psbt| {
            add_foreign(psbt, foreign_witness_only(&prev), &prev)
        });
        let review = review(&s, payload);
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(review.alerts[0], "有 1 个输入不属于本钱包");
    }

    #[test]
    fn foreign_segwit_input_without_its_previous_transaction_leaves_the_fee_unconfirmed() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let payload = forged(&taproot_send(&s), |psbt| {
            add_foreign(psbt, foreign_witness_only(&prev), &prev)
        });
        assert_fee_unconfirmed(&review(&s, payload));
    }

    #[test]
    fn anyone_can_pay_does_not_vouch_for_other_inputs() {
        let s = setup();
        let prev = funding(script(&s.other, TAPROOT), 250_000, 7);
        let payload = forged(&taproot_send(&s), |psbt| {
            add_foreign(psbt, foreign_witness_only(&prev), &prev);
            psbt.inputs[0].sighash_type = sighash(0x81);
        });
        assert_fee_unconfirmed(&review(&s, payload));
    }

    #[test]
    fn duplicate_inputs_are_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let (txin, input) = (psbt.unsigned_tx.input[0].clone(), psbt.inputs[0].clone());
            psbt.unsigned_tx.input.push(txin);
            psbt.inputs.push(input);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let s = setup();
        let mut payload = segwit_send(&s);
        payload.push(0);
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn transaction_without_outputs_is_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output.clear();
            psbt.outputs.clear();
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::ParseFailed));
    }
}

mod sighash_types {
    use super::*;

    #[test]
    fn segwit_accepts_all_and_unset() {
        let s = setup();
        let review_all = review(
            &s,
            forged(&segwit_send(&s), |psbt| psbt.inputs[0].sighash_type = sighash(1)),
        );
        assert_eq!(review_all.confirm, Confirm::Tap);
        let review_unset = review(&s, segwit_send(&s));
        assert_eq!(
            review_unset.rows.iter().find(|r| r.label == "sighash").unwrap().value,
            "ALL"
        );
    }

    #[test]
    fn taproot_accepts_default_and_all() {
        let s = setup();
        let unset = review(&s, taproot_send(&s));
        assert_eq!(unset.confirm, Confirm::Tap);
        assert_eq!(
            unset.rows.iter().find(|r| r.label == "sighash").unwrap().value,
            "DEFAULT"
        );
        for value in [0, 1] {
            let payload = forged(&taproot_send(&s), |psbt| psbt.inputs[0].sighash_type = sighash(value));
            assert_eq!(review(&s, payload).confirm, Confirm::Tap, "{value}");
        }
    }

    #[test]
    fn other_values_are_unknown_and_say_the_signature_does_not_bind_the_page() {
        let s = setup();
        for (send, value, label) in [
            (segwit_send(&s), 0x02, "NONE"),
            (segwit_send(&s), 0x03, "SINGLE"),
            (segwit_send(&s), 0x81, "ALL + ANYONECANPAY"),
            (taproot_send(&s), 0x02, "NONE"),
            (taproot_send(&s), 0x83, "SINGLE + ANYONECANPAY"),
        ] {
            let payload = forged(&send, |psbt| psbt.inputs[0].sighash_type = sighash(value));
            let review = review(&s, payload);
            assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold), "{value}");
            assert_eq!(review.alerts.len(), 1);
            assert!(review.alerts[0].contains(label), "{}", review.alerts[0]);
            assert!(review.alerts[0].contains("不绑定页面显示的全部收款方和金额"));
            assert!(review.alerts[0].contains("签名后它们可以被改动"));
        }
    }

    #[test]
    fn none_on_a_legacy_or_nested_input_is_unknown() {
        let s = setup();
        for path in [LEGACY, NESTED] {
            let inputs = [utxo(&s, path, 1_000_000, 1)];
            let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 400_000)], None).unwrap();
            let payload = forged(&payload, |psbt| psbt.inputs[0].sighash_type = sighash(2));
            let result = review(&s, payload);
            assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold), "{path}");
            assert!(result.alerts.iter().any(|a| a.contains("NONE")));
        }
    }

    #[test]
    fn the_alert_names_the_input_that_is_not_bound() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 500_000, 1), utxo(&s, SEGWIT_2, 500_000, 2)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 900_000)], None).unwrap();
        let payload = forged(&payload, |psbt| psbt.inputs[1].sighash_type = sighash(2));
        let result = review(&s, payload);
        assert_eq!(result.risk, Risk::Unknown);
        assert!(result.alerts[0].starts_with("第 2 个输入"), "{}", result.alerts[0]);
    }

    #[test]
    fn single_without_a_matching_output_is_refused() {
        // 旧式输入这样签出来的摘要是常量 1,签名能花掉这个地址的所有币.
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 500_000, 1), utxo(&s, LEGACY, 500_000, 2)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 900_000)], None).unwrap();
        let payload = forged(&payload, |psbt| psbt.inputs[1].sighash_type = sighash(3));
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::Unsupported));
    }

    #[test]
    fn values_that_are_not_sighash_types_are_refused() {
        let s = setup();
        for (send, value) in [(segwit_send(&s), 0), (segwit_send(&s), 0x04), (taproot_send(&s), 0x100)] {
            let payload = forged(&send, |psbt| psbt.inputs[0].sighash_type = sighash(value));
            assert_eq!(forbidden(review(&s, payload)), Some(Forbid::Unsupported), "{value}");
        }
    }

    #[test]
    fn foreign_sighash_does_not_matter() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            let mut input = foreign_with_prev_tx(&prev);
            input.sighash_type = sighash(2);
            add_foreign(psbt, input, &prev);
            psbt.unsigned_tx.output[0].value = Amount::from_sat(650_000);
        });
        assert_eq!(review(&s, payload).alerts, vec!["有 1 个输入不属于本钱包"]);
    }
}

mod change {
    use super::*;

    fn with_change_path(s: &Setup, path: &str) -> Vec<u8> {
        forged(&segwit_send(s), |psbt| {
            for (_, source) in psbt.outputs[1].bip32_derivation.iter_mut() {
                source.1 = BipPath::from_str(path).unwrap();
            }
        })
    }

    #[test]
    fn change_is_recognised_and_not_counted_as_sent() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        assert_eq!(review.amount.as_ref().unwrap().text, "0.004");
        assert_eq!(review.change_index, Some(3));
        let change = review.actions.iter().find(|a| a.title == "找零").unwrap();
        assert_eq!(change.risk, Risk::Transfer);
    }

    #[test]
    fn output_without_derivation_is_external() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.outputs[1].bip32_derivation.clear());
        let review = review(&s, payload);
        assert_eq!(review.change_index, None);
        assert_eq!(review.amount.unwrap().text, "0.00999");
    }

    #[test]
    fn output_with_another_fingerprint_is_external() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            for (_, source) in psbt.outputs[1].bip32_derivation.iter_mut() {
                source.0 = Fingerprint::from([1, 2, 3, 4]);
            }
        });
        let review = review(&s, payload);
        assert_eq!(review.change_index, None);
        assert_eq!(review.actions.iter().filter(|a| a.title == "找零").count(), 0);
    }

    /// 输出改成这条路径的地址,路径,公钥,script 全部对得上.
    fn with_own_output(s: &Setup, path: &str) -> Vec<u8> {
        let key = public(&s.keys, path);
        forged(&segwit_send(s), |psbt| {
            let source = (Fingerprint::from(s.keys.fingerprint), BipPath::from_str(path).unwrap());
            psbt.unsigned_tx.output[1].script_pubkey = script_for(AccountKind::SegWit, &key).unwrap();
            psbt.outputs[1].bip32_derivation = [(key, source)].into();
        })
    }

    #[test]
    fn a_far_index_is_this_wallet_with_a_warning_and_a_hold() {
        let s = setup();
        for index in [100_000, 999_999_999] {
            let path = format!("m/84'/0'/0'/1/{index}");
            let payload = with_own_output(&s, &path);
            let shown = review(&s, payload.clone());
            assert_eq!((shown.risk, shown.confirm), (Risk::Transfer, Confirm::Hold), "{path}");
            assert_eq!(shown.change_index, Some(index));
            assert_eq!(
                shown.alerts,
                vec![format!("找零索引 {index} 跳得太远,资金可能落到不容易找回的位置")]
            );
            assert!(shown.actions.iter().any(|a| a.title == "找零"));
            assert!(sign(&s, Network::Bitcoin, payload).is_ok(), "{path}");
        }
    }

    #[test]
    fn an_unusual_change_path_is_this_wallet_with_a_warning_and_a_hold() {
        let s = setup();
        let path = "m/84'/0'/0'/9/3";
        let shown = review(&s, with_own_output(&s, path));
        assert_eq!((shown.risk, shown.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(shown.change_index, None);
        assert!(shown.alerts[0].contains(path), "{}", shown.alerts[0]);
        assert!(shown.actions.iter().any(|a| a.title == "找零"));
        let row = shown.fields.iter().find(|row| row.own && row.value == path).unwrap();
        assert_eq!(row.name, "output");
    }

    #[test]
    fn a_receive_address_as_change_is_this_wallet_with_a_warning() {
        let s = setup();
        let shown = review(&s, with_own_output(&s, "m/84'/0'/0'/0/3"));
        assert_eq!(shown.confirm, Confirm::Hold);
        assert_eq!(shown.change_index, None);
        assert_eq!(shown.amount.unwrap().text, "0.004");
    }

    #[test]
    fn a_claimed_output_that_does_not_rederive_is_refused() {
        let s = setup();
        assert_eq!(
            forbidden(review(&s, with_change_path(&s, "m/84'/0'/0'/2/3"))),
            Some(Forbid::AddressMismatch)
        );
    }

    #[test]
    fn a_change_path_of_another_chain_or_key_is_refused() {
        let s = setup();
        for path in ["m/84'/1'/0'/1/3", "m/44'/60'/0'/0/0"] {
            assert_eq!(
                forbidden(review(&s, with_change_path(&s, path))),
                Some(Forbid::AddressMismatch),
                "{path}"
            );
        }
    }

    /// 公钥算不出来时不能说它是本钱包的,按外部收款方显示,解锁后再定.
    #[test]
    fn an_unverifiable_claim_is_external_and_settled_after_unlocking() {
        let s = setup();
        let path = "m/84'/0'/1'/1/3";
        let payload = with_change_path(&s, path);
        let shown = review(&s, payload.clone());
        assert_eq!((shown.risk, shown.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(shown.amount.as_ref().unwrap().text, "0.00999");
        assert!(shown.fields.iter().all(|row| !row.own || row.name == "input"));
        assert!(shown.alerts.iter().any(|alert| alert.contains(path)));
        assert_eq!(sign(&s, Network::Bitcoin, payload).unwrap_err(), Error::Refused);
    }

    #[test]
    fn a_real_change_output_on_an_unexported_path_signs_after_unlocking() {
        let s = setup();
        let path = "m/84'/0'/1'/1/3";
        let key = seed_key(path);
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = (Fingerprint::from(s.keys.fingerprint), BipPath::from_str(path).unwrap());
            psbt.unsigned_tx.output[1].script_pubkey = script_for(AccountKind::SegWit, &key).unwrap();
            psbt.outputs[1].bip32_derivation = [(key, source)].into();
        });
        assert!(sign(&s, Network::Bitcoin, payload).is_ok());
    }

    #[test]
    fn change_script_replaced_by_an_external_address_is_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[1].script_pubkey = script(&s.other, CHANGE);
        });
        assert_eq!(forbidden(review(&s, payload.clone())), Some(Forbid::AddressMismatch));
        assert_eq!(sign(&s, Network::Bitcoin, payload).unwrap_err(), Error::Refused);
    }

    #[test]
    fn taproot_change_is_recognised() {
        let s = setup();
        let review = review(&s, taproot_send(&s));
        assert_eq!(review.change_index, Some(3));
        assert_eq!(review.amount.unwrap().text, "0.004");
    }

    #[test]
    fn taproot_change_with_a_tree_is_refused() {
        let s = setup();
        let builder = TaprootBuilder::new()
            .add_leaf(0, ScriptBuf::new_op_return([1u8]))
            .unwrap();
        let tree = TapTree::try_from(builder).unwrap();
        let payload = forged(&taproot_send(&s), |psbt| psbt.outputs[1].tap_tree = Some(tree));
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn taproot_change_hiding_a_script_path_is_refused() {
        // 内部公钥是自己的,但输出密钥藏了别人能花的脚本树,PSBT 里的 tap_tree 故意留空.
        let s = setup();
        let secp = Secp256k1::new();
        let internal = public(&s.keys, TAPROOT_CHANGE).x_only_public_key().0;
        let builder = TaprootBuilder::new()
            .add_leaf(0, ScriptBuf::new_op_return([1u8]))
            .unwrap();
        let spend = builder.finalize(&secp, internal).unwrap();
        let payload = forged(&taproot_send(&s), |psbt| {
            psbt.unsigned_tx.output[1].script_pubkey = ScriptBuf::new_p2tr(&secp, internal, spend.merkle_root());
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn taproot_change_with_another_internal_key_is_refused() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| {
            psbt.outputs[1].tap_internal_key = Some(public(&s.other, TAPROOT).x_only_public_key().0);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn taproot_change_with_script_leaves_is_refused() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| {
            let (hashes, _) = psbt.outputs[1].tap_key_origins.values_mut().next().unwrap();
            hashes.push(bitcoin::taproot::TapLeafHash::from_byte_array([4; 32]));
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn two_entries_of_my_fingerprint_on_an_output_are_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = psbt.outputs[1].bip32_derivation.values().next().unwrap().clone();
            psbt.outputs[1]
                .bip32_derivation
                .insert(public(&s.keys, SEGWIT_2), source);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn index_past_the_gap_is_high_risk() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 1_000_000, 1)];
        let build = |index: u32| {
            let path = format!("m/84'/0'/0'/1/{index}");
            build_psbt(
                Network::Bitcoin,
                &s.keys,
                &inputs,
                &[pay(PEER, 400_000)],
                Some((&path, 599_000)),
            )
            .unwrap()
        };
        let at_gap = review_with(&s, Network::Bitcoin, build(30), 10);
        assert_eq!(at_gap.confirm, Confirm::Tap);
        let past_gap = review_with(&s, Network::Bitcoin, build(31), 10);
        assert_eq!(past_gap.confirm, Confirm::Hold);
        assert_eq!(past_gap.change_index, Some(31));
        assert_eq!(past_gap.alerts.len(), 1);
    }

    #[test]
    fn gap_does_not_overflow() {
        let s = setup();
        let review = review_with(&s, Network::Bitcoin, segwit_send(&s), u32::MAX);
        assert_eq!(review.confirm, Confirm::Tap);
    }

    #[test]
    fn change_index_is_the_highest_of_the_transaction() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = |path: &str| (Fingerprint::from(s.keys.fingerprint), BipPath::from_str(path).unwrap());
            let path = "m/84'/0'/0'/1/7";
            psbt.unsigned_tx.output.push(TxOut {
                value: Amount::from_sat(1),
                script_pubkey: script(&s.keys, path),
            });
            let mut meta = bitcoin::psbt::Output::default();
            meta.bip32_derivation.insert(public(&s.keys, path), source(path));
            psbt.outputs.push(meta);
        });
        assert_eq!(review(&s, payload).change_index, Some(7));
    }
}

mod fees {
    use super::*;

    fn with_outputs(s: &Setup, sent: u64, change: u64) -> Vec<u8> {
        let inputs = [utxo(s, SEGWIT, 1_000_000, 1)];
        let change = (change > 0).then_some((CHANGE, change));
        build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, sent)], change).unwrap()
    }

    #[test]
    fn outputs_above_inputs_are_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[0].value = Amount::from_sat(400_001 + 1_000);
        });
        assert_eq!(forbidden(review(&s, payload.clone())), Some(Forbid::ParseFailed));
        assert_eq!(sign(&s, Network::Bitcoin, payload).unwrap_err(), Error::Refused);
    }

    #[test]
    fn output_overflow_is_refused() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[0].value = Amount::from_sat(u64::MAX);
            psbt.unsigned_tx.output[1].value = Amount::from_sat(2);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn amounts_above_the_supply_are_refused() {
        let s = setup();
        // 输入和输出一起放大,手续费看起来正常,只有总量上限能发现.
        let payload = forged(&taproot_send(&s), |psbt| {
            psbt.inputs[0].witness_utxo.as_mut().unwrap().value = Amount::MAX_MONEY + Amount::from_sat(1_000_000);
            psbt.unsigned_tx.output[0].value = Amount::MAX_MONEY + Amount::from_sat(1);
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn fee_at_five_percent_passes_and_one_sat_more_alerts() {
        let s = setup();
        // 手续费 25000,发送额 500000,正好 5%.
        assert_eq!(review(&s, with_outputs(&s, 500_000, 475_000)).confirm, Confirm::Tap);
        // 手续费 25001,发送额 500000.
        let high = review(&s, with_outputs(&s, 500_000, 474_999));
        assert_eq!((high.risk, high.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(high.alerts, vec!["手续费明显偏高"]);
    }

    #[test]
    fn fee_above_the_ceiling_alerts_even_below_the_percentage() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 10_000_000, 1)];
        let at = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 9_900_000)], None).unwrap();
        assert_eq!(review(&s, at).confirm, Confirm::Tap);
        let over = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 9_899_999)], None).unwrap();
        assert_eq!(review(&s, over).alerts, vec!["手续费明显偏高"]);
    }

    #[test]
    fn fee_rate_is_in_sat_per_vbyte() {
        let s = setup();
        let (req, response, review) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        // 手续费 1000 sat.估算的大小要和真实签名后的交易一致.
        let tx: Transaction = deserialize(&btc::verify(Network::Bitcoin, &req, &response).unwrap().raw).unwrap();
        assert_eq!(tx.vsize(), 141);
        assert_eq!(
            review.rows.iter().find(|r| r.label == "费率").unwrap().value,
            "7.09 sat/vB"
        );
    }
}

mod display {
    use super::*;

    #[test]
    fn one_external_output_sets_amount_and_counterparty() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!(amount.symbol.as_deref(), Some("BTC"));
        assert_eq!(amount.asset.as_deref(), Some("bitcoin:native"));
        assert_eq!(review.counterparty_label.as_deref(), Some("收款地址"));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(review.actions.iter().filter(|a| a.title != "找零").count(), 1);
    }

    #[test]
    fn many_external_outputs_show_the_total_and_no_counterparty() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 1_000_000, 1)];
        let payments = [pay(PEER, 100_000), pay(PEER_2, 200_000)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &payments, Some((CHANGE, 699_000))).unwrap();
        let review = review(&s, payload);
        assert_eq!(review.amount.as_ref().unwrap().text, "0.003");
        assert_eq!(review.counterparty, None);
        assert_eq!(review.counterparty_label, None);
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(review.alerts, vec!["这笔交易有 2 个收款方,请逐个核对"]);
        assert_eq!(review.actions.len(), 3);
        assert!(review.actions.iter().all(|a| a.risk == Risk::Transfer));
        assert!(review.actions[0].rows.iter().any(|r| r.value == PEER));
        assert!(review.actions[1].rows.iter().any(|r| r.value == PEER_2));
        assert_eq!(review.actions[2].title, "找零");
    }

    #[test]
    fn everything_coming_back_is_a_transfer_to_myself() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 1_000_000, 1)];
        let payload = forged(
            &build_psbt(
                Network::Bitcoin,
                &s.keys,
                &inputs,
                &[pay(PEER, 400_000)],
                Some((CHANGE, 599_000)),
            )
            .unwrap(),
            |psbt| {
                psbt.unsigned_tx.output.remove(0);
                psbt.outputs.remove(0);
            },
        );
        let review = review(&s, payload);
        assert_eq!(review.risk, Risk::Transfer);
        assert_eq!(review.actions[0].title, "转给自己");
        assert_eq!(review.actions[1].title, "找零");
        assert_eq!(review.counterparty, None);
        // 发送额是 0,任何手续费都超过发送额的 5%.
        assert_eq!(review.alerts, vec!["手续费明显偏高"]);
    }

    /// 取消交易没有收款方,全部回到找零.
    #[test]
    fn cancel_without_a_payment_is_a_self_transfer() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 1_000_000, 1)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[], Some((CHANGE, 999_000))).unwrap();
        let review = review(&s, payload);
        assert_eq!(review.risk, Risk::Transfer);
        assert_eq!(review.actions[0].title, "转给自己");
        assert_eq!(review.counterparty, None);
    }

    #[test]
    fn a_transaction_needs_at_least_one_output() {
        let s = setup();
        let result = build_psbt(Network::Bitcoin, &s.keys, &[utxo(&s, SEGWIT, 1_000_000, 1)], &[], None);
        assert_eq!(result, Err(CoreError::InvalidParams));
    }

    #[test]
    fn dust_outputs_are_refused_when_building() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 1_000_000, 1)];
        let dusty = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 293)], None);
        assert_eq!(dusty, Err(CoreError::Dust));
        assert!(build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 294)], None).is_ok());
        let change = build_psbt(
            Network::Bitcoin,
            &s.keys,
            &inputs,
            &[pay(PEER, 1000)],
            Some((CHANGE, 100)),
        );
        assert_eq!(change, Err(CoreError::Dust));
    }

    #[test]
    fn rows_follow_the_field_table() {
        let s = setup();
        let review = review(&s, mixed_send(&s));
        let labels: Vec<&str> = review.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "网络",
                "付款账户",
                "派生路径",
                "输入金额",
                "付款账户",
                "派生路径",
                "输入金额",
                "地址类型",
                "手续费",
                "费率",
                "sighash"
            ]
        );
        assert_eq!(review.rows[0].value, "Bitcoin");
        assert_eq!(review.rows[7].value, "原生 SegWit,Taproot");
        assert_eq!(review.rows[8].value, "0.00001 BTC");
        assert_eq!(review.rows[10].value, "ALL,DEFAULT");
        assert!(!review.raw.is_empty());
    }

    #[test]
    fn non_address_output_is_shown_raw_and_alerted() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[0].script_pubkey = ScriptBuf::new_op_return([1u8, 2, 3]);
        });
        let review = review(&s, payload);
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Hold));
        assert_eq!(review.counterparty.as_deref(), Some("6a03010203"));
        assert!(review.alerts.iter().any(|a| a.contains("销毁")));
    }

    #[test]
    fn testnet_uses_testnet_addresses() {
        let s = setup();
        let inputs = [utxo(&s, "m/84'/1'/0'/0/0", 1_000_000, 1)];
        let change = Some(("m/86'/1'/0'/1/0", 599_000));
        for network in [Network::BtcTestnet4, Network::BtcSignet, Network::BtcTestnet3] {
            let payload = build_psbt(network, &s.keys, &inputs, &[pay(TEST_PEER, 400_000)], change).unwrap();
            let review = review_with(&s, network, payload, 0);
            assert_eq!(review.confirm, Confirm::Tap);
            assert_eq!(review.counterparty.as_deref(), Some(TEST_PEER));
            assert!(review.rows[1].value.starts_with("tb1q"));
            assert_eq!(review.amount.unwrap().asset, Some(format!("{}:native", network.id())));
        }
    }
}

mod building {
    use super::*;

    #[test]
    fn inputs_signal_rbf_and_carry_their_fields() {
        let s = setup();
        let inputs = [
            utxo(&s, SEGWIT, 100_000, 1),
            utxo(&s, TAPROOT, 100_000, 2),
            utxo(&s, NESTED, 100_000, 3),
            utxo(&s, LEGACY, 100_000, 4),
        ];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 300_000)], None).unwrap();
        let psbt = Psbt::deserialize(&payload).unwrap();
        assert!(
            psbt.unsigned_tx
                .input
                .iter()
                .all(|txin| txin.sequence.to_consensus_u32() == 0xFFFF_FFFD)
        );

        let [segwit, taproot, nested, legacy] = psbt.inputs.as_slice() else {
            panic!()
        };
        assert!(segwit.non_witness_utxo.is_some() && segwit.witness_utxo.is_some());
        assert_eq!(segwit.bip32_derivation.len(), 1);

        assert!(taproot.witness_utxo.is_some());
        assert!(taproot.tap_internal_key.is_some());
        assert_eq!(taproot.tap_key_origins.len(), 1);
        assert!(taproot.bip32_derivation.is_empty());

        assert!(nested.non_witness_utxo.is_some() && nested.witness_utxo.is_some());
        assert!(nested.redeem_script.is_some());
        assert_eq!(nested.bip32_derivation.len(), 1);

        assert!(legacy.non_witness_utxo.is_some() && legacy.witness_utxo.is_none());
        assert_eq!(legacy.bip32_derivation.len(), 1);
    }

    #[test]
    fn change_output_carries_its_derivation() {
        let s = setup();
        let segwit = Psbt::deserialize(&segwit_send(&s)).unwrap();
        assert_eq!(segwit.outputs[1].bip32_derivation.len(), 1);
        assert!(segwit.outputs[0].bip32_derivation.is_empty());
        let taproot = Psbt::deserialize(&taproot_send(&s)).unwrap();
        assert_eq!(taproot.outputs[1].tap_key_origins.len(), 1);
        assert!(taproot.outputs[1].tap_internal_key.is_some());
    }

    #[test]
    fn non_taproot_input_without_a_previous_transaction_is_an_error() {
        let s = setup();
        let mut coin = utxo(&s, SEGWIT, 100_000, 1);
        coin.prev_tx = None;
        let result = build_psbt(Network::Bitcoin, &s.keys, &[coin], &[pay(PEER, 1)], None);
        assert_eq!(result, Err(CoreError::InvalidParams));
    }

    #[test]
    fn previous_transaction_must_match_the_utxo() {
        let s = setup();
        let good = utxo(&s, SEGWIT, 100_000, 1);
        let mut amount = good.clone();
        amount.amount = 99_999;
        let mut txid = good.clone();
        txid.txid = "11".repeat(32);
        let mut vout = good.clone();
        vout.vout = 1;
        let mut path = good.clone();
        path.path = SEGWIT_2.into();
        for coin in [amount, txid, vout, path] {
            let result = build_psbt(Network::Bitcoin, &s.keys, &[coin], &[pay(PEER, 1)], None);
            assert_eq!(result, Err(CoreError::InvalidParams));
        }
    }

    #[test]
    fn addresses_are_checked_against_the_network() {
        let s = setup();
        let coin = [utxo(&s, SEGWIT, 100_000, 1)];
        let wrong = build_psbt(Network::Bitcoin, &s.keys, &coin, &[pay(TEST_PEER, 1)], None);
        assert_eq!(wrong, Err(CoreError::InvalidAddress));
        let coin = [utxo(&s, "m/84'/1'/0'/0/0", 100_000, 1)];
        let wrong = build_psbt(Network::BtcSignet, &s.keys, &coin, &[pay(PEER, 1)], None);
        assert_eq!(wrong, Err(CoreError::InvalidAddress));
        let garbage = build_psbt(Network::BtcSignet, &s.keys, &coin, &[pay("not an address", 1)], None);
        assert_eq!(garbage, Err(CoreError::InvalidAddress));
    }

    #[test]
    fn paths_are_checked_against_the_network() {
        let s = setup();
        let main = [utxo(&s, SEGWIT, 100_000, 1)];
        let result = build_psbt(Network::BtcTestnet4, &s.keys, &main, &[pay(TEST_PEER, 1)], None);
        assert_eq!(result, Err(CoreError::PathNetworkMismatch));
    }

    #[test]
    fn change_must_be_a_real_change_path() {
        let s = setup();
        let coin = [utxo(&s, SEGWIT, 100_000, 1)];
        let build = |path| {
            build_psbt(
                Network::Bitcoin,
                &s.keys,
                &coin,
                &[pay(PEER, 1_000)],
                Some((path, 2_000)),
            )
        };
        assert_eq!(build(SEGWIT_2), Err(CoreError::InvalidParams));
        assert!(build("m/84'/0'/0'/1/100000").is_ok());
        assert_eq!(build("m/84'/1'/0'/1/0"), Err(CoreError::PathNetworkMismatch));
    }

    #[test]
    fn outputs_cannot_exceed_inputs() {
        let s = setup();
        let coin = [utxo(&s, SEGWIT, 100_000, 1)];
        let result = build_psbt(Network::Bitcoin, &s.keys, &coin, &[pay(PEER, 100_001)], None);
        assert_eq!(result, Err(CoreError::InvalidParams));
    }

    #[test]
    fn empty_or_duplicate_parts_are_errors() {
        let s = setup();
        let coin = utxo(&s, SEGWIT, 100_000, 1);
        let build =
            |inputs: &[Utxo], payments: &[Payment]| build_psbt(Network::Bitcoin, &s.keys, inputs, payments, None);
        assert_eq!(build(&[], &[pay(PEER, 1)]), Err(CoreError::InvalidParams));
        assert_eq!(build(std::slice::from_ref(&coin), &[]), Err(CoreError::InvalidParams));
        assert_eq!(
            build(std::slice::from_ref(&coin), &[pay(PEER, 0)]),
            Err(CoreError::InvalidParams)
        );
        assert_eq!(
            build(&[coin.clone(), coin], &[pay(PEER, 1)]),
            Err(CoreError::InvalidParams)
        );
    }

    #[test]
    fn taproot_input_of_a_mixed_transaction_carries_its_previous_transaction() {
        let s = setup();
        let psbt = Psbt::deserialize(&mixed_send(&s)).unwrap();
        assert!(psbt.inputs.iter().all(|input| input.non_witness_utxo.is_some()));
        assert!(psbt.inputs.iter().all(|input| input.witness_utxo.is_some()));

        let mut coin = utxo(&s, TAPROOT, 500_000, 2);
        coin.prev_tx = None;
        let inputs = [utxo(&s, SEGWIT, 500_000, 1), coin];
        let result = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 1)], None);
        assert_eq!(result, Err(CoreError::InvalidParams));
    }

    #[test]
    fn built_psbt_and_decoded_psbt_agree_with_the_transaction() {
        let s = setup();
        let psbt = Psbt::deserialize(&mixed_send(&s)).unwrap();
        assert_eq!(psbt.unsigned_tx.version, transaction::Version::TWO);
        assert_eq!(psbt.unsigned_tx.lock_time, absolute::LockTime::ZERO);
        assert_eq!(psbt.fee().unwrap(), Amount::from_sat(1_000));
    }
}

mod decoded_fields {
    use super::*;
    use bitcoin::script::{Builder, PushBytesBuf};
    use bitcoin::{Opcode, opcodes};
    use bitone_core::review::FieldRow;

    fn row<'a>(review: &'a Review, name: &str) -> &'a FieldRow {
        review.fields.iter().find(|row| row.name == name).unwrap()
    }

    fn with_output_script(s: &Setup, script: ScriptBuf, sats: u64) -> Vec<u8> {
        forged(&segwit_send(s), |psbt| {
            psbt.unsigned_tx.output[0] = TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: script,
            };
        })
    }

    fn witness(version: Opcode, program: &[u8]) -> ScriptBuf {
        let pushed = PushBytesBuf::try_from(program.to_vec()).unwrap();
        Builder::new().push_opcode(version).push_slice(pushed).into_script()
    }

    #[test]
    fn the_phone_gets_the_generic_list_with_the_summary_on_top() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        assert_eq!(review.field_count as usize, review.fields.len());
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(row(&review, "fee").value, "0.00001 BTC");
        assert_eq!(row(&review, "fee_rate").value, "7.09 sat/vB");
        assert!(review.review_digest != [0; 32]);
    }

    #[test]
    fn sequence_version_and_locktime_are_shown_with_their_meaning() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        let version = row(&review, "version");
        assert_eq!(version.value, "2");
        assert_eq!(version.note.as_deref(), Some("支持相对锁定(BIP-68)"));
        let sequence = row(&review, "sequence");
        assert_eq!(sequence.value, "4294967293");
        assert_eq!(sequence.note.as_deref(), Some("可替换(BIP-125)"));
        let lock_time = row(&review, "lock_time");
        assert_eq!(lock_time.value, "0");
        assert_eq!(lock_time.note.as_deref(), Some("无锁定"));
    }

    #[test]
    fn a_lock_with_every_input_final_says_it_has_no_effect() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.lock_time = absolute::LockTime::from_consensus(800_000);
            psbt.unsigned_tx.input[0].sequence = Sequence::MAX;
        });
        let review = review(&s, payload);
        let note = row(&review, "lock_time").note.clone().unwrap();
        assert!(note.contains("不生效"), "{note}");
    }

    #[test]
    fn truc_and_other_versions_alert() {
        let s = setup();
        for (version, word) in [(3, "TRUC"), (7, "不是标准值")] {
            let payload = forged(&segwit_send(&s), |psbt| {
                psbt.unsigned_tx.version = transaction::Version(version);
            });
            let review = review(&s, payload);
            assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
            assert!(review.alerts[0].contains(word), "{}", review.alerts[0]);
        }
    }

    #[test]
    fn own_rows_carry_the_mark_and_the_path() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        let own: Vec<_> = review.fields.iter().filter(|row| row.own).collect();
        assert_eq!(own.len(), 2);
        assert_eq!((own[0].name.as_str(), own[0].value.as_str()), ("input", SEGWIT));
        assert_eq!((own[1].name.as_str(), own[1].value.as_str()), ("output", CHANGE));
    }

    #[test]
    fn op_return_payload_is_text_and_hex() {
        let s = setup();
        let review = review(&s, with_output_script(&s, ScriptBuf::new_op_return(b"gm"), 0));
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Hold));
        assert_eq!(row(&review, "text").value, "gm");
        assert_eq!(row(&review, "data").value, "676d");
    }

    #[test]
    fn a_zero_amount_op_return_adds_no_alert_of_its_own() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[0] = TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_op_return(b"gm"),
            };
            psbt.unsigned_tx.output[1].value = Amount::from_sat(980_000);
        });
        let review = review(&s, payload);
        // 发送额是 0,任何手续费都超过发送额的 5%.
        assert_eq!(review.alerts, vec!["手续费明显偏高"]);
        assert_eq!(review.risk, Risk::Recognized);
    }

    #[test]
    fn witness_versions_two_to_sixteen_are_unknown_and_never_a_tap() {
        let s = setup();
        for opcode in opcodes::all::OP_PUSHNUM_2.to_u8()..=opcodes::all::OP_PUSHNUM_16.to_u8() {
            let script = witness(Opcode::from(opcode), &[1; 32]);
            let review = review(&s, with_output_script(&s, script, 400_000));
            assert_eq!(
                (review.risk, review.confirm),
                (Risk::Unknown, Confirm::Hold),
                "{opcode}"
            );
            assert_eq!(review.title, "无法识别");
            assert_ne!(review.actions[0].title, "普通转账");
            assert!(row(&review, "witness_version").source == bitone_core::decode::SourceKind::Derived);
            assert!(row(&review, "script").value.contains("OP_PUSHBYTES_32"));
            assert!(review.alerts.iter().any(|alert| alert.contains("见证版本")));
        }
    }

    #[test]
    fn bad_witness_lengths_and_other_scripts_are_unknown_and_shown_as_opcodes() {
        let s = setup();
        for script in [
            witness(opcodes::all::OP_PUSHNUM_1, &[1; 31]),
            witness(opcodes::all::OP_PUSHBYTES_0, &[1; 19]),
            ScriptBuf::from_bytes(vec![opcodes::all::OP_NOP.to_u8()]),
        ] {
            let review = review(&s, with_output_script(&s, script, 400_000));
            assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
            assert!(row(&review, "script").value.contains("OP_"));
        }
    }

    #[test]
    fn outputs_with_no_address_form_are_recognized_with_an_alert() {
        let s = setup();
        let key = bitcoin::PublicKey::new(public(&s.other, SEGWIT));
        let multisig = Builder::new()
            .push_opcode(opcodes::all::OP_PUSHNUM_1)
            .push_key(&key)
            .push_opcode(opcodes::all::OP_PUSHNUM_1)
            .push_opcode(opcodes::all::OP_CHECKMULTISIG)
            .into_script();
        for (script, word) in [
            (ScriptBuf::new_p2pk(&key), "P2PK"),
            (multisig, "裸多签"),
            (witness(opcodes::all::OP_PUSHNUM_1, &[0x4e, 0x73]), "P2A"),
        ] {
            let review = review(&s, with_output_script(&s, script, 400_000));
            assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Hold));
            assert!(review.alerts[0].contains(word), "{}", review.alerts[0]);
        }
    }

    #[test]
    fn an_unknown_psbt_key_is_unknown_and_signable() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| {
            let key = bitcoin::psbt::raw::Key {
                type_value: 0x70,
                key: vec![1],
            };
            psbt.inputs[0].unknown.insert(key, vec![2]);
        });
        let shown = review(&s, payload.clone());
        assert_eq!((shown.risk, shown.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(sign(&s, Network::Bitcoin, payload).is_ok());
    }

    #[test]
    fn an_input_of_this_wallet_on_an_unknown_script_cannot_be_signed() {
        let s = setup();
        let key = public(&s.keys, SEGWIT);
        let prev = funding(ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::all_zeros()), 1_000_000, 9);
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = psbt.inputs[0].bip32_derivation.values().next().unwrap().clone();
            psbt.unsigned_tx.input[0].previous_output.txid = prev.compute_txid();
            psbt.inputs[0] = Input {
                non_witness_utxo: Some(prev.clone()),
                bip32_derivation: [(key, source)].into(),
                ..Input::default()
            };
        });
        assert_eq!(forbidden(review(&s, payload)), Some(Forbid::Unsupported));
    }

    #[test]
    fn a_foreign_input_on_an_unknown_script_is_only_shown() {
        let s = setup();
        let prev = funding(ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::all_zeros()), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            add_foreign(psbt, foreign_with_prev_tx(&prev), &prev);
            psbt.unsigned_tx.output[0].value = Amount::from_sat(650_000);
        });
        assert_eq!(review(&s, payload).alerts, vec!["有 1 个输入不属于本钱包"]);
    }

    #[test]
    fn an_undecodable_psbt_shows_the_reason_and_the_digest() {
        let s = setup();
        let mut payload = segwit_send(&s);
        payload.push(0);
        let shown = review(&s, payload);
        assert_eq!(shown.forbidden, Some(Forbid::ParseFailed));
        assert!(shown.alerts[0].contains("SHA-256"), "{}", shown.alerts[0]);
    }

    #[test]
    fn a_payload_that_is_not_a_psbt_cannot_be_signed() {
        let s = setup();
        assert_eq!(forbidden(review(&s, vec![1, 2, 3])), Some(Forbid::ParseFailed));
    }

    #[test]
    fn signed_and_context_rows_have_ranges_and_derived_rows_do_not() {
        let s = setup();
        let review = review(&s, segwit_send(&s));
        for row in &review.fields {
            let ranged = matches!(
                row.source,
                bitone_core::decode::SourceKind::Signed | bitone_core::decode::SourceKind::Context
            );
            assert_eq!(row.offset.is_some(), ranged, "{}", row.name);
        }
    }
}

mod unknown_networks {
    use super::*;

    const LOCAL: &str = "local-debug";

    fn local(s: &Setup, payload: Vec<u8>, chain: Option<u8>) -> SignRequest {
        SignRequest {
            network: LOCAL.into(),
            chain,
            ..request(s, Network::Bitcoin, payload)
        }
    }

    fn reviewed(s: &Setup, req: &SignRequest) -> Review {
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange::default(),
        };
        tx::review(req, &ctx)
    }

    fn signed(s: &Setup, req: &SignRequest) -> Result<(SignResponse, Review), Error> {
        s.wallet
            .sign(req, &s.keys, Manifest::builtin(), HighestChange::default())
    }

    #[test]
    fn a_psbt_on_an_unknown_network_is_detected_from_the_payload_and_signable() {
        let s = setup();
        let req = local(&s, segwit_send(&s), None);
        let shown = reviewed(&s, &req);
        assert_eq!((shown.risk, shown.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(shown.network, LOCAL);
        assert!(shown.alerts[0].contains("不认识这个网络"), "{}", shown.alerts[0]);
        assert_eq!(
            shown.fields.iter().find(|row| row.name == "network").unwrap().value,
            LOCAL
        );
        let known = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap().1;
        let (response, _) = signed(&s, &req).unwrap();
        assert_eq!(response.signatures, known.signatures);
    }

    #[test]
    fn the_chain_field_decides_when_it_is_present() {
        let s = setup();
        let bitcoin = local(&s, segwit_send(&s), Some(0));
        assert_eq!(reviewed(&s, &bitcoin).risk, Risk::Unknown);
        // 编号 1 是 EVM,PSBT 没有路径可以签.200 这个版本不认识.
        for (chain, reason) in [(1, Forbid::PathNotAllowed), (200, Forbid::UnknownNetwork)] {
            let other = local(&s, segwit_send(&s), Some(chain));
            assert_eq!(forbidden(reviewed(&s, &other)), Some(reason), "{chain}");
            assert_eq!(signed(&s, &other).unwrap_err(), Error::Refused);
        }
    }

    #[test]
    fn a_payload_without_the_magic_stays_refused() {
        let s = setup();
        let garbled = local(&s, vec![1, 2, 3], None);
        assert_eq!(forbidden(reviewed(&s, &garbled)), Some(Forbid::UnknownNetwork));
        let declared = local(&s, vec![1, 2, 3], Some(0));
        assert_eq!(forbidden(reviewed(&s, &declared)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn paths_of_any_kind_of_key_are_not_contradictions_here() {
        let s = setup();
        let inputs = [utxo(&s, "m/84'/1'/0'/0/0", 1_000_000, 1)];
        let payload = build_psbt(Network::BtcTestnet4, &s.keys, &inputs, &[pay(TEST_PEER, 400_000)], None).unwrap();
        let shown = reviewed(&s, &local(&s, payload, None));
        assert_eq!((shown.risk, shown.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(shown.counterparty.as_deref(), Some(TEST_PEER));
    }

    #[test]
    fn the_change_index_goes_to_the_slot_of_the_key() {
        let s = setup();
        let inputs = [utxo(&s, "m/84'/1'/0'/0/0", 1_000_000, 1)];
        let change = Some(("m/84'/1'/0'/1/3", 599_000));
        let payload = build_psbt(
            Network::BtcTestnet4,
            &s.keys,
            &inputs,
            &[pay(TEST_PEER, 400_000)],
            change,
        )
        .unwrap();
        let shown = reviewed(&s, &local(&s, payload, None));
        assert_eq!((shown.change_index, shown.change_is_testnet), (Some(3), true));
    }

    #[test]
    fn the_network_name_is_escaped_for_display() {
        let s = setup();
        let mut req = local(&s, segwit_send(&s), None);
        req.network = "evil\u{202E}net".into();
        assert_eq!(reviewed(&s, &req).network, "evil\\u{202e}net");
    }
}

mod version_two {
    use super::*;
    use psbt_v2::v2;

    /// 同一笔交易的 PSBT v2.
    fn v2_of(payload: &[u8], modifiable: u8) -> Vec<u8> {
        let psbt = Psbt::deserialize(payload).unwrap();
        let global = v2::Global {
            version: psbt_v2::V2,
            tx_version: psbt.unsigned_tx.version,
            fallback_lock_time: None,
            tx_modifiable_flags: modifiable,
            input_count: psbt.inputs.len(),
            output_count: psbt.outputs.len(),
            xpubs: Default::default(),
            proprietaries: Default::default(),
            unknowns: Default::default(),
        };
        let inputs = psbt.unsigned_tx.input.iter().zip(&psbt.inputs).map(|(txin, input)| {
            let mut map = v2::Input::new(&txin.previous_output);
            map.sequence = Some(txin.sequence);
            map.non_witness_utxo = input.non_witness_utxo.clone();
            map.witness_utxo = input.witness_utxo.clone();
            map.redeem_script = input.redeem_script.clone();
            map.tap_internal_key = input.tap_internal_key;
            map.tap_key_origins = input.tap_key_origins.clone();
            let keys = input.bip32_derivation.iter();
            map.bip32_derivations = keys.map(|(key, source)| ((*key).into(), source.clone())).collect();
            map
        });
        let outputs = psbt.unsigned_tx.output.iter().zip(&psbt.outputs).map(|(out, meta)| {
            let mut map = v2::Output::new(out.clone());
            map.redeem_script = meta.redeem_script.clone();
            map.tap_internal_key = meta.tap_internal_key;
            map.tap_key_origins = meta.tap_key_origins.clone();
            let keys = meta.bip32_derivation.iter();
            map.bip32_derivations = keys.map(|(key, source)| ((*key).into(), source.clone())).collect();
            map
        });
        v2::Psbt {
            global,
            inputs: inputs.collect(),
            outputs: outputs.collect(),
        }
        .serialize()
    }

    #[test]
    fn v2_reviews_like_v0() {
        let s = setup();
        let review = review(&s, v2_of(&segwit_send(&s), 0));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(review.change_index, Some(3));
        assert!(review.fields.iter().any(|row| row.own && row.name == "psbt_input"));
        assert!(review.fields.iter().any(|row| row.own && row.name == "psbt_output"));
        assert!(review.fields.iter().any(|row| row.name == "output_index"));
    }

    #[test]
    fn v2_signs_the_same_bytes_as_v0_and_verifies() {
        let s = setup();
        for payload in [segwit_send(&s), taproot_send(&s), mixed_send(&s)] {
            let (_, first, _) = sign(&s, Network::Bitcoin, payload.clone()).unwrap();
            let (req, second, _) = sign(&s, Network::Bitcoin, v2_of(&payload, 0)).unwrap();
            assert_eq!(first.signatures, second.signatures);
            assert!(btc::verify(Network::Bitcoin, &req, &second).is_ok());
        }
    }

    #[test]
    fn a_modifiable_v2_is_signable_with_an_alert() {
        let s = setup();
        let payload = v2_of(&segwit_send(&s), 3);
        let shown = review(&s, payload.clone());
        assert_eq!((shown.risk, shown.confirm), (Risk::Transfer, Confirm::Hold));
        assert!(shown.alerts[0].contains("修改"), "{}", shown.alerts[0]);
        assert!(sign(&s, Network::Bitcoin, payload).is_ok());
    }

    #[test]
    fn v2_keeps_the_rules_of_v0() {
        let s = setup();
        let stripped = forged(&segwit_send(&s), |psbt| psbt.inputs[0].non_witness_utxo = None);
        let shown = review(&s, v2_of(&stripped, 0));
        assert_eq!((shown.risk, shown.confirm), (Risk::Unknown, Confirm::Hold));
        let bad_change = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.output[1].script_pubkey = script(&s.other, CHANGE);
        });
        assert_eq!(
            forbidden(review(&s, v2_of(&bad_change, 0))),
            Some(Forbid::AddressMismatch)
        );
    }
}

mod signing {
    use super::*;

    fn raw_tx(bytes: &[u8]) -> Transaction {
        deserialize(bytes).unwrap()
    }

    #[test]
    fn segwit_signature_is_der_with_a_sighash_byte() {
        let s = setup();
        let (req, response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        let signature = response.signatures[0].as_slice();
        assert_eq!(signature[0], 0x30);
        assert_eq!(*signature.last().unwrap(), 0x01);
        let verified = btc::verify(Network::Bitcoin, &req, &response).unwrap();
        let tx = raw_tx(&verified.raw);
        assert_eq!(verified.hash, tx.compute_txid().to_string());
        assert_eq!(tx.input[0].witness.len(), 2);
        assert!(tx.input[0].script_sig.is_empty());
    }

    #[test]
    fn taproot_signature_is_64_bytes_by_default() {
        let s = setup();
        let (req, response, _) = sign(&s, Network::Bitcoin, taproot_send(&s)).unwrap();
        assert_eq!(response.signatures[0].len(), 64);
        let tx = raw_tx(&btc::verify(Network::Bitcoin, &req, &response).unwrap().raw);
        assert_eq!(tx.input[0].witness.len(), 1);
    }

    #[test]
    fn taproot_all_has_a_trailing_byte() {
        let s = setup();
        let payload = forged(&taproot_send(&s), |psbt| psbt.inputs[0].sighash_type = sighash(1));
        let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        assert_eq!(response.signatures[0].len(), 65);
        assert_eq!(response.signatures[0][64], 1);
        assert!(btc::verify(Network::Bitcoin, &req, &response).is_ok());
    }

    #[test]
    fn signatures_are_deterministic() {
        let s = setup();
        for payload in [segwit_send(&s), taproot_send(&s)] {
            let (_, first, _) = sign(&s, Network::Bitcoin, payload.clone()).unwrap();
            let (_, second, _) = sign(&s, Network::Bitcoin, payload).unwrap();
            assert_eq!(first.signatures, second.signatures);
        }
    }

    #[test]
    fn mixed_inputs_verify_and_finalize() {
        let s = setup();
        let (req, response, _) = sign(&s, Network::Bitcoin, mixed_send(&s)).unwrap();
        assert_eq!(response.signatures[1].len(), 64);
        let tx = raw_tx(&btc::verify(Network::Bitcoin, &req, &response).unwrap().raw);
        assert_eq!((tx.input[0].witness.len(), tx.input[1].witness.len()), (2, 1));
    }

    #[test]
    fn nested_and_legacy_inputs_verify_and_finalize() {
        let s = setup();
        let inputs = [utxo(&s, NESTED, 500_000, 1), utxo(&s, LEGACY, 500_000, 2)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 900_000)], None).unwrap();
        let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        let tx = raw_tx(&btc::verify(Network::Bitcoin, &req, &response).unwrap().raw);
        assert_eq!(tx.input[0].script_sig.len(), 23);
        assert_eq!(tx.input[0].witness.len(), 2);
        assert!(tx.input[1].script_sig.len() > 100);
        assert_eq!(tx.input[1].witness.len(), 0);
    }

    #[test]
    fn non_default_sighash_is_signed_as_declared() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.inputs[0].sighash_type = sighash(2));
        let (req, response, review) = sign(&s, Network::Bitcoin, payload).unwrap();
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(*response.signatures[0].last().unwrap(), 0x02);
        assert!(btc::verify(Network::Bitcoin, &req, &response).is_ok());
    }

    #[test]
    fn foreign_input_gets_an_empty_placeholder() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let witness = {
            let mut w = Witness::new();
            w.push([1u8; 71]);
            w.push([2u8; 33]);
            w
        };
        // 其他钱包的输入排在前面,签名下标不能错位.
        let payload = forged(&segwit_send(&s), |psbt| {
            psbt.unsigned_tx.input.insert(
                0,
                TxIn {
                    previous_output: OutPoint {
                        txid: prev.compute_txid(),
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
            );
            let mut input = foreign_with_prev_tx(&prev);
            input.final_script_witness = Some(witness.clone());
            psbt.inputs.insert(0, input);
        });
        let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        assert_eq!(response.signatures.len(), 2);
        assert!(response.signatures[0].is_empty());
        assert!(!response.signatures[1].is_empty());
        let tx = raw_tx(&btc::verify(Network::Bitcoin, &req, &response).unwrap().raw);
        assert_eq!(tx.input[0].witness, witness);
        assert_eq!(tx.input[1].witness.len(), 2);
    }

    #[test]
    fn foreign_input_must_already_be_final() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            add_foreign(psbt, foreign_with_prev_tx(&prev), &prev)
        });
        let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn signature_on_a_foreign_input_is_rejected() {
        let s = setup();
        let prev = funding(script(&s.other, SEGWIT), 250_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            let mut input = foreign_with_prev_tx(&prev);
            input.final_script_witness = Some(Witness::new());
            add_foreign(psbt, input, &prev);
        });
        let (req, mut response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        response.signatures[1] = vec![1u8; 64].into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn any_bad_signature_fails_verification() {
        let s = setup();
        for payload in [segwit_send(&s), taproot_send(&s), mixed_send(&s)] {
            let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
            for index in 0..response.signatures.len() {
                let mut bad = response.clone();
                let mut bytes = bad.signatures[index].to_vec();
                bytes[10] ^= 1;
                bad.signatures[index] = bytes.into();
                assert_eq!(btc::verify(Network::Bitcoin, &req, &bad), Err(CoreError::BadSignature));
            }
        }
    }

    #[test]
    fn signatures_cannot_swap_inputs() {
        let s = setup();
        let inputs = [utxo(&s, SEGWIT, 500_000, 1), utxo(&s, SEGWIT_2, 500_000, 2)];
        let payload = build_psbt(Network::Bitcoin, &s.keys, &inputs, &[pay(PEER, 900_000)], None).unwrap();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        response.signatures.swap(0, 1);
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn wrong_signature_count_is_rejected() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, mixed_send(&s)).unwrap();
        response.signatures.pop();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn empty_signature_on_my_input_is_rejected() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        response.signatures[0] = Vec::new().into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn request_id_must_match() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        response.id = [1; 16];
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::ResponseMismatch)
        );
    }

    #[test]
    fn signature_is_bound_to_the_built_transaction() {
        let s = setup();
        let (mut req, response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        req.payload = forged(&req.payload, |psbt| {
            psbt.unsigned_tx.output[0].script_pubkey = script(&s.other, SEGWIT);
        });
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn sighash_byte_must_match_the_psbt() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        let mut bytes = response.signatures[0].to_vec();
        *bytes.last_mut().unwrap() = 0x02;
        response.signatures[0] = bytes.into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn taproot_default_with_a_zero_tail_is_rejected() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, taproot_send(&s)).unwrap();
        let mut bytes = response.signatures[0].to_vec();
        bytes.push(0);
        response.signatures[0] = bytes.into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn high_s_signature_is_rejected() {
        let s = setup();
        let (req, mut response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        let bytes = response.signatures[0].to_vec();
        let parsed = ecdsa::Signature::from_der(&bytes[..bytes.len() - 1]).unwrap();
        let mut compact = parsed.serialize_compact();
        // n - s.
        let order = bitcoin::secp256k1::constants::CURVE_ORDER;
        let mut borrow = 0i16;
        for i in (32..64).rev() {
            let value = order[i - 32] as i16 - compact[i] as i16 - borrow;
            compact[i] = value.rem_euclid(256) as u8;
            borrow = i16::from(value < 0);
        }
        let high = ecdsa::Signature::from_compact(&compact).unwrap();
        let mut der = high.serialize_der().to_vec();
        der.push(1);
        response.signatures[0] = der.into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn verify_refuses_negative_fees_before_looking_at_signatures() {
        let s = setup();
        let (mut req, response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        req.payload = forged(&req.payload, |psbt| {
            psbt.unsigned_tx.output[0].value = Amount::from_sat(2_000_000);
        });
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::InvalidParams)
        );
    }

    /// 手机能签手续费无法确认的 PSBT,所以核验也要放行,否则用户签完了广播不出去.
    #[test]
    fn a_signed_psbt_with_an_unconfirmed_fee_verifies_and_says_so() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.inputs[0].non_witness_utxo = None);
        let (req, response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        let verified = btc::verify(Network::Bitcoin, &req, &response).unwrap();
        assert!(verified.fee_unconfirmed);
        assert!(!verified.signature_only);
    }

    #[test]
    fn a_confirmed_fee_verifies_without_the_mark() {
        let s = setup();
        let (req, response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        assert!(!btc::verify(Network::Bitcoin, &req, &response).unwrap().fee_unconfirmed);
    }

    #[test]
    fn an_unconfirmed_fee_still_needs_a_good_signature() {
        let s = setup();
        let payload = forged(&segwit_send(&s), |psbt| psbt.inputs[0].non_witness_utxo = None);
        let (req, mut response, _) = sign(&s, Network::Bitcoin, payload).unwrap();
        response.signatures[0] = vec![0; 71].into();
        assert_eq!(
            btc::verify(Network::Bitcoin, &req, &response),
            Err(CoreError::BadSignature)
        );
    }

    #[test]
    fn tx_module_routes_to_bitcoin() {
        let s = setup();
        let (req, response, _) = sign(&s, Network::Bitcoin, segwit_send(&s)).unwrap();
        let verified = tx::verify(&req, &response, &[]).unwrap();
        assert_eq!(verified, btc::verify(Network::Bitcoin, &req, &response).unwrap());
    }

    #[test]
    fn testnet_keys_sign_testnet_transactions() {
        let s = setup();
        let inputs = [
            utxo(&s, "m/84'/1'/0'/0/0", 500_000, 1),
            utxo(&s, "m/86'/1'/0'/0/0", 500_000, 2),
        ];
        let payload = build_psbt(Network::BtcTestnet4, &s.keys, &inputs, &[pay(TEST_PEER, 900_000)], None).unwrap();
        let (req, response, _) = sign(&s, Network::BtcTestnet4, payload).unwrap();
        assert!(btc::verify(Network::BtcTestnet4, &req, &response).is_ok());
    }

    #[test]
    fn stale_public_keys_cannot_sign_with_another_wallet() {
        // 换了 BIP39 密码,本机保存的公钥还是旧钱包的.核对时第二个输入是"其他钱包",
        // 用户没有确认过花它,解锁的钱包却能签.
        let s = setup();
        let unlocked = Wallet::new(&[0u8; 16], "TREZOR").unwrap();
        let unlocked_keys = PublicKeys::decode(&unlocked.public_keys(false).unwrap()).unwrap();
        let prev = funding(script(&unlocked_keys, SEGWIT), 1_000_000, 7);
        let payload = forged(&segwit_send(&s), |psbt| {
            let source = (
                Fingerprint::from(unlocked_keys.fingerprint),
                BipPath::from_str(SEGWIT).unwrap(),
            );
            let mut input = foreign_with_prev_tx(&prev);
            input.bip32_derivation.insert(public(&unlocked_keys, SEGWIT), source);
            add_foreign(psbt, input, &prev);
        });
        let req = request(&s, Network::Bitcoin, payload);
        let result = unlocked.sign(&req, &s.keys, Manifest::builtin(), HighestChange::default());
        assert_eq!(result.unwrap_err(), Error::Refused);
    }
}

//! Bitcoin 交易:构建 PSBT,核对,核验.见 docs/04-security.md 第 2.6,2.7 节.
//!
//! PSBT 来自网页端,里面的声明都不可信.`prepare` 只查第 1.1 节的拒绝情况,它是核对,签名,核验共用的入口,
//! 三处看到的输入不可能不一致.`review` 把树交给策略,路径和地址靠重新派生,金额靠前序交易的 txid.

use crate::decode::{self, NodeId, PsbtWalker, ScriptClass, address_network, parse};
use crate::keys::PublicKeys;
use crate::manifest::Asset;
use crate::path::{
    DerivationPath, bitcoin_path_mode, check_bitcoin_path, classify, is_usual_bitcoin_path, validate_for_network,
};
use crate::policy::{self, BitcoinFacts, OutputFact, OwnEntry, SpendFact};
use crate::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use crate::review::{AmountView, Forbid, Review, Summary};
use crate::tx::{ReviewContext, Verified};
use crate::units::format_units;
use crate::{AccountKind, Chain, Error, Mode, Network};
use bitcoin::address::NetworkUnchecked;
use bitcoin::bip32::{ChildNumber, DerivationPath as BipPath, Fingerprint, KeySource};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::hex::DisplayHex;
use bitcoin::key::{CompressedPublicKey, TapTweak};
use bitcoin::psbt::{Input, Output, Psbt};
use bitcoin::script::{Builder, PushBytesBuf};
use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, Verification, XOnlyPublicKey};
use bitcoin::sighash::{EcdsaSighashType, Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot::TapLeafHash;
use bitcoin::{
    Address, Amount, OutPoint, Script, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute, ecdsa,
    taproot, transaction,
};
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

/// 交易开销:版本,锁定时间,输入输出计数,加 SegWit 标记.单位是重量.
const OVERHEAD_WEIGHT: u64 = 42;
/// 每个输入固定的 41 字节:outpoint,脚本长度,sequence.
const INPUT_WEIGHT: u64 = 41 * 4;
/// `select_coins` 不知道收款地址的类型,按最长的标准输出估算,宁可多留手续费.
const PAYMENT_WEIGHT: u64 = 43 * 4;

/// 一个可花费的输出.`prev_tx` 是前序交易的原始字节,只有全 Taproot 的交易可以不带.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub amount: u64,
    pub path: String,
    #[serde(skip)]
    pub prev_tx: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    pub address: String,
    pub amount: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub inputs: Vec<Utxo>,
    pub fee: u64,
    pub change: u64,
}

/// 输入声明的 sighash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sighash {
    Ecdsa(EcdsaSighashType),
    Taproot(TapSighashType),
}

impl Sighash {
    /// 签名承诺全部输入和全部输出.
    fn commits_everything(self) -> bool {
        matches!(
            self,
            Sighash::Ecdsa(EcdsaSighashType::All) | Sighash::Taproot(TapSighashType::Default | TapSighashType::All)
        )
    }

    fn anyone_can_pay(self) -> bool {
        use {EcdsaSighashType as E, TapSighashType as T};
        matches!(
            self,
            Sighash::Ecdsa(E::AllPlusAnyoneCanPay | E::NonePlusAnyoneCanPay | E::SinglePlusAnyoneCanPay)
                | Sighash::Taproot(T::AllPlusAnyoneCanPay | T::NonePlusAnyoneCanPay | T::SinglePlusAnyoneCanPay)
        )
    }

    fn single(self) -> bool {
        use {EcdsaSighashType as E, TapSighashType as T};
        matches!(
            self,
            Sighash::Ecdsa(E::Single | E::SinglePlusAnyoneCanPay)
                | Sighash::Taproot(T::Single | T::SinglePlusAnyoneCanPay)
        )
    }

    fn label(self) -> String {
        let text = match self {
            Sighash::Ecdsa(ty) => ty.to_string(),
            Sighash::Taproot(ty) => ty.to_string(),
        };
        text.replace("SIGHASH_", "").replace('|', " + ")
    }
}

/// PSBT 声明的公钥.Taproot 只有 x 坐标.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatedKey {
    Full(PublicKey),
    XOnly(XOnlyPublicKey),
}

impl StatedKey {
    fn script(self, kind: AccountKind) -> Result<ScriptBuf, Forbid> {
        match self {
            StatedKey::Full(key) => script_for(kind, &key),
            StatedKey::XOnly(key) => Ok(p2tr(key)),
        }
    }
}

/// PSBT 里本钱包指纹对应的派生信息.
struct Origin {
    key: StatedKey,
    path: DerivationPath,
    leaves: usize,
}

/// 本钱包的输入.声明的公钥能算出输入实际的 script,路径属于本钱包要靠重新派生.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnInput {
    pub path: DerivationPath,
    pub kind: AccountKind,
    pub sighash: Sighash,
    key: StatedKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedInput {
    /// 这个输入花掉的输出,金额和 script 以它为准.
    pub prevout: TxOut,
    /// `prevout` 是否由前序交易确认.
    pub proven: bool,
    /// 不属于本钱包时是 `None`.
    pub own: Option<OwnInput>,
}

/// 声明属于本钱包的输出.声明的公钥能算出它的 script,路径属于本钱包要靠重新派生.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnOutput {
    pub path: DerivationPath,
    pub kind: AccountKind,
}

/// 解析并确认过的 PSBT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub psbt: Psbt,
    pub inputs: Vec<PreparedInput>,
    /// 和输出一一对应.
    pub outputs: Vec<Option<OwnOutput>>,
    /// 有输入的金额没有经过前序交易确认,全部签名也不能替它担保.
    pub fee_unconfirmed: bool,
}

fn bitcoin_network(network: Network) -> Option<bitcoin::Network> {
    match network {
        Network::Bitcoin => Some(bitcoin::Network::Bitcoin),
        Network::BtcTestnet4 => Some(bitcoin::Network::Testnet4),
        Network::BtcSignet => Some(bitcoin::Network::Signet),
        Network::BtcTestnet3 => Some(bitcoin::Network::Testnet),
        _ => None,
    }
}

fn p2tr(key: XOnlyPublicKey) -> ScriptBuf {
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None)
}

/// 从公钥算 scriptPubKey.Taproot 按 BIP86,没有脚本树.
pub fn script_for(kind: AccountKind, key: &PublicKey) -> Result<ScriptBuf, Forbid> {
    let compressed = CompressedPublicKey(*key);
    match kind {
        AccountKind::SegWit => Ok(ScriptBuf::new_p2wpkh(&compressed.wpubkey_hash())),
        AccountKind::Taproot => Ok(p2tr(key.x_only_public_key().0)),
        AccountKind::NestedSegWit => Ok(ScriptBuf::new_p2sh(&redeem_script(key).script_hash())),
        AccountKind::Legacy => Ok(ScriptBuf::new_p2pkh(&compressed.pubkey_hash())),
        AccountKind::Single => Err(Forbid::AddressMismatch),
    }
}

fn redeem_script(key: &PublicKey) -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&CompressedPublicKey(*key).wpubkey_hash())
}

/// 找出指纹是本钱包的派生信息.一个单签 script 只可能有一把钥匙,多于一条就不知道该用哪条.
fn origin(
    ecdsa: &BTreeMap<PublicKey, KeySource>,
    tap: &BTreeMap<XOnlyPublicKey, (Vec<TapLeafHash>, KeySource)>,
    fingerprint: [u8; 4],
) -> Result<Option<Origin>, Forbid> {
    let fingerprint = Fingerprint::from(fingerprint);
    let plain = ecdsa
        .iter()
        .filter(|(_, (f, _))| *f == fingerprint)
        .map(|(key, (_, path))| Origin {
            key: StatedKey::Full(*key),
            path: DerivationPath(path.to_u32_vec()),
            leaves: 0,
        });
    let schnorr = tap
        .iter()
        .filter(|(_, (_, (f, _)))| *f == fingerprint)
        .map(|(key, (leaves, (_, path)))| Origin {
            key: StatedKey::XOnly(*key),
            path: DerivationPath(path.to_u32_vec()),
            leaves: leaves.len(),
        });
    let mut found = plain.chain(schnorr);
    let first = found.next();
    if found.next().is_some() {
        return Err(Forbid::AddressMismatch);
    }
    Ok(first)
}

impl OwnInput {
    /// 这个输入的签名是否承诺全部输入的金额和 script.
    fn vouches_for_all(&self) -> bool {
        self.kind == AccountKind::Taproot && !self.sighash.anyone_can_pay()
    }
}

impl Origin {
    /// BIP86 的单签 key path 才是本钱包的 Taproot.
    fn check_form(&self, kind: AccountKind) -> Result<(), Forbid> {
        let schnorr = matches!(self.key, StatedKey::XOnly(_));
        if schnorr != (kind == AccountKind::Taproot) || self.leaves != 0 {
            return Err(Forbid::AddressMismatch);
        }
        Ok(())
    }
}

/// 金额和 script 优先从前序交易取,取不到时只有见证输入可以先用声明的值.
/// 返回的布尔值表示这个值有没有经过前序交易确认.
fn spent_output(txin: &TxIn, input: &Input) -> Result<(TxOut, bool), Forbid> {
    if let Some(prev) = &input.non_witness_utxo {
        if prev.compute_txid() != txin.previous_output.txid {
            return Err(Forbid::PrevTxMismatch);
        }
        let out = prev
            .output
            .get(txin.previous_output.vout as usize)
            .ok_or(Forbid::PrevTxMismatch)?;
        if input.witness_utxo.as_ref().is_some_and(|declared| declared != out) {
            return Err(Forbid::PrevTxMismatch);
        }
        return Ok((out.clone(), true));
    }
    match &input.witness_utxo {
        Some(out) if out.script_pubkey.is_witness_program() || out.script_pubkey.is_p2sh() => Ok((out.clone(), false)),
        _ => Err(Forbid::MissingPrevTx),
    }
}

fn input_sighash(input: &Input, kind: AccountKind, index: usize, outputs: usize) -> Result<Sighash, Forbid> {
    let sighash = if kind == AccountKind::Taproot {
        Sighash::Taproot(input.taproot_hash_ty().map_err(|_| Forbid::Unsupported)?)
    } else {
        Sighash::Ecdsa(input.ecdsa_hash_ty().map_err(|_| Forbid::Unsupported)?)
    };
    // SINGLE 没有对应输出时,旧式 sighash 的原文是常量 1,这个签名能花掉这把钥匙的任何币.
    if sighash.single() && index >= outputs {
        return Err(Forbid::Unsupported);
    }
    Ok(sighash)
}

/// 本钱包能签的地址类型由 script 决定,路径不设白名单,说明不了类型.
fn spend_kind(script: &Script) -> Option<AccountKind> {
    match ScriptClass::of(script) {
        ScriptClass::P2wpkh => Some(AccountKind::SegWit),
        ScriptClass::P2tr => Some(AccountKind::Taproot),
        ScriptClass::P2sh => Some(AccountKind::NestedSegWit),
        ScriptClass::P2pkh => Some(AccountKind::Legacy),
        ScriptClass::P2wsh
        | ScriptClass::P2pk
        | ScriptClass::BareMultisig
        | ScriptClass::Anchor
        | ScriptClass::OpReturn
        | ScriptClass::FutureWitness
        | ScriptClass::Other => None,
    }
}

fn own_input(origin: Origin, input: &Input, prevout: &TxOut, index: usize, outputs: usize) -> Result<OwnInput, Forbid> {
    // 不认识的脚本算不出 sighash 的原文,签名无法完成.
    let kind = spend_kind(&prevout.script_pubkey).ok_or(Forbid::Unsupported)?;
    origin.check_form(kind)?;
    match origin.key {
        StatedKey::XOnly(key) => {
            if input.tap_merkle_root.is_some() || input.tap_internal_key.is_some_and(|internal| internal != key) {
                return Err(Forbid::AddressMismatch);
            }
        }
        StatedKey::Full(key) => {
            let wrapped = kind == AccountKind::NestedSegWit;
            if wrapped
                && input
                    .redeem_script
                    .as_ref()
                    .is_some_and(|script| *script != redeem_script(&key))
            {
                return Err(Forbid::AddressMismatch);
            }
        }
    }
    let sighash = input_sighash(input, kind, index, outputs)?;
    let own = OwnInput {
        path: origin.path,
        kind,
        sighash,
        key: origin.key,
    };
    if own.key.script(kind)? != prevout.script_pubkey {
        return Err(Forbid::AddressMismatch);
    }
    Ok(own)
}

/// 带本钱包指纹的输出.声明的公钥算不出输出的 script 时拒绝,不能当成外部输出放过去.
fn own_output(
    out: &TxOut,
    meta: &Output,
    network: Option<Network>,
    fingerprint: [u8; 4],
) -> Result<Option<OwnOutput>, Forbid> {
    let Some(origin) = origin(&meta.bip32_derivation, &meta.tap_key_origins, fingerprint)? else {
        return Ok(None);
    };
    check_bitcoin_path(&origin.path, network).map_err(|_| Forbid::AddressMismatch)?;
    let kind = spend_kind(&out.script_pubkey).ok_or(Forbid::AddressMismatch)?;
    origin.check_form(kind)?;
    if meta.tap_tree.is_some() {
        return Err(Forbid::AddressMismatch);
    }
    if let (StatedKey::XOnly(key), Some(internal)) = (origin.key, meta.tap_internal_key)
        && key != internal
    {
        return Err(Forbid::AddressMismatch);
    }
    if origin.key.script(kind)? != out.script_pubkey {
        return Err(Forbid::AddressMismatch);
    }
    Ok(Some(OwnOutput {
        path: origin.path,
        kind,
    }))
}

/// 手续费能不能确认.SegWit 签名只承诺自身输入的金额,这就是 CVE-2020-14199.
/// 没有前序交易的输入不是 Taproot,或者别的输入的签名不承诺它的金额,手续费都只是声明.
fn fee_unconfirmed(inputs: &[PreparedInput]) -> bool {
    let weak = |index: usize| {
        inputs
            .iter()
            .enumerate()
            .any(|(other, input)| other != index && input.own.as_ref().is_some_and(|own| !own.vouches_for_all()))
    };
    inputs
        .iter()
        .enumerate()
        .any(|(index, input)| !input.proven && (!input.prevout.script_pubkey.is_p2tr() || weak(index)))
}

/// 解析 PSBT,找出本钱包的输入,确认每个输入的金额和 script.
///
/// 这里只看 PSBT 自身.公钥对不对由调用方负责:`review` 用保存的公钥重新派生,
/// 签名时用私钥派生,`verify` 没有账户公钥,靠签名本身.
pub fn prepare(network: impl Into<Option<Network>>, payload: &[u8], fingerprint: [u8; 4]) -> Result<Prepared, Forbid> {
    let network = network.into();
    let parsed = parse(payload).map_err(|_| Forbid::ParseFailed)?;
    let psbt = parsed.ok_or(Forbid::ParseFailed)?.psbt;
    let tx = &psbt.unsigned_tx;
    let outpoints: HashSet<OutPoint> = tx.input.iter().map(|txin| txin.previous_output).collect();
    if tx.output.is_empty() || outpoints.len() != tx.input.len() {
        return Err(Forbid::ParseFailed);
    }

    let origins = psbt
        .inputs
        .iter()
        .map(|input| origin(&input.bip32_derivation, &input.tap_key_origins, fingerprint))
        .collect::<Result<Vec<_>, _>>()?;
    if origins.iter().all(Option::is_none) {
        return Err(Forbid::AddressMismatch);
    }

    let mut inputs = Vec::with_capacity(tx.input.len());
    for (index, ((txin, input), origin)) in tx.input.iter().zip(&psbt.inputs).zip(origins).enumerate() {
        if let Some(origin) = &origin {
            check_bitcoin_path(&origin.path, network).map_err(|_| Forbid::PathNetworkMismatch)?;
        }
        let (prevout, proven) = spent_output(txin, input)?;
        let own = origin
            .map(|origin| own_input(origin, input, &prevout, index, tx.output.len()))
            .transpose()?;
        inputs.push(PreparedInput { prevout, proven, own });
    }
    let outputs = tx
        .output
        .iter()
        .zip(&psbt.outputs)
        .map(|(out, meta)| own_output(out, meta, network, fingerprint))
        .collect::<Result<Vec<_>, _>>()?;

    // 任何一个有效签名都必须来自一次真实的展示.Taproot 签名承诺全部输入的金额和 script,
    // 声明有假它就无效.ECDSA 和 ANYONECANPAY 只承诺自己这个输入,别的输入造假时依然有效.
    // "是不是 Taproot"本身也只是 witness_utxo 的声明.这些情况可以签,但手续费无法确认,类别是无法识别.
    let fee_unconfirmed = fee_unconfirmed(&inputs);
    Ok(Prepared {
        psbt,
        inputs,
        outputs,
        fee_unconfirmed,
    })
}

/// 累加金额.超过总量上限的值不可能是真的.
fn total(mut values: impl Iterator<Item = Amount>) -> Result<Amount, Forbid> {
    values
        .try_fold(Amount::ZERO, |sum, value| sum.checked_add(value))
        .filter(|sum| *sum <= Amount::MAX_MONEY)
        .ok_or(Forbid::ParseFailed)
}

impl Prepared {
    /// 本机不认识网络时,私钥类型只能来自路径.有一条主网路径就按主网,不能确认是测试网时都按主网.
    fn key_mode(&self) -> Option<Mode> {
        let inputs = self
            .inputs
            .iter()
            .filter_map(|input| input.own.as_ref().map(|own| &own.path));
        let outputs = self.outputs.iter().flatten().map(|own| &own.path);
        let modes: Vec<Mode> = inputs.chain(outputs).filter_map(bitcoin_path_mode).collect();
        modes
            .iter()
            .copied()
            .find(|mode| *mode == Mode::Mainnet)
            .or(modes.first().copied())
    }

    /// 输入总额减输出总额.
    pub fn fee(&self) -> Result<u64, Forbid> {
        let spent = total(self.inputs.iter().map(|i| i.prevout.value))?;
        let created = total(self.psbt.unsigned_tx.output.iter().map(|o| o.value))?;
        spent
            .checked_sub(created)
            .map(Amount::to_sat)
            .ok_or(Forbid::ParseFailed)
    }

    /// 本钱包第 `index` 个输入要签的摘要.
    pub fn sighash(&self, index: usize) -> Result<[u8; 32], Forbid> {
        let input = self.inputs.get(index).ok_or(Forbid::ParseFailed)?;
        let own = input.own.as_ref().ok_or(Forbid::AddressMismatch)?;
        let code = match own.key {
            StatedKey::Full(key) if own.kind == AccountKind::NestedSegWit => redeem_script(&key),
            _ => input.prevout.script_pubkey.clone(),
        };
        let prevouts: Vec<TxOut> = self.inputs.iter().map(|i| i.prevout.clone()).collect();
        signature_hash(&self.psbt.unsigned_tx, index, &prevouts, own.kind, &code, own.sighash)
    }
}

/// `code` 是 SegWit 的 P2WPKH script,或旧式的 scriptPubKey.Taproot 不用它.
pub fn signature_hash(
    tx: &Transaction,
    index: usize,
    prevouts: &[TxOut],
    kind: AccountKind,
    code: &Script,
    sighash: Sighash,
) -> Result<[u8; 32], Forbid> {
    let mut cache = SighashCache::new(tx);
    let value = prevouts.get(index).ok_or(Forbid::ParseFailed)?.value;
    match (kind, sighash) {
        (AccountKind::SegWit | AccountKind::NestedSegWit, Sighash::Ecdsa(ty)) => cache
            .p2wpkh_signature_hash(index, code, value, ty)
            .map(|hash| hash.to_byte_array())
            .map_err(|_| Forbid::ParseFailed),
        (AccountKind::Legacy, Sighash::Ecdsa(ty)) => cache
            .legacy_signature_hash(index, code, ty.to_u32())
            .map(|hash| hash.to_byte_array())
            .map_err(|_| Forbid::ParseFailed),
        (AccountKind::Taproot, Sighash::Taproot(ty)) => cache
            .taproot_key_spend_signature_hash(index, &Prevouts::All(prevouts), ty)
            .map(|hash| hash.to_byte_array())
            .map_err(|_| Forbid::ParseFailed),
        _ => Err(Forbid::Unsupported),
    }
}

fn address_text(script: &Script, mode: Mode) -> Option<String> {
    Address::from_script(script, address_network(mode))
        .ok()
        .map(|a| a.to_string())
}

/// 用保存的公钥重新算出 script.算不一样时拒绝.没有导出的硬化路径算不出来,返回假,解锁后用私钥核对.
fn rederives(ctx: &ReviewContext, path: &DerivationPath, kind: AccountKind, script: &Script) -> Result<bool, Forbid> {
    let key = match ctx.keys.secp256k1(path) {
        Ok(key) => key,
        Err(Forbid::PathNotAllowed) => return Ok(false),
        Err(_) => return Err(Forbid::AddressMismatch),
    };
    let key = PublicKey::from_slice(&key).map_err(|_| Forbid::AddressMismatch)?;
    if script_for(kind, &key)? != *script {
        return Err(Forbid::AddressMismatch);
    }
    Ok(true)
}

/// 花费这种输入要加的重量.
fn spend_weight(kind: AccountKind) -> u64 {
    decode::spend_weight(match kind {
        AccountKind::Taproot => ScriptClass::P2tr,
        AccountKind::NestedSegWit => ScriptClass::P2sh,
        AccountKind::Legacy => ScriptClass::P2pkh,
        AccountKind::SegWit | AccountKind::Single => ScriptClass::P2wpkh,
    })
}

/// 签名后的虚拟大小.
fn estimated_vsize(prepared: &Prepared) -> u64 {
    let spent = prepared
        .inputs
        .iter()
        .map(|input| input.prevout.script_pubkey.as_script());
    decode::estimated_vsize(&prepared.psbt.unsigned_tx, spent)
}

/// 重新派生之后的事实.策略只看它和树.
fn facts(prepared: &Prepared, ctx: &ReviewContext, mode: Mode) -> Result<BitcoinFacts, Forbid> {
    let spends = prepared
        .inputs
        .iter()
        .map(|input| {
            let Some(own) = &input.own else {
                return Ok(SpendFact::default());
            };
            Ok(SpendFact {
                own: Some(OwnEntry {
                    path: own.path.to_string(),
                    verified: rederives(ctx, &own.path, own.kind, &input.prevout.script_pubkey)?,
                    usual: is_usual_bitcoin_path(&own.path, own.kind),
                }),
                unbound_sighash: (!own.sighash.commits_everything()).then(|| own.sighash.label()),
            })
        })
        .collect::<Result<Vec<_>, Forbid>>()?;
    let outputs = prepared
        .psbt
        .unsigned_tx
        .output
        .iter()
        .zip(&prepared.outputs)
        .map(|(out, own)| {
            let sats = out.value.to_sat();
            let Some(own) = own else {
                return Ok(OutputFact {
                    sats,
                    ..OutputFact::default()
                });
            };
            let usual = is_usual_bitcoin_path(&own.path, own.kind) && matches!(own.path.0.as_slice(), [.., 1, _]);
            let verified = rederives(ctx, &own.path, own.kind, &out.script_pubkey)?;
            let change_index = match own.path.0.as_slice() {
                [.., index] if usual && verified => Some(*index),
                _ => None,
            };
            Ok(OutputFact {
                own: Some(OwnEntry {
                    path: own.path.to_string(),
                    verified,
                    usual,
                }),
                sats,
                change_index,
            })
        })
        .collect::<Result<Vec<_>, Forbid>>()?;
    Ok(BitcoinFacts {
        spends,
        outputs,
        fee: prepared.fee()?,
        fee_unconfirmed: prepared.fee_unconfirmed,
        highest_change: ctx.highest_change.of(mode),
    })
}

/// 页面顶部.发送额是全部外部输出之和,没有转给别人时是转回本钱包的金额,它们的行在 `field_ids` 里.
fn summary(
    prepared: &Prepared,
    facts: &BitcoinFacts,
    native: &Asset,
    known: Option<Network>,
    mode: Mode,
    outputs: &[NodeId],
) -> Result<Summary, Forbid> {
    let own_inputs = prepared.inputs.iter().filter(|input| input.own.is_some());
    let payers = own_inputs
        .map(|input| address_text(&input.prevout.script_pubkey, mode).ok_or(Forbid::AddressMismatch))
        .collect::<Result<Vec<String>, Forbid>>()?;
    let tx = &prepared.psbt.unsigned_tx;
    let mut sent = 0u64;
    let mut returned = 0u64;
    let mut external = Vec::new();
    let mut own = Vec::new();
    for ((out, fact), node) in tx.output.iter().zip(&facts.outputs).zip(outputs) {
        let value = out.value.to_sat();
        let text = address_text(&out.script_pubkey, mode).filter(|_| ScriptClass::of(&out.script_pubkey).has_address());
        if fact.own.as_ref().is_some_and(|own| own.verified) {
            returned += value;
            text.ok_or(Forbid::AddressMismatch)?;
            own.push(*node);
        } else {
            sent += value;
            let shown = text.unwrap_or_else(|| out.script_pubkey.as_bytes().to_lower_hex_string());
            external.push((*node, shown));
        }
    }
    let counterparty = match external.as_slice() {
        [(_, only)] => Some(only.clone()),
        [] | [_, _, ..] => None,
    };
    let value = if external.is_empty() { returned } else { sent };
    let field_ids = if external.is_empty() {
        own
    } else {
        external.iter().map(|(node, _)| *node).collect()
    };
    Ok(Summary {
        amount: Some(AmountView {
            text: format_units(&value.to_string(), native.decimals),
            symbol: Some(native.symbol.clone()),
            asset: known.map(|_| native.key()),
            unlimited: false,
        }),
        counterparty_label: counterparty.as_ref().map(|_| "收款地址".to_string()),
        counterparty,
        field_ids,
        payer: payers.into_iter().next(),
        ..Summary::default()
    })
}

/// `network` 为空表示本机不认识这个网络,类别是无法识别,路径和网络的矛盾不检查,见 docs/10-tx-decoding.md 第 1.1 节.
pub fn review(network: Option<Network>, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::RawHashSigning);
    }
    let prepared = prepare(network, &request.payload, ctx.keys.fingerprint);
    let mode = network
        .map(Network::mode)
        .or(prepared.as_ref().ok().and_then(Prepared::key_mode))
        .unwrap_or(Mode::Mainnet);
    let input = decode::Input {
        chain: Chain::Bitcoin,
        mode: Some(mode),
        network: &request.network,
        kind: request.kind,
        // 派生路径在 PSBT 里,解码后逐条核对.`request.paths` 没有校验,不能进树.
        paths: &[],
        payload: &request.payload,
        signer: None,
    };
    let decoded = match decode::decode(&PsbtWalker, &input) {
        Ok(decoded) => decoded,
        Err(failure) => return Ok(Review::undecodable(&request.network, &failure)),
    };
    let prepared = prepared?;
    let facts = facts(&prepared, ctx, mode)?;
    let assessment = policy::assess_bitcoin(&decoded, &facts);
    let native = network.map_or_else(
        || Cow::Owned(Asset::plain_native(Network::Bitcoin)),
        |network| ctx.native(network),
    );
    let outputs: Vec<NodeId> = decoded
        .actions()
        .into_iter()
        .filter(|(_, _, kind)| kind.starts_with(policy::OUTPUT_PREFIX))
        .map(|(id, _, _)| id)
        .collect();
    let summary = summary(&prepared, &facts, &native, network, mode, &outputs)?;
    let change_index = facts.outputs.iter().filter_map(|output| output.change_index).max();
    let review = Review {
        change_index,
        change_is_testnet: mode == Mode::Testnet,
        ..Review::assessed(&decoded, assessment)
    };
    Ok(review.summarized(summary))
}

fn output_weight(kind: AccountKind) -> Result<u64, Error> {
    let script = match kind {
        AccountKind::SegWit => 22,
        AccountKind::Taproot => 34,
        AccountKind::NestedSegWit => 23,
        AccountKind::Legacy => 25,
        AccountKind::Single => return Err(Error::InvalidParams),
    };
    Ok((8 + 1 + script) * 4)
}

/// 低于这个金额的输出节点不转发,和 Bitcoin Core 默认的粉尘门限一致.
fn dust_limit(kind: AccountKind) -> u64 {
    match kind {
        AccountKind::Taproot => 330,
        AccountKind::NestedSegWit => 540,
        AccountKind::Legacy => 546,
        AccountKind::SegWit | AccountKind::Single => 294,
    }
}

fn bitcoin_kind(path: &str) -> Result<AccountKind, Error> {
    let info = classify(&DerivationPath::parse(path)?)?;
    if info.chain != Chain::Bitcoin {
        return Err(Error::PathNetworkMismatch);
    }
    Ok(info.kind)
}

/// 选币.从大到小取,够付金额和手续费就停.`fee_rate` 的单位是 sat/vB.
///
/// 只知道总额,不知道收款地址,所以按一个最长的收款输出估算.
pub fn select_coins(utxos: &[Utxo], amount: u64, fee_rate: u64, change_kind: AccountKind) -> Result<Selection, Error> {
    select_coins_for(utxos, amount, 1, fee_rate, change_kind)
}

/// 同 `select_coins`,一笔发给 `payments` 个地址,每个都按最长的收款输出估算.
pub fn select_coins_for(
    utxos: &[Utxo],
    amount: u64,
    payments: u64,
    fee_rate: u64,
    change_kind: AccountKind,
) -> Result<Selection, Error> {
    if amount == 0 || fee_rate == 0 || payments == 0 {
        return Err(Error::InvalidParams);
    }
    let change_weight = output_weight(change_kind)?;
    let fee_for = |weight: u64| weight.div_ceil(4).checked_mul(fee_rate).ok_or(Error::InvalidParams);
    let mut sorted = utxos
        .iter()
        .map(|utxo| Ok((utxo, INPUT_WEIGHT + spend_weight(bitcoin_kind(&utxo.path)?))))
        .collect::<Result<Vec<_>, Error>>()?;
    sorted.sort_by_key(|(utxo, _)| std::cmp::Reverse(utxo.amount));

    let payment_weight = PAYMENT_WEIGHT.checked_mul(payments).ok_or(Error::InvalidParams)?;
    let mut weight = OVERHEAD_WEIGHT + payment_weight;
    let mut value = 0u64;
    let mut inputs = Vec::new();
    for (utxo, cost) in sorted {
        // 这个输入付不起自己的手续费,加上只会亏.
        if utxo.amount <= fee_for(cost)? {
            continue;
        }
        weight += cost;
        value = value.checked_add(utxo.amount).ok_or(Error::InvalidParams)?;
        inputs.push(utxo.clone());
        let Some(rest) = value.checked_sub(amount) else {
            continue;
        };
        if rest < fee_for(weight)? {
            continue;
        }
        let with_change = fee_for(weight + change_weight)?;
        return Ok(match rest.checked_sub(with_change) {
            Some(change) if change >= dust_limit(change_kind) => Selection {
                inputs,
                fee: with_change,
                change,
            },
            _ => Selection {
                inputs,
                fee: rest,
                change: 0,
            },
        });
    }
    Err(Error::InsufficientFunds)
}

/// 替换交易的金额分配.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Replacement {
    pub fee: u64,
    pub change: u64,
}

/// RBF 用同一组输入提高手续费.费率不低于 `fee_rate`,也不低于原费率的 1.25 倍.
/// 总额至少比原交易多付一次中继费(BIP-125).`amount` 为 0 表示取消,全部回到找零.
/// 太小的找零并入手续费,取消时不能这样,因为没有别的输出.
pub fn replacement(
    total: u64,
    amount: u64,
    old_fee: u64,
    vsize: u64,
    fee_rate: u64,
    change_kind: AccountKind,
) -> Result<Replacement, Error> {
    if vsize == 0 {
        return Err(Error::InvalidParams);
    }
    let old_rate = old_fee.div_ceil(vsize);
    let rate = fee_rate.max((old_rate * 5).div_ceil(4)).max(old_rate + 1);
    let fee = rate
        .checked_mul(vsize)
        .ok_or(Error::InvalidParams)?
        .max(old_fee.checked_add(vsize).ok_or(Error::InvalidParams)?);
    let rest = total
        .checked_sub(amount)
        .and_then(|rest| rest.checked_sub(fee))
        .ok_or(Error::InsufficientFunds)?;
    let dust = dust_limit(change_kind);
    Ok(match (rest >= dust, amount) {
        (true, _) => Replacement { fee, change: rest },
        (false, 0) => return Err(Error::InsufficientFunds),
        (false, _) => Replacement {
            fee: fee + rest,
            change: 0,
        },
    })
}

/// 请求里的手续费和估算的虚拟大小.
pub fn fee_info(network: Network, request: &SignRequest) -> Result<(u64, u64), Error> {
    let prepared = prepare(network, &request.payload, request.fingerprint).map_err(|_| Error::InvalidParams)?;
    let fee = prepared.fee().map_err(|_| Error::InvalidParams)?;
    Ok((fee, estimated_vsize(&prepared)))
}

fn key_source(keys: &PublicKeys, path: &DerivationPath) -> KeySource {
    let children = path.0.iter().map(|level| ChildNumber::from(*level)).collect::<Vec<_>>();
    (Fingerprint::from(keys.fingerprint), BipPath::from(children))
}

fn public_key(keys: &PublicKeys, path: &DerivationPath) -> Result<PublicKey, Error> {
    let key = keys.secp256k1(path).map_err(|_| Error::InvalidKey)?;
    PublicKey::from_slice(&key).map_err(|_| Error::InvalidKey)
}

fn psbt_input(network: Network, keys: &PublicKeys, utxo: &Utxo, with_prev: bool) -> Result<(TxIn, Input), Error> {
    let path = DerivationPath::parse(&utxo.path)?;
    let kind = validate_for_network(&path, network)?.kind;
    let key = public_key(keys, &path)?;
    let script = script_for(kind, &key).map_err(|_| Error::InvalidKey)?;
    let txid = Txid::from_str(&utxo.txid).map_err(|_| Error::InvalidParams)?;
    let source = key_source(keys, &path);

    let spent = TxOut {
        value: Amount::from_sat(utxo.amount),
        script_pubkey: script,
    };
    let mut input = Input::default();
    if with_prev {
        let raw = utxo.prev_tx.as_deref().ok_or(Error::InvalidParams)?;
        let prev: Transaction = deserialize(raw).map_err(|_| Error::InvalidParams)?;
        if prev.compute_txid() != txid || prev.output.get(utxo.vout as usize) != Some(&spent) {
            return Err(Error::InvalidParams);
        }
        input.non_witness_utxo = Some(prev);
    }
    if kind == AccountKind::Taproot {
        let internal = key.x_only_public_key().0;
        input.tap_internal_key = Some(internal);
        input.tap_key_origins.insert(internal, (Vec::new(), source));
    } else {
        if kind == AccountKind::NestedSegWit {
            input.redeem_script = Some(redeem_script(&key));
        }
        input.bip32_derivation.insert(key, source);
    }
    if kind != AccountKind::Legacy {
        input.witness_utxo = Some(spent);
    }
    let txin = TxIn {
        previous_output: OutPoint { txid, vout: utxo.vout },
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::new(),
    };
    Ok((txin, input))
}

fn change_output(network: Network, keys: &PublicKeys, path: &str, amount: u64) -> Result<(TxOut, Output), Error> {
    let path = DerivationPath::parse(path)?;
    let kind = validate_for_network(&path, network)?.kind;
    if amount == 0 || !matches!(path.0.as_slice(), [.., 1, _]) {
        return Err(Error::InvalidParams);
    }
    let key = public_key(keys, &path)?;
    let source = key_source(keys, &path);
    let mut meta = Output::default();
    if kind == AccountKind::Taproot {
        let internal = key.x_only_public_key().0;
        meta.tap_internal_key = Some(internal);
        meta.tap_key_origins.insert(internal, (Vec::new(), source));
    } else {
        if kind == AccountKind::NestedSegWit {
            meta.redeem_script = Some(redeem_script(&key));
        }
        meta.bip32_derivation.insert(key, source);
    }
    let script_pubkey = script_for(kind, &key).map_err(|_| Error::InvalidKey)?;
    Ok((
        TxOut {
            value: Amount::from_sat(amount),
            script_pubkey,
        },
        meta,
    ))
}

/// 构建待签名的 PSBT,所有输入开启 RBF.`change` 是找零路径和金额.
/// 有非 Taproot 输入时,Taproot 输入也要带前序交易,原因见 `prepare`.
pub fn build_psbt(
    network: Network,
    keys: &PublicKeys,
    inputs: &[Utxo],
    payments: &[Payment],
    change: Option<(&str, u64)>,
) -> Result<Vec<u8>, Error> {
    // 没有收款方时全部回到找零,这是取消交易的写法,冷钱包显示"转给自己".
    if network.chain() != Chain::Bitcoin || inputs.is_empty() || (payments.is_empty() && change.is_none()) {
        return Err(Error::InvalidParams);
    }
    let params = bitcoin_network(network).ok_or(Error::InvalidParams)?;
    let with_prev = inputs
        .iter()
        .any(|utxo| bitcoin_kind(&utxo.path) != Ok(AccountKind::Taproot));
    let (txins, metas): (Vec<_>, Vec<_>) = inputs
        .iter()
        .map(|utxo| psbt_input(network, keys, utxo, with_prev))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();
    let outpoints: HashSet<OutPoint> = txins.iter().map(|txin| txin.previous_output).collect();
    if outpoints.len() != txins.len() {
        return Err(Error::InvalidParams);
    }

    let mut output = Vec::with_capacity(payments.len() + 1);
    let mut output_metas = vec![Output::default(); payments.len()];
    for payment in payments {
        if payment.amount == 0 {
            return Err(Error::InvalidParams);
        }
        let address = Address::<NetworkUnchecked>::from_str(&payment.address)
            .map_err(|_| Error::InvalidAddress)?
            .require_network(params)
            .map_err(|_| Error::InvalidAddress)?;
        let script_pubkey = address.script_pubkey();
        if Amount::from_sat(payment.amount) < script_pubkey.minimal_non_dust() {
            return Err(Error::Dust);
        }
        output.push(TxOut {
            value: Amount::from_sat(payment.amount),
            script_pubkey,
        });
    }
    if let Some((path, amount)) = change {
        if amount < dust_limit(bitcoin_kind(path)?) {
            return Err(Error::Dust);
        }
        let (out, meta) = change_output(network, keys, path, amount)?;
        output.push(out);
        output_metas.push(meta);
    }

    let spent = total(inputs.iter().map(|utxo| Amount::from_sat(utxo.amount))).map_err(|_| Error::InvalidParams)?;
    let created = total(output.iter().map(|out| out.value)).map_err(|_| Error::InvalidParams)?;
    if created > spent {
        return Err(Error::InvalidParams);
    }

    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: txins,
        output,
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).map_err(|_| Error::InvalidParams)?;
    psbt.inputs = metas;
    psbt.outputs = output_metas;
    Ok(psbt.serialize())
}

fn check_signature<C: Verification>(
    secp: &Secp256k1<C>,
    own: &OwnInput,
    message: &Message,
    bytes: &[u8],
) -> Result<(), Error> {
    match (own.key, own.sighash) {
        (StatedKey::Full(key), Sighash::Ecdsa(ty)) => {
            let signature = ecdsa::Signature::from_slice(bytes).map_err(|_| Error::BadSignature)?;
            if signature.sighash_type != ty {
                return Err(Error::BadSignature);
            }
            secp.verify_ecdsa(message, &signature.signature, &key)
                .map_err(|_| Error::BadSignature)
        }
        (StatedKey::XOnly(key), Sighash::Taproot(ty)) => {
            let signature = taproot::Signature::from_slice(bytes).map_err(|_| Error::BadSignature)?;
            // 65 字节且尾字节是 0 的签名在共识上无效,解析器却会放过.
            let expected = if ty == TapSighashType::Default { 64 } else { 65 };
            if signature.sighash_type != ty || bytes.len() != expected {
                return Err(Error::BadSignature);
            }
            let (output_key, _) = key.tap_tweak(secp, None);
            secp.verify_schnorr(&signature.signature, message, &output_key.to_x_only_public_key())
                .map_err(|_| Error::BadSignature)
        }
        _ => Err(Error::BadSignature),
    }
}

/// 把验证过的签名写成最终的解锁脚本和见证.
fn finalize(target: &mut Input, own: &OwnInput, signature: &[u8]) -> Result<(), Error> {
    let push = |bytes: &[u8]| PushBytesBuf::try_from(bytes.to_vec()).map_err(|_| Error::BadSignature);
    let mut script_sig = ScriptBuf::new();
    let mut witness = Witness::new();
    match own.key {
        StatedKey::XOnly(_) => witness.push(signature),
        StatedKey::Full(key) if own.kind == AccountKind::Legacy => {
            script_sig = Builder::new()
                .push_slice(push(signature)?)
                .push_slice(push(&key.serialize())?)
                .into_script();
        }
        StatedKey::Full(key) => {
            witness.push(signature);
            witness.push(key.serialize());
            if own.kind == AccountKind::NestedSegWit {
                script_sig = Builder::new()
                    .push_slice(push(redeem_script(&key).as_bytes())?)
                    .into_script();
            }
        }
    }
    target.final_script_sig = Some(script_sig);
    target.final_script_witness = Some(witness);
    Ok(())
}

/// 用公钥逐个验证签名,装回 PSBT,提取原始交易.输出和金额都来自构建记录,签名改变不了它们.
pub fn verify(network: Network, request: &SignRequest, response: &SignResponse) -> Result<Verified, Error> {
    if request.id != response.id {
        return Err(Error::ResponseMismatch);
    }
    let prepared = prepare(network, &request.payload, request.fingerprint).map_err(|_| Error::InvalidParams)?;
    prepared.fee().map_err(|_| Error::InvalidParams)?;
    if response.signatures.len() != prepared.inputs.len() {
        return Err(Error::BadSignature);
    }
    let secp = Secp256k1::verification_only();
    let mut psbt = prepared.psbt.clone();
    for (index, (input, signature)) in prepared.inputs.iter().zip(&response.signatures).enumerate() {
        let target = &mut psbt.inputs[index];
        match &input.own {
            Some(own) => {
                let digest = prepared.sighash(index).map_err(|_| Error::InvalidParams)?;
                check_signature(&secp, own, &Message::from_digest(digest), signature.as_slice())?;
                finalize(target, own, signature.as_slice())?;
            }
            // 其他钱包的输入不归这里签,必须已经带着最终的解锁数据.
            None if signature.is_empty()
                && (target.final_script_sig.is_some() || target.final_script_witness.is_some()) => {}
            None => return Err(Error::BadSignature),
        }
    }
    let tx = psbt.extract_tx_unchecked_fee_rate();
    Ok(Verified {
        raw: serialize(&tx),
        hash: tx.compute_txid().to_string(),
        fee_unconfirmed: prepared.fee_unconfirmed,
        signature_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::encode::deserialize_hex;
    use bitcoin::secp256k1::Parity;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn txout(script: &str, value: u64) -> TxOut {
        TxOut {
            value: Amount::from_sat(value),
            script_pubkey: ScriptBuf::from_bytes(unhex(script)),
        }
    }

    // BIP-143 的 Native P2WPKH 示例:
    // https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki#native-p2wpkh
    #[test]
    fn bip143_native_p2wpkh_sighash() {
        let tx: Transaction = deserialize_hex(concat!(
            "0100000002fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f0000000000eeffffff",
            "ef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff",
            "02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac",
            "9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac11000000",
        ))
        .unwrap();
        let prevouts = [
            txout(
                "2103c9f4836b9a4f77fc0d81f7bcb01b7f1b35916864b9476c241ce9fc198bd25432ac",
                625_000_000,
            ),
            txout("00141d0f172a0ecb48aee1be1f2687d2963ae33f71a1", 600_000_000),
        ];
        let digest = signature_hash(
            &tx,
            1,
            &prevouts,
            AccountKind::SegWit,
            &prevouts[1].script_pubkey,
            Sighash::Ecdsa(EcdsaSighashType::All),
        )
        .unwrap();
        assert_eq!(
            digest.to_lower_hex_string(),
            "c37af31116d1b27caf68aae9e3ac82f1477929014d5b917657d0eb49478cb670"
        );
    }

    // BIP-143 的 P2SH-P2WPKH 示例:
    // https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki#p2sh-p2wpkh
    #[test]
    fn bip143_nested_p2wpkh_sighash() {
        let tx: Transaction = deserialize_hex(concat!(
            "0100000001db6b1b20aa0fd7b23880be2ecbd4a98130974cf4748fb66092ac4d3ceb1a54770100000000feffffff",
            "02b8b4eb0b000000001976a914a457b684d7f0d539a46a45bbc043f35b59d0d96388ac",
            "0008af2f000000001976a914fd270b1ee6abcaea97fea7ad0402e8bd8ad6d77c88ac92040000",
        ))
        .unwrap();
        let key = PublicKey::from_slice(&unhex(
            "03ad1d8e89212f0b92c74d23bb710c00662ad1470198ac48c43f7d6f93a2a26873",
        ))
        .unwrap();
        let prevouts = [txout("a9144733f37cf4db86fbc2efed2500b4f4e49f31202387", 1_000_000_000)];
        assert_eq!(
            script_for(AccountKind::NestedSegWit, &key).unwrap(),
            prevouts[0].script_pubkey
        );
        let digest = signature_hash(
            &tx,
            0,
            &prevouts,
            AccountKind::NestedSegWit,
            &redeem_script(&key),
            Sighash::Ecdsa(EcdsaSighashType::All),
        )
        .unwrap();
        assert_eq!(
            digest.to_lower_hex_string(),
            "64f3b0f4dd2bb3aa1ce8566d220cc74dda9df97d8490cc81d89d735c92e59fb6"
        );
    }

    // BIP-341 的 wallet-test-vectors.json,keyPathSpending 全部 7 个输入:
    // https://github.com/bitcoin/bips/blob/master/bip-0341/wallet-test-vectors.json
    #[test]
    fn bip341_key_path_sighash() {
        let tx: Transaction = deserialize_hex(concat!(
            "02000000097de20cbff686da83a54981d2b9bab3586f4ca7e48f57f5b55963115f3b334e9c010000000000000000",
            "d7b7cab57b1393ace2d064f4d4a2cb8af6def61273e127517d44759b6dafdd990000000000fffffffff8e1f58338",
            "4333689228c5d28eac13366be082dc57441760d957275419a418420000000000fffffffff0689180aa63b30cb162",
            "a73c6d2a38b7eeda2a83ece74310fda0843ad604853b0100000000feffffffaa5202bdf6d8ccd2ee0f0202afbbb7",
            "461d9264a25e5bfd3c5a52ee1239e0ba6c0000000000feffffff956149bdc66faa968eb2be2d2faa29718acbfe39",
            "41215893a2a3446d32acd050000000000000000000e664b9773b88c09c32cb70a2a3e4da0ced63b7ba3b22f84853",
            "1bbb1d5d5f4c94010000000000000000e9aa6b8e6c9de67619e6a3924ae25696bb7b694bb677a632a74ef7eadfd4",
            "eabf0000000000ffffffffa778eb6a263dc090464cd125c466b5a99667720b1c110468831d058aa1b82af1010000",
            "0000ffffffff0200ca9a3b000000001976a91406afd46bcdfd22ef94ac122aa11f241244a37ecc88ac807840cb00",
            "00000020ac9a87f5594be208f8532db38cff670c450ed2fea8fcdefcc9a663f78bab962b0065cd1d",
        ))
        .unwrap();
        let prevouts = [
            txout(
                "512053a1f6e454df1aa2776a2814a721372d6258050de330b3c6d10ee8f4e0dda343",
                420_000_000,
            ),
            txout(
                "5120147c9c57132f6e7ecddba9800bb0c4449251c92a1e60371ee77557b6620f3ea3",
                462_000_000,
            ),
            txout("76a914751e76e8199196d454941c45d1b3a323f1433bd688ac", 294_000_000),
            txout(
                "5120e4d810fd50586274face62b8a807eb9719cef49c04177cc6b76a9a4251d5450e",
                504_000_000,
            ),
            txout(
                "512091b64d5324723a985170e4dc5a0f84c041804f2cd12660fa5dec09fc21783605",
                630_000_000,
            ),
            txout("00147dd65592d0ab2fe0d0257d571abf032cd9db93dc", 378_000_000),
            txout(
                "512075169f4001aa68f15bbed28b218df1d0a62cbbcf1188c6665110c293c907b831",
                672_000_000,
            ),
            txout(
                "5120712447206d7a5238acc7ff53fbe94a3b64539ad291c7cdbc490b7577e4b17df5",
                546_000_000,
            ),
            txout(
                "512077e30a5522dd9f894c3f8b8bd4c4b2cf82ca7da8a3ea6a239655c39c050ab220",
                588_000_000,
            ),
        ];
        let cases = [
            (
                0,
                TapSighashType::Single,
                "2514a6272f85cfa0f45eb907fcb0d121b808ed37c6ea160a5a9046ed5526d555",
            ),
            (
                1,
                TapSighashType::SinglePlusAnyoneCanPay,
                "325a644af47e8a5a2591cda0ab0723978537318f10e6a63d4eed783b96a71a4d",
            ),
            (
                3,
                TapSighashType::All,
                "bf013ea93474aa67815b1b6cc441d23b64fa310911d991e713cd34c7f5d46669",
            ),
            (
                4,
                TapSighashType::Default,
                "4f900a0bae3f1446fd48490c2958b5a023228f01661cda3496a11da502a7f7ef",
            ),
            (
                6,
                TapSighashType::None,
                "15f25c298eb5cdc7eb1d638dd2d45c97c4c59dcaec6679cfc16ad84f30876b85",
            ),
            (
                7,
                TapSighashType::NonePlusAnyoneCanPay,
                "cd292de50313804dabe4685e83f923d2969577191a3e1d2882220dca88cbeb10",
            ),
            (
                8,
                TapSighashType::AllPlusAnyoneCanPay,
                "cccb739eca6c13a8a89e6e5cd317ffe55669bbda23f2fd37b0f18755e008edd2",
            ),
        ];
        for (index, ty, expected) in cases {
            let code = &prevouts[index].script_pubkey;
            let digest =
                signature_hash(&tx, index, &prevouts, AccountKind::Taproot, code, Sighash::Taproot(ty)).unwrap();
            assert_eq!(digest.to_lower_hex_string(), expected, "输入 {index}");
        }
    }

    // BIP-341 的 wallet-test-vectors.json,scriptPubKey 第一项,没有脚本树.
    #[test]
    fn bip341_output_key_without_script_tree() {
        let internal = XOnlyPublicKey::from_slice(&unhex(
            "d6889cb081036e0faefa3a35157ad71086b123b2b144b649798b494c300a961d",
        ))
        .unwrap();
        let key = PublicKey::from_x_only_public_key(internal, Parity::Even);
        assert_eq!(
            script_for(AccountKind::Taproot, &key)
                .unwrap()
                .as_bytes()
                .to_lower_hex_string(),
            "512053a1f6e454df1aa2776a2814a721372d6258050de330b3c6d10ee8f4e0dda343"
        );
    }

    // BIP-174 的测试向量:https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki#test-vectors
    #[test]
    fn bip174_invalid_psbts_are_refused() {
        let cases = [
            // 不是 PSBT,是普通交易.
            "0200000001268171371edff285e937adeea4b37b78000c0566cbb3ad64641713ca42171bf6000000006a",
            // 声明的长度和实际不符.
            "70736274ff0100337401ff0700010000000100ff01000a73317428ff0000000001ff010301000001000000000000000076010000004100090000000000",
            // 只有 magic.
            "70736274ff",
        ];
        for case in cases {
            assert_eq!(
                prepare(Network::Bitcoin, &unhex(case), [0; 4]),
                Err(Forbid::ParseFailed),
                "{case}"
            );
        }
    }

    #[test]
    fn bip174_unknown_input_fields_round_trip() {
        // BIP-174 的 PSBT with unknown types in the inputs.解析成功,只是输入都不属于这个钱包.
        let bytes = unhex(concat!(
            "70736274ff01003f0200000001ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000000ffffffff",
            "010000000000000000036a010000000000000af00102030405060708090f0102030405060708090a0b0c0d0e0f0000",
        ));
        assert_eq!(prepare(Network::Bitcoin, &bytes, [0; 4]), Err(Forbid::AddressMismatch));
    }

    fn coin(path: &str, amount: u64) -> Utxo {
        Utxo {
            txid: "00".repeat(32),
            vout: 0,
            amount,
            path: path.into(),
            prev_tx: None,
        }
    }

    const SEGWIT: &str = "m/84'/0'/0'/0/0";
    const TAPROOT: &str = "m/86'/0'/0'/0/0";

    /// 一个 SegWit 输入,一个收款输出的基准重量,加找零.
    const ONE_IN: u64 = OVERHEAD_WEIGHT + PAYMENT_WEIGHT + INPUT_WEIGHT + 108;

    #[test]
    fn selection_pays_the_estimated_fee_and_returns_change() {
        let picked = select_coins(&[coin(SEGWIT, 1_000_000)], 400_000, 10, AccountKind::SegWit).unwrap();
        let vsize = (ONE_IN + 31 * 4).div_ceil(4);
        assert_eq!(vsize, 153);
        assert_eq!(picked.fee, vsize * 10);
        assert_eq!(picked.change, 1_000_000 - 400_000 - picked.fee);
    }

    #[test]
    fn each_extra_payment_is_paid_for() {
        let coins = [coin(SEGWIT, 1_000_000)];
        let one = select_coins(&coins, 400_000, 10, AccountKind::SegWit).unwrap();
        let three = select_coins_for(&coins, 400_000, 3, 10, AccountKind::SegWit).unwrap();
        assert_eq!(three.fee, one.fee + 2 * 43 * 10);
        assert_eq!(three.change, 1_000_000 - 400_000 - three.fee);
        assert_eq!(
            select_coins_for(&coins, 400_000, 0, 10, AccountKind::SegWit),
            Err(Error::InvalidParams)
        );
    }

    #[test]
    fn selection_takes_the_largest_coins_first_and_stops() {
        let coins = [coin(SEGWIT, 10_000), coin(TAPROOT, 900_000), coin(SEGWIT, 50_000)];
        let picked = select_coins(&coins, 100_000, 1, AccountKind::Taproot).unwrap();
        assert_eq!(picked.inputs, vec![coins[1].clone()]);
    }

    #[test]
    fn dust_change_goes_to_the_fee() {
        let fee_without_change = ONE_IN.div_ceil(4);
        // 留下的找零低于 294 sat.
        let total = 100_000 + fee_without_change + 31 + 293;
        let picked = select_coins(&[coin(SEGWIT, total)], 100_000, 1, AccountKind::SegWit).unwrap();
        assert_eq!((picked.change, picked.fee), (0, total - 100_000));

        // 正好到粉尘门限时保留找零.
        let total = total + 1;
        let picked = select_coins(&[coin(SEGWIT, total)], 100_000, 1, AccountKind::SegWit).unwrap();
        assert_eq!(picked.change, 294);
        assert_eq!(picked.fee + picked.change, total - 100_000);
    }

    #[test]
    fn replacement_pays_the_relay_fee_on_top() {
        let result = replacement(100_000, 50_000, 1_000, 200, 1, AccountKind::SegWit).unwrap();
        assert_eq!(result.fee, 1_400);
        assert_eq!(result.change, 48_600);
        let slow = replacement(100_000, 50_000, 200, 200, 1, AccountKind::SegWit).unwrap();
        assert_eq!(slow.fee, 400);
    }

    #[test]
    fn replacement_follows_a_higher_market_rate() {
        let result = replacement(100_000, 50_000, 1_000, 200, 20, AccountKind::SegWit).unwrap();
        assert_eq!(result.fee, 4_000);
    }

    #[test]
    fn replacement_folds_dust_change_into_the_fee() {
        let result = replacement(52_000, 50_000, 1_000, 200, 9, AccountKind::SegWit).unwrap();
        assert_eq!(result, Replacement { fee: 2_000, change: 0 });
    }

    #[test]
    fn cancelling_returns_everything_as_change() {
        let result = replacement(100_000, 0, 1_000, 200, 1, AccountKind::SegWit).unwrap();
        assert_eq!(result.fee + result.change, 100_000);
        assert_eq!(
            replacement(1_400, 0, 1_000, 200, 1, AccountKind::SegWit),
            Err(Error::InsufficientFunds)
        );
    }

    #[test]
    fn replacement_needs_enough_inputs() {
        assert_eq!(
            replacement(50_100, 50_000, 1_000, 200, 1, AccountKind::SegWit),
            Err(Error::InsufficientFunds)
        );
    }

    #[test]
    fn insufficient_balance_is_an_error() {
        let coins = [coin(SEGWIT, 10_000)];
        assert_eq!(
            select_coins(&coins, 10_000, 1, AccountKind::SegWit),
            Err(Error::InsufficientFunds)
        );
        assert_eq!(
            select_coins(&[], 1, 1, AccountKind::SegWit),
            Err(Error::InsufficientFunds)
        );
        assert_eq!(
            select_coins(&coins, 1, u64::MAX, AccountKind::SegWit),
            Err(Error::InvalidParams)
        );
    }

    #[test]
    fn coins_that_cannot_pay_for_themselves_are_skipped() {
        let coins = [coin(SEGWIT, 100_000), coin(SEGWIT, 500)];
        let picked = select_coins(&coins, 90_000, 10, AccountKind::SegWit).unwrap();
        assert_eq!(picked.inputs.len(), 1);
        assert_eq!(
            select_coins(&coins, 100_100, 10, AccountKind::SegWit),
            Err(Error::InsufficientFunds)
        );
    }

    #[test]
    fn large_expensive_coins_do_not_block_cheaper_ones() {
        // 旧式币金额更大,排在前面,但付不起自己的 148 vB.选了它这笔就凑不够了.
        let coins = [coin(SEGWIT, 100_000), coin("m/44'/0'/0'/0/0", 1_000), coin(SEGWIT, 900)];
        let picked = select_coins(&coins, 99_000, 10, AccountKind::SegWit).unwrap();
        assert_eq!(picked.inputs, vec![coins[0].clone(), coins[2].clone()]);
        assert_eq!((picked.fee, picked.change), (1_900, 0));
    }

    #[test]
    fn selection_rejects_paths_of_other_chains() {
        let evm = coin("m/44'/60'/0'/0/0", 100_000);
        assert_eq!(
            select_coins(&[evm], 1, 1, AccountKind::SegWit),
            Err(Error::PathNetworkMismatch)
        );
        assert_eq!(select_coins(&[], 1, 1, AccountKind::Single), Err(Error::InvalidParams));
    }
}

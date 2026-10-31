//! Bitcoin 交易:构建 PSBT,核对,核验.见 docs/04-security.md 第 2.6,2.7 节.
//!
//! PSBT 来自网页端,里面的声明都不可信.路径和地址靠重新派生,金额靠前序交易的 txid,
//! 有一项对不上就禁止签名.`prepare` 是核对,签名,核验共用的入口,三处看到的输入不可能不一致.

use crate::keys::PublicKeys;
use crate::path::{DerivationPath, classify, validate_for_network};
use crate::policy;
use crate::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use crate::review::{ActionView, AmountView, Forbid, RawLine, Review, ReviewBuilder, Risk, Row};
use crate::tx::{ReviewContext, Verified};
use crate::units::format_units;
use crate::{AccountKind, Chain, Error, Network};
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

/// 本钱包的输入.路径已过白名单,声明的公钥能算出输入实际的 script.
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

/// 解析并确认过的 PSBT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub psbt: Psbt,
    pub inputs: Vec<PreparedInput>,
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

/// 金额和 script 只从前序交易取,取不到时只有 Taproot 可以先用声明的值.
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
        Some(out) if out.script_pubkey.is_p2tr() => Ok((out.clone(), false)),
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

fn own_input(
    origin: Origin,
    input: &Input,
    index: usize,
    outputs: usize,
    network: Network,
) -> Result<OwnInput, Forbid> {
    let info = validate_for_network(&origin.path, network).map_err(|e| match e {
        Error::PathNetworkMismatch => Forbid::PathNetworkMismatch,
        _ => Forbid::PathNotAllowed,
    })?;
    origin.check_form(info.kind)?;
    match origin.key {
        StatedKey::XOnly(key) => {
            if input.tap_merkle_root.is_some() || input.tap_internal_key.is_some_and(|internal| internal != key) {
                return Err(Forbid::AddressMismatch);
            }
        }
        StatedKey::Full(key) => {
            let wrapped = info.kind == AccountKind::NestedSegWit;
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
    let sighash = input_sighash(input, info.kind, index, outputs)?;
    Ok(OwnInput {
        path: origin.path,
        kind: info.kind,
        sighash,
        key: origin.key,
    })
}

/// 解析 PSBT,找出本钱包的输入,确认每个输入的金额和 script.
///
/// 这里只看 PSBT 自身.公钥对不对由调用方负责:`review` 用保存的公钥重新派生,
/// 签名时用私钥派生,`verify` 没有账户公钥,靠签名本身.
pub fn prepare(network: Network, payload: &[u8], fingerprint: [u8; 4]) -> Result<Prepared, Forbid> {
    let psbt = Psbt::deserialize(payload).map_err(|_| Forbid::ParseFailed)?;
    // 解析器会忽略末尾多余的字节,重新编码必须回到完全相同的内容.
    if psbt.serialize() != payload {
        return Err(Forbid::ParseFailed);
    }
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
        let own = origin
            .map(|o| own_input(o, input, index, tx.output.len(), network))
            .transpose()?;
        let (prevout, proven) = spent_output(txin, input)?;
        if let Some(own) = &own
            && own.key.script(own.kind)? != prevout.script_pubkey
        {
            return Err(Forbid::AddressMismatch);
        }
        inputs.push(PreparedInput { prevout, proven, own });
    }

    // 任何一个有效签名都必须来自一次真实的展示.Taproot 签名承诺全部输入的金额和 script,
    // 声明有假它就无效.ECDSA 和 ANYONECANPAY 只承诺自己这个输入,别的输入造假时依然有效.
    // "是不是 Taproot"本身也只是 witness_utxo 的声明:把用户的 SegWit 币说成 Taproot,
    // 分两次各骗一个 ECDSA 签名再拼起来,就是 CVE-2020-14199.
    // 所以只要有这样的签名,其余输入都要有前序交易,Taproot 也不例外.
    for (index, input) in inputs.iter().enumerate() {
        let weak = input.own.as_ref().is_some_and(|own| !own.vouches_for_all());
        let unproven = inputs
            .iter()
            .enumerate()
            .any(|(other, input)| other != index && !input.proven);
        if weak && unproven {
            return Err(Forbid::MissingPrevTx);
        }
    }
    Ok(Prepared { psbt, inputs })
}

/// 累加金额.超过总量上限的值不可能是真的.
fn total(mut values: impl Iterator<Item = Amount>) -> Result<Amount, Forbid> {
    values
        .try_fold(Amount::ZERO, |sum, value| sum.checked_add(value))
        .filter(|sum| *sum <= Amount::MAX_MONEY)
        .ok_or(Forbid::ParseFailed)
}

impl Prepared {
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
fn signature_hash(
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

fn kind_label(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::SegWit => "原生 SegWit",
        AccountKind::Taproot => "Taproot",
        AccountKind::NestedSegWit => "嵌套 SegWit",
        AccountKind::Legacy => "旧式",
        AccountKind::Single => "单地址",
    }
}

fn address_text(script: &Script, network: Network) -> Option<String> {
    let params = bitcoin_network(network)?;
    Address::from_script(script, params).ok().map(|a| a.to_string())
}

fn derived_script(ctx: &ReviewContext, path: &DerivationPath, kind: AccountKind) -> Result<ScriptBuf, Forbid> {
    let key = ctx.keys.secp256k1(path)?;
    let key = PublicKey::from_slice(&key).map_err(|_| Forbid::AddressMismatch)?;
    script_for(kind, &key)
}

/// 输出是不是本钱包的找零.带本钱包指纹却对不上的输出一律拒绝,不能当成外部输出放过去.
/// 返回找零索引.
fn change_index(out: &TxOut, meta: &Output, network: Network, ctx: &ReviewContext) -> Result<Option<u32>, Forbid> {
    let Some(origin) = origin(&meta.bip32_derivation, &meta.tap_key_origins, ctx.keys.fingerprint)? else {
        return Ok(None);
    };
    let kind = validate_for_network(&origin.path, network)
        .map_err(|_| Forbid::AddressMismatch)?
        .kind;
    origin.check_form(kind)?;
    let [.., 1, index] = origin.path.0.as_slice() else {
        return Err(Forbid::AddressMismatch);
    };
    if meta.tap_tree.is_some() {
        return Err(Forbid::AddressMismatch);
    }
    if let (StatedKey::XOnly(key), Some(internal)) = (origin.key, meta.tap_internal_key)
        && key != internal
    {
        return Err(Forbid::AddressMismatch);
    }
    let derived = derived_script(ctx, &origin.path, kind).map_err(|_| Forbid::AddressMismatch)?;
    if derived != out.script_pubkey {
        return Err(Forbid::AddressMismatch);
    }
    Ok(Some(*index))
}

/// 花费这种输入要加的重量.解锁脚本每字节算 4,见证每字节算 1,签名按最长算.
fn spend_weight(kind: AccountKind) -> u64 {
    match kind {
        AccountKind::Taproot => 66,
        AccountKind::NestedSegWit => 23 * 4 + 108,
        AccountKind::Legacy => 107 * 4,
        AccountKind::SegWit | AccountKind::Single => 108,
    }
}

/// 其他钱包的输入只知道 script,看不出类型的按 SegWit 估算.
fn script_kind(script: &Script) -> AccountKind {
    if script.is_p2tr() {
        AccountKind::Taproot
    } else if script.is_p2sh() {
        AccountKind::NestedSegWit
    } else if script.is_p2pkh() {
        AccountKind::Legacy
    } else {
        AccountKind::SegWit
    }
}

/// 签名后的虚拟大小.签名长度有 1 字节浮动,所以费率是估算值.
fn estimated_vsize(prepared: &Prepared) -> u64 {
    let spending: u64 = prepared
        .inputs
        .iter()
        .map(|input| spend_weight(script_kind(&input.prevout.script_pubkey)))
        .sum();
    let witness = prepared
        .inputs
        .iter()
        .any(|i| script_kind(&i.prevout.script_pubkey) != AccountKind::Legacy);
    let base = prepared.psbt.unsigned_tx.weight().to_wu();
    (base + spending + if witness { 2 } else { 0 }).div_ceil(4)
}

fn joined(labels: impl Iterator<Item = String>) -> String {
    let mut unique: Vec<String> = Vec::new();
    for label in labels {
        if !unique.contains(&label) {
            unique.push(label);
        }
    }
    unique.join(",")
}

pub fn review(network: Network, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    if request.kind != KIND_TRANSACTION {
        return Err(Forbid::RawHashSigning);
    }
    let prepared = prepare(network, &request.payload, ctx.keys.fingerprint)?;
    let tx = &prepared.psbt.unsigned_tx;
    let native = ctx.manifest.native(network.id()).ok_or(Forbid::UnknownNetwork)?;
    let coin = |value: u64| {
        format!(
            "{} {}",
            format_units(&value.to_string(), native.decimals),
            native.symbol
        )
    };
    let amount_view = |value: u64| AmountView {
        text: format_units(&value.to_string(), native.decimals),
        symbol: Some(native.symbol.clone()),
        asset: Some(native.key()),
        unlimited: false,
    };
    let mut builder = ReviewBuilder::default();

    let mut foreign = Vec::new();
    let mut payer_rows = Vec::new();
    for (index, input) in prepared.inputs.iter().enumerate() {
        let Some(own) = &input.own else {
            foreign.push(input.prevout.value.to_sat());
            continue;
        };
        if derived_script(ctx, &own.path, own.kind)? != input.prevout.script_pubkey {
            return Err(Forbid::AddressMismatch);
        }
        if !own.sighash.commits_everything() {
            builder.alerts.push(format!(
                "第 {} 个输入的 sighash 是 {},交易内容可能被改动",
                index + 1,
                own.sighash.label()
            ));
        }
        let address = address_text(&input.prevout.script_pubkey, network).ok_or(Forbid::AddressMismatch)?;
        payer_rows.push(Row::mono("付款账户", address));
        payer_rows.push(Row::mono("派生路径", own.path.to_string()));
        payer_rows.push(Row::text("输入金额", coin(input.prevout.value.to_sat())));
    }
    if !foreign.is_empty() {
        builder.alerts.push(format!("有 {} 个输入不属于本钱包", foreign.len()));
    }
    let fee = prepared.fee()?;

    let mut sent = 0u64;
    let mut returned = 0u64;
    let mut external = Vec::new();
    let mut change_actions = Vec::new();
    let mut highest = None;
    for (out, meta) in tx.output.iter().zip(&prepared.psbt.outputs) {
        let value = out.value.to_sat();
        let text = address_text(&out.script_pubkey, network);
        match change_index(out, meta, network, ctx)? {
            Some(index) => {
                returned += value;
                highest = highest.max(Some(index));
                if index > ctx.highest_change.of(network).saturating_add(policy::CHANGE_GAP) {
                    builder
                        .alerts
                        .push(format!("找零索引 {index} 跳得太远,资金可能落到不容易找回的位置"));
                }
                let address = text.ok_or(Forbid::AddressMismatch)?;
                change_actions.push(ActionView {
                    risk: Risk::Transfer,
                    title: "找零".to_string(),
                    rows: vec![Row::mono("找零地址", address), Row::text("金额", coin(value))],
                });
            }
            None => {
                sent += value;
                if text.is_none() {
                    builder.alerts.push("有输出不是标准地址,请核对原始脚本".to_string());
                }
                let shown = text.unwrap_or_else(|| out.script_pubkey.as_bytes().to_lower_hex_string());
                external.push(ActionView {
                    risk: Risk::Transfer,
                    title: Risk::Transfer.label().to_string(),
                    rows: vec![Row::mono("收款地址", shown), Row::text("金额", coin(value))],
                });
            }
        }
    }
    builder.change_index = highest;

    let over_percent = u128::from(fee) * 100 > u128::from(sent) * u128::from(policy::BITCOIN_FEE_PERCENT);
    if over_percent || u128::from(fee) > policy::fee_ceiling(network) {
        builder.alerts.push("手续费明显偏高".to_string());
    }

    match external.as_slice() {
        [] => {
            builder.amount = Some(amount_view(returned));
            builder.actions.push(ActionView {
                risk: Risk::Transfer,
                title: "转给自己".to_string(),
                rows: vec![Row::text("金额", coin(returned))],
            });
        }
        [only] => {
            builder.amount = Some(amount_view(sent));
            builder.counterparty_label = Some("收款地址".to_string());
            builder.counterparty = Some(only.rows[0].value.clone());
        }
        _ => builder.amount = Some(amount_view(sent)),
    }
    builder.actions.append(&mut external);
    builder.actions.append(&mut change_actions);

    let owns = || prepared.inputs.iter().filter_map(|input| input.own.as_ref());
    builder.rows.push(Row::text("网络", network.label()));
    builder.rows.append(&mut payer_rows);
    builder.rows.push(Row::text(
        "地址类型",
        joined(owns().map(|own| kind_label(own.kind).to_string())),
    ));
    if !foreign.is_empty() {
        let value = foreign.iter().sum::<u64>();
        builder.rows.push(Row::text(
            "其他钱包的输入",
            format!("{} 个,共 {}", foreign.len(), coin(value)),
        ));
    }
    builder.rows.push(Row::text("手续费", coin(fee)));
    let rate = u128::from(fee) * 100 / u128::from(estimated_vsize(&prepared));
    builder.rows.push(Row::text(
        "费率",
        format!("{} sat/vB", format_units(&rate.to_string(), 2)),
    ));
    builder
        .rows
        .push(Row::text("sighash", joined(owns().map(|own| own.sighash.label()))));
    if tx.lock_time != absolute::LockTime::ZERO {
        builder
            .rows
            .push(Row::text("锁定时间", tx.lock_time.to_consensus_u32().to_string()));
    }
    builder.raw = serialize(tx)
        .chunks(32)
        .map(|c| RawLine {
            text: c.to_lower_hex_string(),
            note: None,
        })
        .collect();
    Ok(builder.finish(network.id()))
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
/// 只知道总额,不知道收款地址,所以按一个最长的收款输出估算.一笔发给多个地址时,
/// 每多一个输出要由调用方多留手续费.
pub fn select_coins(utxos: &[Utxo], amount: u64, fee_rate: u64, change_kind: AccountKind) -> Result<Selection, Error> {
    if amount == 0 || fee_rate == 0 {
        return Err(Error::InvalidParams);
    }
    let change_weight = output_weight(change_kind)?;
    let fee_for = |weight: u64| weight.div_ceil(4).checked_mul(fee_rate).ok_or(Error::InvalidParams);
    let mut sorted = utxos
        .iter()
        .map(|utxo| Ok((utxo, INPUT_WEIGHT + spend_weight(bitcoin_kind(&utxo.path)?))))
        .collect::<Result<Vec<_>, Error>>()?;
    sorted.sort_by_key(|(utxo, _)| std::cmp::Reverse(utxo.amount));

    let mut weight = OVERHEAD_WEIGHT + PAYMENT_WEIGHT;
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

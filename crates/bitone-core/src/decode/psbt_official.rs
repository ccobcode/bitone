//! 官方解码器.遍历器负责字节范围,官方类型负责含义,两边读到的条目数必须相同.
//!
//! 这里的解构都不写 `..`,`bitcoin`,`psbt-v2` 增加字段时编译失败,这是 C3.

use super::Reject;
use super::psbt::{Map, Record, ValueCodec};
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::psbt::{self, Psbt, PsbtSighashType, raw};
use bitcoin::{Amount, OutPoint, Script, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute};
use psbt_v2::v2;

/// 官方解码器放进键值表的条目数,全局映射.
fn global_entries(psbt: &Psbt) -> usize {
    let Psbt {
        unsigned_tx: _unsigned_tx,
        version: _version,
        xpub,
        proprietary,
        unknown,
        inputs: _inputs,
        outputs: _outputs,
    } = psbt;
    xpub.len() + proprietary.len() + unknown.len()
}

fn input_entries(input: &psbt::Input) -> usize {
    let psbt::Input {
        non_witness_utxo,
        witness_utxo,
        partial_sigs,
        sighash_type,
        redeem_script,
        witness_script,
        bip32_derivation,
        final_script_sig,
        final_script_witness,
        ripemd160_preimages,
        sha256_preimages,
        hash160_preimages,
        hash256_preimages,
        tap_key_sig,
        tap_script_sigs,
        tap_scripts,
        tap_key_origins,
        tap_internal_key,
        tap_merkle_root,
        proprietary,
        unknown,
    } = input;
    let optional = [
        non_witness_utxo.is_some(),
        witness_utxo.is_some(),
        sighash_type.is_some(),
        redeem_script.is_some(),
        witness_script.is_some(),
        final_script_sig.is_some(),
        final_script_witness.is_some(),
        tap_key_sig.is_some(),
        tap_internal_key.is_some(),
        tap_merkle_root.is_some(),
    ];
    let maps = [
        partial_sigs.len(),
        bip32_derivation.len(),
        ripemd160_preimages.len(),
        sha256_preimages.len(),
        hash160_preimages.len(),
        hash256_preimages.len(),
        tap_script_sigs.len(),
        tap_scripts.len(),
        tap_key_origins.len(),
        proprietary.len(),
        unknown.len(),
    ];
    optional.iter().filter(|present| **present).count() + maps.iter().sum::<usize>()
}

fn output_entries(output: &psbt::Output) -> usize {
    let psbt::Output {
        redeem_script,
        witness_script,
        bip32_derivation,
        tap_internal_key,
        tap_tree,
        tap_key_origins,
        proprietary,
        unknown,
    } = output;
    let optional = [
        redeem_script.is_some(),
        witness_script.is_some(),
        tap_internal_key.is_some(),
        tap_tree.is_some(),
    ];
    let maps = [
        bip32_derivation.len(),
        tap_key_origins.len(),
        proprietary.len(),
        unknown.len(),
    ];
    optional.iter().filter(|present| **present).count() + maps.iter().sum::<usize>()
}

/// 遍历器和官方解码器读到的条目数相同.解析器之间的差异是攻击面,这里不留.
pub(super) fn agrees(psbt: &Psbt, global: &Map<'_>, inputs: &[Map<'_>], outputs: &[Map<'_>]) -> Result<(), Reject> {
    let same = global.entries() == global_entries(psbt)
        && inputs.len() == psbt.inputs.len()
        && outputs.len() == psbt.outputs.len()
        && inputs
            .iter()
            .zip(&psbt.inputs)
            .all(|(map, input)| map.entries() == input_entries(input))
        && outputs
            .iter()
            .zip(&psbt.outputs)
            .all(|(map, output)| map.entries() == output_entries(output));
    if same { Ok(()) } else { Err(Reject::Malformed) }
}

/// 未签名交易的共识编码.没有输入的交易 `serialize` 会写成见证格式,这和 PSBT 要求的旧格式不同.
pub(super) fn unsigned_bytes(tx: &Transaction) -> Vec<u8> {
    let Transaction {
        version,
        lock_time,
        input,
        output,
    } = tx;
    [
        serialize(version),
        serialize(input),
        serialize(output),
        serialize(lock_time),
    ]
    .concat()
}

/// 金额 0 和空脚本都是合法输出,比如 `OP_RETURN`,而 `psbt-v2` 0.3.0 把它们当成缺失.
/// 这里换成占位值让它解码,字段的值以遍历器读到的为准.
fn placeholder_for_psbt_v2(record: &Record<'_>) -> Option<Vec<u8>> {
    let bytes = record.value_bytes();
    if record.has_codec(ValueCodec::Amount) && deserialize::<u64>(bytes).is_ok_and(|sats| sats == 0) {
        return Some(serialize(&1u64));
    }
    if record.is_output_script() && bytes.is_empty() {
        return Some(ScriptBuf::new_op_return([]).into_bytes());
    }
    None
}

/// 交给 `psbt-v2` 的字节.映射内的键序不固定,而它在 `INPUT_COUNT` 排在 `OUTPUT_COUNT` 之后时报重复键,所以按键排序.
fn sorted_map(map: &Map<'_>, buffer: &mut Vec<u8>) {
    let mut records: Vec<&Record<'_>> = map.records.iter().collect();
    records.sort_by_key(|record| record.sort_key());
    for record in records {
        let value = placeholder_for_psbt_v2(record).unwrap_or_else(|| record.value_bytes().to_vec());
        buffer.extend_from_slice(record.raw_key());
        buffer.extend(serialize(&value));
    }
    buffer.push(0);
}

fn raw_key(key: psbt_v2::raw::Key) -> raw::Key {
    let psbt_v2::raw::Key { type_value, key } = key;
    raw::Key { type_value, key }
}

fn proprietary_key(key: psbt_v2::raw::ProprietaryKey) -> raw::ProprietaryKey {
    let psbt_v2::raw::ProprietaryKey { prefix, subtype, key } = key;
    raw::ProprietaryKey { prefix, subtype, key }
}

fn sighash(ty: psbt_v2::PsbtSighashType) -> PsbtSighashType {
    PsbtSighashType::from_u32(ty.to_u32())
}

/// `psbt-v2` 的输入规整成交易输入和 `bitcoin` 的键值表.没有 `SEQUENCE` 时是 final,
/// BIP-370 这样规定,而 `psbt-v2` 0.3.0 的 `unsigned_tx` 取 0,所以不用它.
fn v0_input(input: v2::Input) -> (TxIn, psbt::Input) {
    let v2::Input {
        previous_txid,
        spent_output_index,
        sequence,
        min_time: _min_time,
        min_height: _min_height,
        non_witness_utxo,
        witness_utxo,
        partial_sigs,
        sighash_type,
        redeem_script,
        witness_script,
        bip32_derivations,
        final_script_sig,
        final_script_witness,
        ripemd160_preimages,
        sha256_preimages,
        hash160_preimages,
        hash256_preimages,
        tap_key_sig,
        tap_script_sigs,
        tap_scripts,
        tap_key_origins,
        tap_internal_key,
        tap_merkle_root,
        proprietaries,
        unknowns,
    } = input;
    let txin = TxIn {
        previous_output: OutPoint {
            txid: previous_txid,
            vout: spent_output_index,
        },
        script_sig: ScriptBuf::new(),
        sequence: sequence.unwrap_or(Sequence::MAX),
        witness: Witness::new(),
    };
    let map = psbt::Input {
        non_witness_utxo,
        witness_utxo,
        partial_sigs,
        sighash_type: sighash_type.map(sighash),
        redeem_script,
        witness_script,
        bip32_derivation: bip32_derivations
            .into_iter()
            .map(|(key, source)| (key.inner, source))
            .collect(),
        final_script_sig,
        final_script_witness,
        ripemd160_preimages,
        sha256_preimages,
        hash160_preimages,
        hash256_preimages,
        tap_key_sig,
        tap_script_sigs,
        tap_scripts,
        tap_key_origins,
        tap_internal_key,
        tap_merkle_root,
        proprietary: proprietaries
            .into_iter()
            .map(|(key, value)| (proprietary_key(key), value))
            .collect(),
        unknown: unknowns.into_iter().map(|(key, value)| (raw_key(key), value)).collect(),
    };
    (txin, map)
}

fn v0_output(output: v2::Output) -> (TxOut, psbt::Output) {
    let v2::Output {
        amount,
        script_pubkey,
        redeem_script,
        witness_script,
        bip32_derivations,
        tap_internal_key,
        tap_tree,
        tap_key_origins,
        proprietaries,
        unknowns,
    } = output;
    let txout = TxOut {
        value: amount,
        script_pubkey,
    };
    let map = psbt::Output {
        redeem_script,
        witness_script,
        bip32_derivation: bip32_derivations
            .into_iter()
            .map(|(key, source)| (key.inner, source))
            .collect(),
        tap_internal_key,
        tap_tree,
        tap_key_origins,
        proprietary: proprietaries
            .into_iter()
            .map(|(key, value)| (proprietary_key(key), value))
            .collect(),
        unknown: unknowns.into_iter().map(|(key, value)| (raw_key(key), value)).collect(),
    };
    (txout, map)
}

/// 用 `psbt-v2` 解码,再规整成 `bitcoin` 的类型.核对,签名,核验都只看规整后的结果.
pub(super) fn decode_v2(
    head: &[u8],
    global: &Map<'_>,
    inputs: &[Map<'_>],
    outputs: &[Map<'_>],
) -> Result<Psbt, Reject> {
    let mut bytes = head.to_vec();
    sorted_map(global, &mut bytes);
    inputs.iter().for_each(|map| sorted_map(map, &mut bytes));
    outputs.iter().for_each(|map| sorted_map(map, &mut bytes));
    let decoded = v2::Psbt::deserialize(&bytes).map_err(|_| Reject::Malformed)?;
    let lock_time = decoded.determine_lock_time().map_err(|_| Reject::Malformed)?;
    let v2::Psbt {
        global: official,
        inputs: official_inputs,
        outputs: official_outputs,
    } = decoded;
    let v2::Global {
        version,
        tx_version,
        fallback_lock_time: _fallback_lock_time,
        tx_modifiable_flags: _tx_modifiable_flags,
        input_count: _input_count,
        output_count: _output_count,
        xpubs,
        proprietaries,
        unknowns,
    } = official;
    let (input, input_maps): (Vec<_>, Vec<_>) = official_inputs.into_iter().map(v0_input).unzip();
    let (mut output, output_maps): (Vec<_>, Vec<_>) = official_outputs.into_iter().map(v0_output).unzip();
    for (out, map) in output.iter_mut().zip(outputs) {
        if map
            .records
            .iter()
            .any(|record| placeholder_for_psbt_v2(record).is_some())
        {
            *out = txout_from_records(map)?;
        }
    }
    Ok(Psbt {
        unsigned_tx: Transaction {
            version: tx_version,
            lock_time,
            input,
            output,
        },
        version: version.to_u32(),
        xpub: xpubs,
        proprietary: proprietaries
            .into_iter()
            .map(|(key, value)| (proprietary_key(key), value))
            .collect(),
        unknown: unknowns.into_iter().map(|(key, value)| (raw_key(key), value)).collect(),
        inputs: input_maps,
        outputs: output_maps,
    })
}

fn txout_from_records(map: &Map<'_>) -> Result<TxOut, Reject> {
    let amount = map.value_of(ValueCodec::Amount).ok_or(Reject::Malformed)?;
    let script = map.output_script().ok_or(Reject::Malformed)?;
    Ok(TxOut {
        value: Amount::from_sat(deserialize(amount).map_err(|_| Reject::Malformed)?),
        script_pubkey: Script::from_bytes(script).to_owned(),
    })
}

/// 遍历器读到的 v2 交易字段拼成的交易.C2 拿它和官方解码的结果比较.
pub(super) fn transaction_from_records(
    global: &Map<'_>,
    inputs: &[Map<'_>],
    outputs: &[Map<'_>],
    lock_time: absolute::LockTime,
) -> Result<Transaction, Reject> {
    let bad = |_| Reject::Malformed;
    let version = global.value_of(ValueCodec::TxVersion).ok_or(Reject::Malformed)?;
    let input = inputs
        .iter()
        .map(|map| {
            let txid: Txid = deserialize(map.value_of(ValueCodec::Txid).ok_or(Reject::Malformed)?).map_err(bad)?;
            let vout: u32 = deserialize(map.value_of(ValueCodec::Uint32).ok_or(Reject::Malformed)?).map_err(bad)?;
            let sequence = match map.value_of(ValueCodec::Sequence) {
                Some(bytes) => deserialize(bytes).map_err(bad)?,
                None => Sequence::MAX,
            };
            Ok(TxIn {
                previous_output: OutPoint { txid, vout },
                script_sig: ScriptBuf::new(),
                sequence,
                witness: Witness::new(),
            })
        })
        .collect::<Result<Vec<_>, Reject>>()?;
    let output = outputs.iter().map(txout_from_records).collect::<Result<Vec<_>, _>>()?;
    Ok(Transaction {
        version: deserialize(version).map_err(bad)?,
        lock_time,
        input,
        output,
    })
}

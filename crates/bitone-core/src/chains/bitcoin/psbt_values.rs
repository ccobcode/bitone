//! PSBT 键和值的解码.格式来自 assets/bitcoin/psbt-schema.json 的 `key`,`value`,取值都用官方类型读.

#![deny(clippy::wildcard_enum_match_arm, clippy::wildcard_in_or_patterns)]

use super::psbt::{ConsensusRead, Entry, KeyCodec, Origin, ValueCodec, coin};
use super::script::{
    inside, output_fields, script_node, script_type_node, sequence_node, transaction_nodes, txid_value, uint,
    version_node,
};
use crate::Mode;
use crate::decode::{Cursor, Extent, Field, Node, Reject, Source, Value};
use bitcoin::bip32::{ChildNumber, Xpub};
use bitcoin::consensus::encode::{VarInt, deserialize};
use bitcoin::hex::DisplayHex;
use bitcoin::psbt::PsbtSighashType;
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::taproot::{ControlBlock, LeafVersion};
use bitcoin::{PublicKey, Script, Sequence, Transaction, TxOut, Txid, Witness, absolute, ecdsa, taproot, transaction};

fn malformed<E>(_: E) -> Reject {
    Reject::Malformed
}

fn path_text(levels: impl Iterator<Item = u32>) -> String {
    let parts = levels.map(|level| ChildNumber::from(level).to_string());
    std::iter::once("m".to_string())
        .chain(parts)
        .collect::<Vec<_>>()
        .join("/")
}

/// `<4 字节指纹> <小端 u32 路径>*`.
fn key_source_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let fingerprint: [u8; 4] = cursor.read()?;
    let mut levels = Vec::new();
    while !cursor.is_empty() {
        levels.push(cursor.read::<u32>()?);
    }
    Ok(vec![
        Node::new("master_fingerprint", Value::Bytes(fingerprint.to_vec()), source),
        Node::new("derivation_path", Value::Text(path_text(levels.into_iter())), source),
    ])
}

fn tap_key_source_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let VarInt(count) = cursor.read()?;
    let hashes = (0..count)
        .map(|_| {
            let hash: [u8; 32] = cursor.read()?;
            Ok(Node::new("leaf_hash", Value::Hash(hash.to_vec()), source))
        })
        .collect::<Result<Vec<_>, Reject>>()?;
    let mut rows = vec![Node::new("leaf_hashes", Value::List, source).children(hashes)];
    rows.extend(key_source_rows(cursor, source)?);
    Ok(rows)
}

fn tap_tree_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let mut leaves = Vec::new();
    while !cursor.is_empty() {
        let depth: u8 = cursor.read()?;
        let version: u8 = cursor.read()?;
        let VarInt(length) = cursor.read()?;
        let script = cursor.take(length)?;
        LeafVersion::from_consensus(version).map_err(malformed)?;
        let rows = vec![
            Node::new("depth", uint(depth.into()), source),
            Node::new("leaf_version", uint(version.into()), source),
            script_node("leaf_script", Script::from_bytes(script.bytes), source),
        ];
        leaves.push(Node::new("tap_leaf", Value::Struct, source).children(rows));
    }
    Ok(leaves)
}

/// `<1 字节长度的 BIP-353 名字> <RFC 9102 证明>`.
fn dnssec_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let length: u8 = cursor.read()?;
    let name = cursor.take(length.into())?;
    let name = std::str::from_utf8(name.bytes).map_err(malformed)?;
    let proof = cursor.rest();
    Ok(vec![
        Node::new("dnssec_name", Value::Text(name.to_string()), source),
        Node::new("dnssec_rfc9102", Value::Bytes(proof.bytes.to_vec()), source),
    ])
}

fn leaf_script_rows(field: &Field<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let (version, script) = field.bytes.split_last().ok_or(Reject::Malformed)?;
    LeafVersion::from_consensus(*version).map_err(malformed)?;
    Ok(vec![
        script_node("leaf_script", Script::from_bytes(script), source),
        Node::new("leaf_version", uint((*version).into()), source),
    ])
}

/// 花掉的输出单独显示,前序交易的其余内容折在后面.
fn prev_tx_children(tx: &Transaction, vout: Option<u32>, mode: Option<Mode>, own: Source) -> Vec<Node> {
    let mut rows = vec![Node::new("txid", txid_value(&tx.compute_txid()), Source::Derived)];
    let spent = vout.and_then(|vout| tx.output.get(vout as usize));
    if let Some(spent) = spent {
        let mut details = output_fields(spent, mode, Source::Derived);
        details.push(script_type_node(&spent.script_pubkey));
        rows.push(Node::new("spent_output", Value::Struct, Source::Derived).children(details));
    }
    rows.extend(transaction_nodes(tx, mode, false, inside(own)));
    rows
}

/// 值节点.`tx` 是这笔要签的交易,`vout` 是这个输入花掉的序号,两者都来自官方解码的结果.
pub(super) fn value_node(
    entry: &Entry,
    field: &Field<'_>,
    tx: &Transaction,
    vout: Option<u32>,
    mode: Option<Mode>,
) -> Result<Node, Reject> {
    let own = entry.source.own(field.span);
    let keyed = entry.key != KeyCodec::None;
    let within = inside(own);
    let name = |fallback: &'static str| if keyed { fallback } else { entry.name.as_str() };
    let leaf = |value: Value, fallback: &'static str| Ok(Node::new(name(fallback), value, own));
    let group =
        |rows: Vec<Node>, fallback: &'static str| Ok(Node::new(name(fallback), Value::Struct, own).children(rows));
    let bytes = field.bytes;
    match entry.value {
        ValueCodec::UnsignedTx => group(transaction_nodes(tx, mode, true, within), "value"),
        ValueCodec::PrevTx => {
            let prev: Transaction = deserialize(bytes).map_err(malformed)?;
            group(prev_tx_children(&prev, vout, mode, own), "value")
        }
        ValueCodec::TxOut => {
            let out: TxOut = deserialize(bytes).map_err(malformed)?;
            let mut rows = output_fields(&out, mode, within);
            rows.push(script_type_node(&out.script_pubkey));
            group(rows, "value")
        }
        ValueCodec::TxVersion => {
            let version: transaction::Version = deserialize(bytes).map_err(malformed)?;
            Ok(version_node(version, true, own))
        }
        ValueCodec::PsbtVersion | ValueCodec::Uint32 => {
            let number: u32 = deserialize(bytes).map_err(malformed)?;
            leaf(uint(number.into()), "value")
        }
        ValueCodec::InputCount | ValueCodec::OutputCount => {
            let VarInt(count) = deserialize(bytes).map_err(malformed)?;
            leaf(uint(count), "value")
        }
        ValueCodec::Script => Ok(script_node(name("value"), Script::from_bytes(bytes), own)),
        ValueCodec::Witness => {
            let witness: Witness = deserialize(bytes).map_err(malformed)?;
            let rows = witness
                .iter()
                .map(|item| Node::new("witness_item", Value::Bytes(item.to_vec()), within))
                .collect();
            Ok(Node::new(name("value"), Value::List, own).children(rows))
        }
        ValueCodec::Sighash => {
            let number: u32 = deserialize(bytes).map_err(malformed)?;
            leaf(Value::Enum(PsbtSighashType::from_u32(number).to_string()), "value")
        }
        ValueCodec::Sequence => {
            let sequence: Sequence = deserialize(bytes).map_err(malformed)?;
            Ok(sequence_node(sequence, tx.version, true, own))
        }
        ValueCodec::Amount => {
            let sats: u64 = deserialize(bytes).map_err(malformed)?;
            leaf(coin(sats), "value")
        }
        ValueCodec::Flags => {
            let flags: u8 = deserialize(bytes).map_err(malformed)?;
            let node = Node::new(name("value"), uint(flags.into()), own);
            // 只有非 0 才告警,没有标志时这一行只是显示.
            Ok(match (&entry.action, flags) {
                (Some(action), 1..) => node.action(action.as_str()),
                (Some(_) | None, _) => node,
            })
        }
        ValueCodec::Txid => {
            let txid: Txid = deserialize(bytes).map_err(malformed)?;
            leaf(txid_value(&txid), "value")
        }
        ValueCodec::TimeLock => {
            let number: u32 = deserialize(bytes).map_err(malformed)?;
            absolute::Time::from_consensus(number).map_err(malformed)?;
            leaf(Value::Time(i64::from(number) * 1000), "value")
        }
        ValueCodec::HeightLock => {
            let number: u32 = deserialize(bytes).map_err(malformed)?;
            absolute::Height::from_consensus(number).map_err(malformed)?;
            if number == 0 {
                return Err(Reject::Malformed);
            }
            leaf(uint(number.into()), "value")
        }
        ValueCodec::KeySource => group(key_source_rows(&mut Cursor::new(field), within)?, "key_source"),
        ValueCodec::TapKeySource => group(tap_key_source_rows(&mut Cursor::new(field), within)?, "key_source"),
        ValueCodec::EcdsaSignature => {
            ecdsa::Signature::from_slice(bytes).map_err(malformed)?;
            leaf(Value::Bytes(bytes.to_vec()), "signature")
        }
        ValueCodec::TapSignature => {
            taproot::Signature::from_slice(bytes).map_err(malformed)?;
            leaf(Value::Bytes(bytes.to_vec()), "signature")
        }
        ValueCodec::Xonly => {
            XOnlyPublicKey::from_slice(bytes).map_err(malformed)?;
            leaf(Value::Bytes(bytes.to_vec()), "value")
        }
        ValueCodec::Hash32 => {
            let hash: [u8; 32] = deserialize(bytes).map_err(malformed)?;
            leaf(Value::Hash(hash.to_vec()), "value")
        }
        ValueCodec::Preimage => leaf(Value::Bytes(bytes.to_vec()), "preimage"),
        ValueCodec::LeafScript => group(leaf_script_rows(field, within)?, "value"),
        ValueCodec::TapTree => {
            let leaves = tap_tree_rows(&mut Cursor::new(field), within)?;
            Ok(Node::new(name("value"), Value::List, own).children(leaves))
        }
        ValueCodec::Utf8 => leaf(
            match std::str::from_utf8(bytes) {
                Ok(text) => Value::Text(text.to_string()),
                Err(_) => Value::Bytes(bytes.to_vec()),
            },
            "value",
        ),
        ValueCodec::DnssecProof => group(dnssec_rows(&mut Cursor::new(field), within)?, "value"),
        ValueCodec::Opaque => leaf(Value::Opaque(bytes.to_vec()), "value"),
    }
}

/// `<紧凑长度的前缀> <子类型> <子键>`.
fn proprietary_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let VarInt(length) = cursor.read()?;
    let identifier = cursor.take(length)?;
    let VarInt(subtype) = cursor.read()?;
    let subkey = cursor.rest();
    let text = match std::str::from_utf8(identifier.bytes) {
        Ok(text) => Value::Text(text.to_string()),
        Err(_) => Value::Text(identifier.bytes.to_lower_hex_string()),
    };
    Ok(vec![
        Node::new("identifier", text, source),
        Node::new("subtype", uint(subtype), source),
        Node::new("subkey", Value::Bytes(subkey.bytes.to_vec()), source),
    ])
}

fn xonly_leaf_rows(cursor: &mut Cursor<'_>, source: Source) -> Result<Vec<Node>, Reject> {
    let key: [u8; 32] = cursor.read()?;
    let leaf: [u8; 32] = cursor.read()?;
    if !cursor.is_empty() {
        return Err(Reject::Malformed);
    }
    XOnlyPublicKey::from_slice(&key).map_err(malformed)?;
    Ok(vec![
        Node::new("xonly_pubkey", Value::Bytes(key.to_vec()), source),
        Node::new("leaf_hash", Value::Hash(leaf.to_vec()), source),
    ])
}

/// 键数据的节点.键的字节都在 `context` 里.
pub(super) fn key_node(codec: KeyCodec, field: &Field<'_>) -> Result<Node, Reject> {
    let own = Origin::Context.own(field.span);
    let within = Source::Context(Extent::Inherited);
    let bytes = field.bytes;
    let leaf = |name: &str, value: Value| Ok(Node::new(name, value, own));
    let group = |name: &str, rows: Vec<Node>| Ok(Node::new(name, Value::Struct, own).children(rows));
    match codec {
        KeyCodec::None => Err(Reject::Malformed),
        KeyCodec::Pubkey => {
            PublicKey::from_slice(bytes).map_err(malformed)?;
            leaf("pubkey", Value::Bytes(bytes.to_vec()))
        }
        KeyCodec::Xpub => {
            let xpub = Xpub::decode(bytes).map_err(malformed)?;
            leaf("xpub_key", Value::Text(xpub.to_string()))
        }
        KeyCodec::Xonly => {
            XOnlyPublicKey::from_slice(bytes).map_err(malformed)?;
            leaf("xonly_pubkey", Value::Bytes(bytes.to_vec()))
        }
        KeyCodec::XonlyLeaf => group("tap_script_key", xonly_leaf_rows(&mut Cursor::new(field), within)?),
        KeyCodec::ControlBlock => {
            ControlBlock::decode(bytes).map_err(malformed)?;
            leaf("control_block", Value::Bytes(bytes.to_vec()))
        }
        KeyCodec::Hash => leaf("hash", Value::Hash(bytes.to_vec())),
        KeyCodec::Proprietary => group("proprietary_key", proprietary_rows(&mut Cursor::new(field), within)?),
        KeyCodec::Opaque => leaf("key", Value::Opaque(bytes.to_vec())),
    }
}

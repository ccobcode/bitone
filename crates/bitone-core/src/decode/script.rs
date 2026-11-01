//! Bitcoin 脚本和交易字段.这里只看字节本身的含义,类别由规则表给出,见 docs/10-tx-decoding.md 第 5.4 节.

use super::psbt::coin;
use super::{Extent, Node, Source, Value};
use crate::Mode;
use bitcoin::blockdata::opcodes::all::OP_CHECKMULTISIG;
use bitcoin::blockdata::opcodes::{Class, ClassifyContext, Opcode};
use bitcoin::hashes::Hash;
use bitcoin::script::{Builder, Instruction};
use bitcoin::{
    Address, Network, OutPoint, PublicKey, Script, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    WitnessVersion, absolute, opcodes, relative, transaction,
};

/// BIP-433 的 P2A 见证程序.
const ANCHOR_PROGRAM: [u8; 2] = [0x4e, 0x73];

/// 官方类型内部的字段继承父节点的字节范围,`derived` 节点的子节点也是 `derived`.
pub(super) fn inside(source: Source) -> Source {
    match source {
        Source::Signed(_) => Source::Signed(Extent::Inherited),
        Source::Context(_) => Source::Context(Extent::Inherited),
        Source::Derived | Source::Manifest | Source::Descriptor | Source::Hint => source,
    }
}

pub(super) fn uint(value: u64) -> Value {
    Value::Uint(value.to_be_bytes().to_vec())
}

/// 输出脚本的种类.第 5.4 节的矩阵按它给类别.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptClass {
    P2pkh,
    P2sh,
    P2wpkh,
    P2wsh,
    P2tr,
    P2pk,
    BareMultisig,
    Anchor,
    OpReturn,
    FutureWitness,
    Other,
}

impl ScriptClass {
    pub const ALL: [Self; 11] = [
        Self::P2pkh,
        Self::P2sh,
        Self::P2wpkh,
        Self::P2wsh,
        Self::P2tr,
        Self::P2pk,
        Self::BareMultisig,
        Self::Anchor,
        Self::OpReturn,
        Self::FutureWitness,
        Self::Other,
    ];

    pub fn of(script: &Script) -> Self {
        if script.is_p2pkh() {
            Self::P2pkh
        } else if script.is_p2sh() {
            Self::P2sh
        } else if script.is_p2wpkh() {
            Self::P2wpkh
        } else if script.is_p2wsh() {
            Self::P2wsh
        } else if script.is_p2tr() {
            Self::P2tr
        } else if script.is_op_return() {
            Self::OpReturn
        } else if script.p2pk_public_key().is_some() {
            Self::P2pk
        } else if bare_multisig(script).is_some() {
            Self::BareMultisig
        } else if *script == anchor_script() {
            Self::Anchor
        } else if script
            .witness_version()
            .is_some_and(|version| version > WitnessVersion::V1)
        {
            Self::FutureWitness
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::P2pkh => "p2pkh",
            Self::P2sh => "p2sh",
            Self::P2wpkh => "p2wpkh",
            Self::P2wsh => "p2wsh",
            Self::P2tr => "p2tr",
            Self::P2pk => "p2pk",
            Self::BareMultisig => "bare-multisig",
            Self::Anchor => "p2a",
            Self::OpReturn => "op-return",
            Self::FutureWitness => "witness-future",
            Self::Other => "unknown",
        }
    }

    pub fn has_address(self) -> bool {
        match self {
            Self::P2pkh | Self::P2sh | Self::P2wpkh | Self::P2wsh | Self::P2tr => true,
            Self::P2pk | Self::BareMultisig | Self::Anchor | Self::OpReturn | Self::FutureWitness | Self::Other => {
                false
            }
        }
    }
}

/// 规则表里输出的种类名.`OP_RETURN` 的金额不为 0 时这笔钱会被销毁,所以是单独一种.
pub fn output_action(class: ScriptClass, sats: u64) -> String {
    let burned = class == ScriptClass::OpReturn && sats != 0;
    let suffix = if burned { "-burn" } else { "" };
    ["output-", class.name(), suffix].concat()
}

fn anchor_script() -> ScriptBuf {
    Builder::new()
        .push_opcode(opcodes::all::OP_PUSHNUM_1)
        .push_slice(ANCHOR_PROGRAM)
        .into_script()
}

fn pushnum(opcode: Opcode) -> Option<usize> {
    match opcode.classify(ClassifyContext::Legacy) {
        Class::PushNum(number) => usize::try_from(number).ok(),
        Class::PushBytes(_)
        | Class::ReturnOp
        | Class::SuccessOp
        | Class::IllegalOp
        | Class::NoOp
        | Class::Ordinary(_) => None,
    }
}

/// `m <公钥>* n OP_CHECKMULTISIG` 里的公钥.`Script::is_multisig` 的计数器是 `u8`,超过 255 个推入会溢出,所以不用它.
fn bare_multisig(script: &Script) -> Option<Vec<Vec<u8>>> {
    let mut steps = script.instructions();
    let Instruction::Op(first) = steps.next()?.ok()? else {
        return None;
    };
    let required = pushnum(first)?;
    let mut keys = Vec::new();
    loop {
        match steps.next()?.ok()? {
            Instruction::PushBytes(key) => {
                PublicKey::from_slice(key.as_bytes()).ok()?;
                keys.push(key.as_bytes().to_vec());
            }
            Instruction::Op(last) => {
                let total = pushnum(last)?;
                let closed = matches!(steps.next()?.ok()?, Instruction::Op(op) if op == OP_CHECKMULTISIG);
                let valid = closed && steps.next().is_none() && total == keys.len() && required <= total;
                return valid.then_some(keys);
            }
        }
    }
}

fn pubkey_list(keys: Vec<Vec<u8>>, source: Source) -> Node {
    let rows = keys
        .into_iter()
        .map(|key| Node::new("pubkey", Value::Bytes(key), source))
        .collect();
    Node::new("pubkeys", Value::List, source).children(rows)
}

/// `OP_RETURN` 的推入数据:十六进制一定有,合法 UTF-8 时再加文本.不合规的推入留在操作码列表里.
fn op_return_data(script: &Script, source: Source) -> Node {
    let mut rows = Vec::new();
    for step in script.instructions() {
        let Ok(Instruction::PushBytes(pushed)) = step else {
            continue;
        };
        let bytes = pushed.as_bytes();
        rows.push(Node::new("data", Value::Bytes(bytes.to_vec()), source));
        if let Ok(text) = std::str::from_utf8(bytes)
            && !text.is_empty()
        {
            rows.push(Node::new("text", Value::Text(text.to_string()), source));
        }
    }
    Node::new("op_return", Value::List, source).children(rows)
}

fn witness_program(script: &Script, source: Source) -> Vec<Node> {
    let mut rows = Vec::new();
    if let Some(version) = script.witness_version() {
        rows.push(Node::new(
            "witness_version",
            uint(version.to_num().into()),
            Source::Derived,
        ));
    }
    if let Some(Ok(Instruction::PushBytes(program))) = script.instructions().nth(1) {
        rows.push(Node::new(
            "witness_program",
            Value::Bytes(program.as_bytes().to_vec()),
            source,
        ));
    }
    rows
}

/// 脚本节点,值是操作码列表.能再往下解码的脚本带子节点.
pub(super) fn script_node(name: &str, script: &Script, source: Source) -> Node {
    let within = inside(source);
    let children = match ScriptClass::of(script) {
        ScriptClass::OpReturn => vec![op_return_data(script, within)],
        ScriptClass::P2pk => script
            .p2pk_public_key()
            .map(|key| pubkey_list(vec![key.to_bytes()], within))
            .into_iter()
            .collect(),
        ScriptClass::BareMultisig => bare_multisig(script)
            .map(|keys| pubkey_list(keys, within))
            .into_iter()
            .collect(),
        ScriptClass::FutureWitness => witness_program(script, within),
        ScriptClass::P2pkh
        | ScriptClass::P2sh
        | ScriptClass::P2wpkh
        | ScriptClass::P2wsh
        | ScriptClass::P2tr
        | ScriptClass::Anchor
        | ScriptClass::Other => Vec::new(),
    };
    Node::new(name, Value::Script(script.as_bytes().to_vec()), source).children(children)
}

/// 地址的前缀只取决于私钥类型,所有测试币网络的地址格式相同.
pub fn address_network(mode: Mode) -> Network {
    match mode {
        Mode::Mainnet => Network::Bitcoin,
        Mode::Testnet => Network::Testnet,
    }
}

pub(super) fn address_node(script: &Script, mode: Option<Mode>) -> Option<Node> {
    if !ScriptClass::of(script).has_address() {
        return None;
    }
    let address = Address::from_script(script, address_network(mode?)).ok()?;
    Some(Node::new(
        "address",
        Value::Address(address.to_string()),
        Source::Derived,
    ))
}

pub(super) fn script_type_node(script: &Script) -> Node {
    Node::new(
        "script_type",
        Value::Enum(ScriptClass::of(script).name().to_string()),
        Source::Derived,
    )
}

/// 官方的显示顺序.`Txid` 的字节序和浏览器里看到的相反.
pub(super) fn txid_value(txid: &Txid) -> Value {
    let mut bytes = txid.to_byte_array();
    bytes.reverse();
    Value::Hash(bytes.to_vec())
}

/// `tagged` 为真时节点是这笔交易自己的字段,带规则表的种类名.前序交易的字段只显示.
fn tagged(node: Node, is_tagged: bool, action: impl Into<String>) -> Node {
    if is_tagged { node.action(action) } else { node }
}

pub(super) fn version_node(version: transaction::Version, top: bool, source: Source) -> Node {
    let action = match version.0 {
        1 => "version-1",
        2 => "version-2",
        3 => "version-3",
        _other => "version-other",
    };
    tagged(Node::new("version", Value::Int(version.0.into()), source), top, action)
}

pub(super) fn lock_time_node(lock_time: absolute::LockTime, all_final: bool, top: bool, source: Source) -> Node {
    let consensus = lock_time.to_consensus_u32();
    let (action, detail) = match lock_time {
        absolute::LockTime::Blocks(_) if consensus == 0 => ("lock-time-none", None),
        absolute::LockTime::Blocks(_) => (
            "lock-time-height",
            Some(Node::new("lock_time_height", uint(consensus.into()), Source::Derived)),
        ),
        absolute::LockTime::Seconds(_) => (
            "lock-time-utc",
            Some(Node::new(
                "lock_time_utc",
                Value::Time(i64::from(consensus) * 1000),
                Source::Derived,
            )),
        ),
    };
    let action = if all_final && consensus != 0 {
        "lock-time-inactive"
    } else {
        action
    };
    tagged(
        Node::new("lock_time", uint(consensus.into()), source).children(detail.into_iter().collect()),
        top,
        action,
    )
}

pub(super) fn sequence_node(sequence: Sequence, version: transaction::Version, top: bool, source: Source) -> Node {
    let action = if sequence == Sequence::MAX {
        "sequence-final"
    } else if sequence.is_rbf() {
        "sequence-rbf"
    } else {
        "sequence-no-rbf"
    };
    let relative = sequence.to_relative_lock_time().filter(|_| version.0 >= 2);
    let detail = relative.map(|lock_time| match lock_time {
        relative::LockTime::Blocks(blocks) => {
            Node::new("relative_lock_blocks", uint(blocks.value().into()), Source::Derived)
        }
        relative::LockTime::Time(intervals) => Node::new(
            "relative_lock_intervals",
            uint(intervals.value().into()),
            Source::Derived,
        ),
    });
    tagged(
        Node::new("sequence", uint(sequence.to_consensus_u32().into()), source).children(detail.into_iter().collect()),
        top,
        action,
    )
}

fn witness_list(witness: &Witness, source: Source) -> Node {
    let rows = witness
        .iter()
        .map(|item| Node::new("witness_item", Value::Bytes(item.to_vec()), source))
        .collect();
    Node::new("witness", Value::List, source).children(rows)
}

fn input_node(txin: &TxIn, version: transaction::Version, top: bool, source: Source) -> Node {
    let TxIn {
        previous_output,
        script_sig,
        sequence,
        witness,
    } = txin;
    let OutPoint { txid, vout } = previous_output;
    let rows = vec![
        Node::new("txid", txid_value(txid), source),
        Node::new("vout", uint((*vout).into()), source),
        script_node("script_sig", script_sig, source),
        sequence_node(*sequence, version, top, source),
        witness_list(witness, source),
    ];
    tagged(Node::new("input", Value::Struct, source).children(rows), top, "input")
}

/// 金额,脚本和地址.
pub(super) fn output_fields(output: &TxOut, mode: Option<Mode>, source: Source) -> Vec<Node> {
    let TxOut { value, script_pubkey } = output;
    let mut rows = vec![
        Node::new("amount", coin(value.to_sat()), source),
        script_node("script", script_pubkey, source),
    ];
    rows.extend(address_node(script_pubkey, mode));
    rows
}

fn output_node(output: &TxOut, mode: Option<Mode>, top: bool, source: Source) -> Node {
    let class = ScriptClass::of(&output.script_pubkey);
    let rows = output_fields(output, mode, source);
    tagged(
        Node::new("output", Value::Struct, source).children(rows),
        top,
        output_action(class, output.value.to_sat()),
    )
}

/// 交易的全部字段.`top` 为真时是这笔要签的交易.
pub(super) fn transaction_nodes(tx: &Transaction, mode: Option<Mode>, top: bool, source: Source) -> Vec<Node> {
    let Transaction {
        version,
        lock_time,
        input,
        output,
    } = tx;
    let all_final = input.iter().all(|txin| txin.sequence == Sequence::MAX);
    let inputs = input
        .iter()
        .map(|txin| input_node(txin, *version, top, source))
        .collect();
    let outputs = output.iter().map(|out| output_node(out, mode, top, source)).collect();
    vec![
        version_node(*version, top, source),
        lock_time_node(*lock_time, all_final, top, source),
        Node::new("inputs", Value::List, source).children(inputs),
        Node::new("outputs", Value::List, source).children(outputs),
    ]
}

/// 花费这种输出要加的重量.解锁脚本每字节算 4,见证每字节算 1,签名按最长算.其他类型按 SegWit 估算.
pub fn spend_weight(class: ScriptClass) -> u64 {
    match class {
        ScriptClass::P2tr => 66,
        ScriptClass::P2sh => 23 * 4 + 108,
        ScriptClass::P2pkh => 107 * 4,
        ScriptClass::P2wpkh
        | ScriptClass::P2wsh
        | ScriptClass::P2pk
        | ScriptClass::BareMultisig
        | ScriptClass::Anchor
        | ScriptClass::OpReturn
        | ScriptClass::FutureWitness
        | ScriptClass::Other => 108,
    }
}

/// 签名后的虚拟大小.签名长度有 1 字节浮动,所以费率是估算值.
pub fn estimated_vsize<'a>(tx: &Transaction, spent: impl Iterator<Item = &'a Script>) -> u64 {
    let (mut spending, mut witness) = (0, false);
    for script in spent {
        let class = ScriptClass::of(script);
        spending += spend_weight(class);
        witness |= class != ScriptClass::P2pkh;
    }
    (tx.weight().to_wu() + spending + if witness { 2 } else { 0 }).div_ceil(4)
}

//! ABI 值到节点.按描述文件解码的调用数据和 EIP-712 共用,值都来自 `alloy-dyn-abi`.

use super::{Node, Source, Value};
use alloy_dyn_abi::DynSolValue;

/// 地址的写法随链不同,比如 TRON 用 Base58.
pub type AddressText<'a> = &'a dyn Fn(&[u8; 20]) -> String;

fn group(name: String, value: Value, source: Source, children: Vec<Node>) -> Node {
    Node::new(name, value, source).children(children)
}

fn items(values: &[DynSolValue], source: Source, address: AddressText<'_>) -> Vec<Node> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| abi_node(index.to_string(), value, source, address))
        .collect()
}

/// `source` 决定这棵子树能不能作为策略依据.调用数据的字段是 `descriptor`,只影响展示.
pub fn abi_node(name: String, value: &DynSolValue, source: Source, address: AddressText<'_>) -> Node {
    let shown = match value {
        DynSolValue::Bool(flag) => Value::Bool(*flag),
        DynSolValue::Int(number, _) => {
            i128::try_from(*number).map_or_else(|_| Value::Text(number.to_string()), Value::Int)
        }
        DynSolValue::Uint(number, _) => Value::Uint(number.to_be_bytes_trimmed_vec()),
        DynSolValue::FixedBytes(word, size) => Value::Hash(word.get(..*size).unwrap_or_default().to_vec()),
        DynSolValue::Address(target) => Value::Address(address(&target.into_array())),
        DynSolValue::Function(function) => Value::Bytes(function.to_vec()),
        DynSolValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
        DynSolValue::String(text) => Value::Text(text.clone()),
        DynSolValue::Array(values) | DynSolValue::FixedArray(values) => {
            return group(name, Value::List, source, items(values, source, address));
        }
        DynSolValue::Tuple(values) => return group(name, Value::Struct, source, items(values, source, address)),
        DynSolValue::CustomStruct {
            name: _type_name,
            prop_names,
            tuple,
        } => {
            let children = prop_names
                .iter()
                .zip(tuple)
                .map(|(prop, value)| abi_node(prop.clone(), value, source, address))
                .collect();
            return group(name, Value::Struct, source, children);
        }
    };
    Node::new(name, shown, source)
}

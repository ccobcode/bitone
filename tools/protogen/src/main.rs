//! 由 `schemas/` 生成 TRON 的 prost 类型和 `tron-schema.json`.默认写入 `bitone-core`,`--check` 只比较.
//! 先校验 `schemas/lock.toml` 的哈希.见 docs/10-tx-decoding.md 第 6 节.

use heck::ToUpperCamelCase;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorSet};
use protox::Compiler;
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

const GENERATED: &str = "crates/bitone-core/src/decode/generated";
const SCHEMA: &str = "crates/bitone-core/assets/tron-schema.json";
const OVERLAY: &str = "schemas/tron-fields.json";
const LOCK: &str = "schemas/lock.toml";
const ANY_PROTO: &str = "google/protobuf/any.proto";

#[derive(Deserialize)]
struct Lock {
    tron: Source,
    erc7730: Source,
}

#[derive(Deserialize)]
struct Source {
    root: String,
    files: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Overlay {
    package: String,
    root: String,
    envelope: String,
    contract_enum: String,
    type_url_prefix: String,
    owner_fields: Vec<String>,
    address_fields: Vec<String>,
    fields: BTreeMap<String, Patch>,
}

/// `tron-fields.json` 里一个字段的展示方式.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    show: Option<String>,
    action: Option<String>,
    len: Option<u32>,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    single: bool,
}

#[derive(Serialize)]
struct Field {
    number: u32,
    name: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    of: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    repeated: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    map: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    required: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    single: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    show: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    len: Option<u32>,
}

#[derive(Serialize)]
struct Contract {
    name: String,
    message: Option<String>,
}

#[derive(Serialize)]
struct Envelope {
    raw_data: i32,
    signature: i32,
}

#[derive(Serialize)]
struct Schema {
    root: String,
    envelope: Envelope,
    type_url_prefix: String,
    owner_fields: Vec<String>,
    contracts: BTreeMap<i32, Contract>,
    enums: BTreeMap<String, BTreeMap<i32, String>>,
    messages: BTreeMap<String, Vec<Field>>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").expect("写入内存不会失败");
        text
    })
}

/// 哈希不符时不生成,协议文件是下载来的数据.
fn checked_files(root: &Path, source: &Source) -> Result<Vec<String>, Box<dyn Error>> {
    let base = root.join("schemas").join(&source.root);
    for (file, digest) in &source.files {
        let actual = hex(&Sha256::digest(fs::read(base.join(file))?));
        if actual != *digest {
            return Err(format!("{file} 的哈希和 {LOCK} 不符,实际是 {actual}").into());
        }
    }
    Ok(source.files.keys().cloned().collect())
}

/// 全名到定义.全名带前导点,和 `type_name` 一致.
#[derive(Default)]
struct Index<'a> {
    messages: BTreeMap<String, &'a DescriptorProto>,
    enums: BTreeMap<String, &'a EnumDescriptorProto>,
}

impl<'a> Index<'a> {
    fn new(fds: &'a FileDescriptorSet) -> Self {
        let mut index = Self::default();
        for file in &fds.file {
            let prefix = [".", file.package()].concat();
            index.add(&prefix, &file.message_type, &file.enum_type);
        }
        index
    }

    fn add(&mut self, prefix: &str, messages: &'a [DescriptorProto], enums: &'a [EnumDescriptorProto]) {
        for item in enums {
            self.enums.insert([prefix, ".", item.name()].concat(), item);
        }
        for message in messages {
            let name = [prefix, ".", message.name()].concat();
            self.add(&name, &message.nested_type, &message.enum_type);
            self.messages.insert(name, message);
        }
    }

    fn message(&self, name: &str) -> Result<&'a DescriptorProto, Box<dyn Error>> {
        self.messages
            .get(name)
            .copied()
            .ok_or_else(|| format!("没有消息 {name}").into())
    }
}

fn full_name(package: &str, name: &str) -> String {
    [".", package, ".", name].concat()
}

fn contract_table(index: &Index<'_>, overlay: &Overlay) -> Result<BTreeMap<i32, Contract>, Box<dyn Error>> {
    let name = full_name(&overlay.package, &overlay.contract_enum);
    let values = index.enums.get(&name).ok_or("没有合约类型枚举")?;
    Ok(values
        .value
        .iter()
        .map(|value| {
            let message = full_name(&overlay.package, value.name());
            let contract = Contract {
                name: value.name().to_string(),
                message: index.messages.contains_key(&message).then_some(message),
            };
            (value.number(), contract)
        })
        .collect())
}

/// 从根消息和全部合约能走到的消息.类型表和生成的代码都只有这些.
fn reachable(index: &Index<'_>, roots: Vec<String>) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut seen = BTreeSet::new();
    let mut pending = roots;
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for field in &index.message(&name)?.field {
            if field.r#type() == Type::Message {
                pending.push(field.type_name().to_string());
            }
        }
    }
    Ok(seen)
}

fn scalar_kind(field: &FieldDescriptorProto) -> Result<&'static str, Box<dyn Error>> {
    match field.r#type() {
        Type::Int32 => Ok("int32"),
        Type::Int64 => Ok("int64"),
        Type::Bool => Ok("bool"),
        Type::Bytes => Ok("bytes"),
        Type::String => Ok("text"),
        Type::Enum => Ok("enum"),
        Type::Message => Ok("message"),
        // 第 2.4 节:新的线格式要先写进遍历器,这里失败比静默进树好.
        other => Err(format!("{} 的类型 {other:?} 遍历器不认识", field.name()).into()),
    }
}

fn field_entry(
    index: &Index<'_>,
    overlay: &Overlay,
    message: &str,
    is_contract: bool,
    field: &FieldDescriptorProto,
) -> Result<Field, Box<dyn Error>> {
    let kind = scalar_kind(field)?;
    let repeated = field.label() == Label::Repeated;
    if repeated && !matches!(kind, "message" | "bytes" | "text") {
        return Err(format!("{message}.{} 是 packed 字段,遍历器不认识", field.name()).into());
    }
    let map = kind == "message"
        && index
            .message(field.type_name())?
            .options
            .as_ref()
            .is_some_and(|o| o.map_entry());
    let is_entry = index.message(message)?.options.as_ref().is_some_and(|o| o.map_entry());
    let short = message
        .strip_prefix(&[".", &overlay.package, "."].concat())
        .unwrap_or(message);
    let key = [short.trim_start_matches('.'), ".", field.name()].concat();
    let patch = overlay.fields.get(&key).cloned().unwrap_or_default();
    let is_address = overlay.address_fields.iter().any(|name| name == field.name()) && kind == "bytes";
    let is_owner = is_contract && overlay.owner_fields.first().is_some_and(|name| name == field.name());
    Ok(Field {
        number: u32::try_from(field.number())?,
        name: field.name().to_string(),
        kind,
        of: matches!(kind, "message" | "enum").then(|| field.type_name().trim_start_matches('.').to_string()),
        repeated,
        map,
        required: patch.required || is_owner || is_entry,
        single: patch.single,
        show: patch.show.or_else(|| is_address.then(|| "address".to_string())),
        action: patch.action,
        len: patch.len,
    })
}

/// 装回签名要用的两个字段.`Transaction` 的其余字段是链上结果,不进签名.
fn envelope(index: &Index<'_>, overlay: &Overlay) -> Result<Envelope, Box<dyn Error>> {
    let message = index.message(&full_name(&overlay.package, &overlay.envelope))?;
    let number = |name: &str| {
        let field = message.field.iter().find(|field| field.name() == name);
        field
            .map(FieldDescriptorProto::number)
            .ok_or_else(|| format!("没有字段 {name}"))
    };
    Ok(Envelope {
        raw_data: number("raw_data")?,
        signature: number("signature")?,
    })
}

fn schema(index: &Index<'_>, overlay: &Overlay) -> Result<(Schema, BTreeSet<String>), Box<dyn Error>> {
    let contracts = contract_table(index, overlay)?;
    let mut roots: Vec<String> = contracts
        .values()
        .filter_map(|contract| contract.message.clone())
        .collect();
    let root = full_name(&overlay.package, &overlay.root);
    roots.push(root.clone());
    let names = reachable(index, roots)?;
    let contract_messages: BTreeSet<&String> = contracts.values().filter_map(|c| c.message.as_ref()).collect();
    let mut messages = BTreeMap::new();
    let mut enums = BTreeMap::new();
    for name in &names {
        let is_contract = contract_messages.contains(name);
        let mut fields = index
            .message(name)?
            .field
            .iter()
            .map(|field| field_entry(index, overlay, name, is_contract, field))
            .collect::<Result<Vec<_>, _>>()?;
        fields.sort_by_key(|field| field.number);
        for field in fields.iter().filter(|field| field.kind == "enum") {
            let of = field.of.clone().ok_or("枚举字段没有类型")?;
            let values = index.enums.get(&[".", &of].concat()).ok_or("没有枚举")?;
            let table = values
                .value
                .iter()
                .map(|value| (value.number(), value.name().to_string()));
            enums.insert(of, table.collect());
        }
        messages.insert(name.trim_start_matches('.').to_string(), fields);
    }
    let schema = Schema {
        root: root.trim_start_matches('.').to_string(),
        envelope: envelope(index, overlay)?,
        type_url_prefix: overlay.type_url_prefix.clone(),
        owner_fields: overlay.owner_fields.clone(),
        contracts: contracts
            .into_iter()
            .map(|(number, contract)| {
                let message = contract.message.map(|name| name.trim_start_matches('.').to_string());
                (number, Contract { message, ..contract })
            })
            .collect(),
        enums,
        messages,
    };
    Ok((schema, names))
}

/// 这条消息和它的嵌套类型引用的顶层类型.
fn references(message: &DescriptorProto, found: &mut BTreeSet<String>) {
    let names = message.field.iter().map(|field| field.type_name());
    found.extend(names.filter(|name| !name.is_empty()).map(str::to_string));
    message.nested_type.iter().for_each(|nested| references(nested, found));
}

fn top_level(name: &str, package: &str) -> Option<String> {
    let rest = name.strip_prefix(&[".", package, "."].concat())?;
    rest.split('.').next().map(str::to_string)
}

/// 只留下用得到的顶层类型,其余的类型不进产物.
fn pruned(fds: &FileDescriptorSet, names: &BTreeSet<String>, package: &str) -> FileDescriptorSet {
    let mut keep: BTreeSet<String> = names.iter().filter_map(|name| top_level(name, package)).collect();
    loop {
        let before = keep.len();
        let mut found = BTreeSet::new();
        for file in fds.file.iter().filter(|file| file.package() == package) {
            let kept = file.message_type.iter().filter(|message| keep.contains(message.name()));
            kept.for_each(|message| references(message, &mut found));
        }
        keep.extend(found.iter().filter_map(|name| top_level(name, package)));
        if keep.len() == before {
            break;
        }
    }
    let mut output = fds.clone();
    for file in output.file.iter_mut().filter(|file| file.package() == package) {
        file.message_type.retain(|message| keep.contains(message.name()));
        file.enum_type.retain(|item| keep.contains(item.name()));
    }
    output
}

/// C3:`match` 没有通配分支,合约类型枚举多一个分支这里就编译失败.
fn recode_source(contracts: &BTreeMap<i32, Contract>) -> Result<String, Box<dyn Error>> {
    let arms = contracts.values().map(|contract| {
        let variant = format_ident!("{}", contract.name.to_upper_camel_case());
        match contract.message {
            Some(_) => quote!(ContractType::#variant => Some(again::<protocol::#variant>(value))),
            None => quote!(ContractType::#variant => None),
        }
    });
    let source = quote! {
        use super::protocol::{self, transaction::contract::ContractType};
        use prost::Message;

        fn again<M: Message + Default>(value: &[u8]) -> Result<Vec<u8>, prost::DecodeError> {
            M::decode(value).map(|message| message.encode_to_vec())
        }

        pub fn recode(kind: ContractType, value: &[u8]) -> Option<Result<Vec<u8>, prost::DecodeError>> {
            match kind {
                #(#arms,)*
            }
        }
    };
    Ok(prettyplease::unparse(&syn::parse2(source)?))
}

/// 要写入的文件和内容.
type Outputs = Vec<(PathBuf, Vec<u8>)>;

fn outputs(root: &Path, scratch: &Path) -> Result<Outputs, Box<dyn Error>> {
    let lock: Lock = toml::from_str(&fs::read_to_string(root.join(LOCK))?)?;
    let overlay: Overlay = serde_json::from_slice(&fs::read(root.join(OVERLAY))?)?;
    let files = checked_files(root, &lock.tron)?;
    // 描述文件由 `bitone-core` 直接编译进去,这里只校验哈希.
    checked_files(root, &lock.erc7730)?;
    let mut compiler = Compiler::new([root.join("schemas").join(&lock.tron.root)])?;
    let fds = compiler
        .include_imports(true)
        .include_source_info(false)
        .open_files(&files)?
        .file_descriptor_set();
    if !fds.file.iter().any(|file| file.name() == ANY_PROTO) {
        return Err("缺少 google.protobuf.Any".into());
    }
    let index = Index::new(&fds);
    let (table, names) = schema(&index, &overlay)?;
    prost_build::Config::new()
        .out_dir(scratch)
        .disable_comments(["."])
        .compile_well_known_types()
        .btree_map(["."])
        .compile_fds(pruned(&fds, &names, &overlay.package))?;
    let mut produced = vec![
        (
            root.join(SCHEMA),
            [serde_json::to_vec_pretty(&table)?, b"\n".to_vec()].concat(),
        ),
        (
            root.join(GENERATED).join("recode.rs"),
            recode_source(&table.contracts)?.into_bytes(),
        ),
    ];
    for file in fs::read_dir(scratch)? {
        let path = file?.path();
        let name = path.file_name().ok_or("没有文件名")?;
        produced.push((root.join(GENERATED).join(name), fs::read(&path)?));
    }
    Ok(produced)
}

fn run() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let check = std::env::args().any(|arg| arg == "--check");
    let scratch = std::env::temp_dir().join(["protogen-", &std::process::id().to_string()].concat());
    fs::create_dir_all(&scratch)?;
    let produced = outputs(&root, &scratch);
    fs::remove_dir_all(&scratch)?;
    let produced = produced?;
    for (path, bytes) in &produced {
        if check {
            if fs::read(path).ok().as_ref() != Some(bytes) {
                return Err(format!("{} 不是最新的,重新运行 make protogen", path.display()).into());
            }
        } else {
            fs::create_dir_all(path.parent().ok_or("没有目录")?)?;
            fs::write(path, bytes)?;
        }
    }
    println!("{} 个文件", produced.len());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            std::process::ExitCode::FAILURE
        }
    }
}

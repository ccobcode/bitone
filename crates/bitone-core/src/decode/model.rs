//! 解码树.

use serde::Serialize;

/// 载荷里的一段字节.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub offset: u32,
    pub len: u32,
}

impl Span {
    pub fn new(offset: u32, len: u32) -> Self {
        Self { offset, len }
    }

    pub fn end(self) -> u64 {
        u64::from(self.offset) + u64::from(self.len)
    }
}

/// `signed`,`context` 节点和载荷字节的关系.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// 这段字节归这个节点.节点有子节点时,这里是官方编解码器的边界,子节点都是 `Inherited`.
    Own(Span),
    /// `struct`,`list`,字节都在子节点和 `framing` 里.
    Container,
    /// 官方编解码器内部的字段,官方解码器没有字节范围,继承最近的 `Own`,由 C2 证明.
    Inherited,
}

/// 节点来源.`signed`,`context` 带字节范围,其余没有,类型上就写不出来.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Signed(Extent),
    Context(Extent),
    Derived,
    Manifest,
    Descriptor,
    Hint,
}

impl Source {
    pub fn kind(self) -> SourceKind {
        match self {
            Self::Signed(_) => SourceKind::Signed,
            Self::Context(_) => SourceKind::Context,
            Self::Derived => SourceKind::Derived,
            Self::Manifest => SourceKind::Manifest,
            Self::Descriptor => SourceKind::Descriptor,
            Self::Hint => SourceKind::Hint,
        }
    }

    pub fn extent(self) -> Option<Extent> {
        match self {
            Self::Signed(extent) | Self::Context(extent) => Some(extent),
            Self::Derived | Self::Manifest | Self::Descriptor | Self::Hint => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Signed,
    Context,
    Derived,
    Manifest,
    Descriptor,
    Hint,
}

impl SourceKind {
    /// 能否作为策略依据.`context` 由策略校验后才能用.
    pub fn is_evidence(self) -> bool {
        match self {
            Self::Signed | Self::Context | Self::Derived | Self::Manifest => true,
            Self::Descriptor | Self::Hint => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Bool,
    Int,
    Uint,
    Amount,
    Address,
    Hash,
    Bytes,
    Text,
    Enum,
    Time,
    Script,
    Struct,
    List,
    Opaque,
}

/// 金额.`decimals`,`unit` 为空时显示原始整数,比如 TRC-10 的小数位在链上,离线看不到.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Amount {
    /// 大端字节.
    pub raw: Vec<u8>,
    pub decimals: Option<u8>,
    pub unit: Option<String>,
}

/// 类型化的值.种类由值决定,`struct`,`list` 没有值.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i128),
    /// 大端字节,EVM 的 256 位整数放得下.
    Uint(Vec<u8>),
    Amount(Amount),
    Address(String),
    Hash(Vec<u8>),
    Bytes(Vec<u8>),
    Text(String),
    /// 分支名,来自 schema,Solana 的分支名在 IDL 数据文件里.
    Enum(String),
    /// Unix 毫秒,TRON 的时间戳精确到毫秒.
    Time(i64),
    Script(Vec<u8>),
    Struct,
    List,
    Opaque(Vec<u8>),
}

impl Value {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Bool(_) => Kind::Bool,
            Self::Int(_) => Kind::Int,
            Self::Uint(_) => Kind::Uint,
            Self::Amount(_) => Kind::Amount,
            Self::Address(_) => Kind::Address,
            Self::Hash(_) => Kind::Hash,
            Self::Bytes(_) => Kind::Bytes,
            Self::Text(_) => Kind::Text,
            Self::Enum(_) => Kind::Enum,
            Self::Time(_) => Kind::Time,
            Self::Script(_) => Kind::Script,
            Self::Struct => Kind::Struct,
            Self::List => Kind::List,
            Self::Opaque(_) => Kind::Opaque,
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self, Self::Struct | Self::List)
    }
}

/// 树的一个节点.名字是官方字段名,Solana 的名字来自 IDL 数据文件,所以不是 `&'static str`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub value: Value,
    pub source: Source,
    /// 这个节点的结构字节:标签,长度前缀,列表头.只进 C1,不进渲染列表.
    pub framing: Vec<Span>,
    /// 这个节点是一个动作时,规则表里的种类名.种类名来自 schema.
    pub action: Option<String>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: impl Into<String>, value: Value, source: Source) -> Self {
        Self {
            name: name.into(),
            value,
            source,
            framing: Vec::new(),
            action: None,
            children: Vec::new(),
        }
    }

    pub fn framed(mut self, framing: Vec<Span>) -> Self {
        self.framing = framing;
        self
    }

    pub fn action(mut self, kind: impl Into<String>) -> Self {
        self.action = Some(kind.into());
        self
    }

    pub fn children(mut self, children: Vec<Node>) -> Self {
        self.children = children;
        self
    }
}

//! `tools/protogen` 由 `schemas/` 生成的 TRON 类型,不手改.

#![allow(clippy::all)]

pub mod google {
    pub mod protobuf {
        include!("google.protobuf.rs");
    }
}

pub mod protocol {
    include!("protocol.rs");
}

pub mod recode {
    include!("recode.rs");
}

//! 二维码协议,见 docs/03-system.md 第 6 节.
//!
//! 传输层是 BC-UR 喷泉码,载荷是 BitOne 自定义的 CBOR 结构.

use crate::path::{DerivationPath, all_account_specs, classify, validate_for_network};
use crate::{AccountKind, Chain, Error, Network};
use minicbor::data::Type;
use minicbor::{Decode, Decoder, Encode};
use std::collections::BTreeSet;

/// 永远是 1,见 docs/10-tx-decoding.md 第 1.2 节.
pub const VERSION: u8 = 1;

pub const UR_ACCOUNTS: &str = "bitone-accounts";
pub const UR_REQUEST: &str = "bitone-request";
pub const UR_RESPONSE: &str = "bitone-response";
pub const UR_ADDRESS: &str = "bitone-address";
pub const UR_MANIFEST: &str = "bitone-manifest";

const UR_TYPES: [&str; 5] = [UR_ACCOUNTS, UR_REQUEST, UR_RESPONSE, UR_ADDRESS, UR_MANIFEST];

pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;
pub const MAX_FRAMES: usize = 2000;
pub const MAX_PART_CHARS: usize = 3000;
pub const MAX_CBOR_DEPTH: usize = 8;

/// 单帧的分片大小.真机测试后再调.
pub const DEFAULT_FRAGMENT_BYTES: usize = 180;

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct AccountsExport {
    #[n(0)]
    pub version: u8,
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    pub fingerprint: [u8; 4],
    #[n(3)]
    pub accounts: Vec<ExportedAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct ExportedAccount {
    #[n(0)]
    pub chain: Chain,
    #[n(1)]
    pub kind: AccountKind,
    #[n(2)]
    pub path: String,
    /// Bitcoin 是序列化的扩展公钥,其余是账户公钥.
    #[n(3)]
    #[cbor(with = "minicbor::bytes")]
    pub key: Vec<u8>,
}

impl AccountsExport {
    pub fn encode(&self) -> Vec<u8> {
        minicbor::to_vec(self).expect("写入内存不会失败")
    }

    /// 只有内容全部通过校验才返回.路径不在白名单或与主网,测试网声明冲突的数据不会进入浏览器.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        check_limits(bytes)?;
        check_fields(bytes)?;
        let export: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        check_version(export.version)?;
        export.validate()?;
        Ok(export)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.accounts.is_empty() {
            return Err(Error::Decode);
        }
        let specs = all_account_specs(true);
        for (i, account) in self.accounts.iter().enumerate() {
            let key_len = match account.chain {
                Chain::Bitcoin => 78,
                Chain::Evm | Chain::Tron => 33,
                Chain::Solana => 32,
            };
            if account.key.len() != key_len {
                return Err(Error::InvalidKey);
            }
            // Bitcoin 账户不设白名单,路径和类型不能自相矛盾.其他链的路径在各自的迁移步骤里放开.
            let listed = match account.chain {
                Chain::Bitcoin => bitcoin_account_is_consistent(account),
                Chain::Evm | Chain::Tron | Chain::Solana => specs
                    .iter()
                    .any(|s| s.chain == account.chain && s.kind == account.kind && s.path == account.path),
            };
            let repeated = self.accounts[..i].iter().any(|a| a.path == account.path);
            if !listed || repeated {
                return Err(Error::PathNotAllowed);
            }
        }
        Ok(())
    }
}

/// 路径认得出来时,地址类型要和路径一致.另一条链的路径不是 Bitcoin 账户.账户路径比地址路径少两层.
fn bitcoin_account_is_consistent(account: &ExportedAccount) -> bool {
    let Ok(path) = DerivationPath::parse(&account.path) else {
        return false;
    };
    let address = DerivationPath([path.0.as_slice(), &[0, 0]].concat());
    match classify(&path).or_else(|_| classify(&address)) {
        Ok(info) => info.chain == Chain::Bitcoin && info.kind == account.kind,
        Err(_) => account.kind != AccountKind::Single,
    }
}

/// 让冷钱包核对一个收款地址.见 docs/04-security.md 第 4 节.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct AddressCheck {
    #[n(0)]
    pub version: u8,
    #[n(1)]
    #[cbor(with = "minicbor::bytes")]
    pub fingerprint: [u8; 4],
    #[n(2)]
    pub network: String,
    #[n(3)]
    pub path: String,
    #[n(4)]
    pub address: String,
}

impl AddressCheck {
    pub fn new(fingerprint: [u8; 4], network: Network, path: &str, address: &str) -> Result<Self, Error> {
        validate_for_network(&DerivationPath::parse(path)?, network)?;
        Ok(Self {
            version: VERSION,
            fingerprint,
            network: network.id().to_string(),
            path: path.to_string(),
            address: address.to_string(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        minicbor::to_vec(self).expect("写入内存不会失败")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        check_limits(bytes)?;
        check_fields(bytes)?;
        let check: Self = minicbor::decode(bytes).map_err(|_| Error::Decode)?;
        check_version(check.version)?;
        let network = Network::from_id(&check.network).ok_or(Error::Decode)?;
        validate_for_network(&DerivationPath::parse(&check.path)?, network)?;
        Ok(check)
    }

    /// 单帧二维码.内容是不区分大小写的 bytewords,所以 Base58 地址不会被大写破坏.
    pub fn frame(&self) -> Result<String, Error> {
        FrameEncoder::new(&self.encode(), UR_ADDRESS, MAX_PART_CHARS)?.next_part()
    }
}

/// 更高的版本不拒绝:已知字段齐全就能读,冷钱包在核对页告警.
pub(crate) fn check_version(version: u8) -> Result<(), Error> {
    if version < VERSION {
        Err(Error::UnsupportedVersion)
    } else {
        Ok(())
    }
}

/// 解析前的检查:大小和嵌套深度.解析器永远不会看到超限的数据.
pub fn check_limits(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::TooLarge);
    }
    let mut decoder = Decoder::new(bytes);
    // 每一层还剩多少项.不定长容器记 `None`,遇到 break 时结束.
    let mut stack: Vec<Option<u64>> = Vec::new();
    while decoder.position() < bytes.len() {
        let opened = match decoder.datatype().map_err(|_| Error::Decode)? {
            Type::Array | Type::ArrayIndef => Some(decoder.array().map_err(|_| Error::Decode)?),
            Type::Map | Type::MapIndef => {
                let len = decoder.map().map_err(|_| Error::Decode)?;
                Some(len.map(|n| n.saturating_mul(2)))
            }
            Type::Break => {
                decoder.set_position(decoder.position() + 1);
                if stack.pop() != Some(None) {
                    return Err(Error::Decode);
                }
                close_finished(&mut stack);
                continue;
            }
            Type::Tag => {
                decoder.tag().map_err(|_| Error::Decode)?;
                continue;
            }
            _ => {
                decoder.skip().map_err(|_| Error::Decode)?;
                None
            }
        };
        if let Some(Some(left)) = stack.last_mut() {
            *left -= 1;
        }
        match opened {
            Some(len) if len != Some(0) => {
                stack.push(len);
                if stack.len() > MAX_CBOR_DEPTH {
                    return Err(Error::TooDeep);
                }
            }
            _ => close_finished(&mut stack),
        }
    }
    if stack.is_empty() { Ok(()) } else { Err(Error::Decode) }
}

/// 解析前走一遍顶层 map.派生的解码器遇到重复键取最后一个,所以重复键和尾部字节都拒绝.
/// 未知键放行,派生的解码器会跳过它们,旧手机才读得懂新网页端的请求.
/// 不定长 map 没有合法来源,直接拒绝.要在 `check_limits` 之后调用.
pub fn check_fields(bytes: &[u8]) -> Result<(), Error> {
    let mut decoder = Decoder::new(bytes);
    let len = decoder.map().map_err(|_| Error::Decode)?.ok_or(Error::Decode)?;
    let mut seen = BTreeSet::new();
    for _ in 0..len {
        if !seen.insert(decoder.u32().map_err(|_| Error::Decode)?) {
            return Err(Error::Decode);
        }
        decoder.skip().map_err(|_| Error::Decode)?;
    }
    if decoder.position() == bytes.len() {
        Ok(())
    } else {
        Err(Error::Decode)
    }
}

/// 子容器读完后才关闭父容器,否则最后一项嵌套的数据不会累加深度.
fn close_finished(stack: &mut Vec<Option<u64>>) {
    while stack.last() == Some(&Some(0)) {
        stack.pop();
    }
}

/// 分帧编码.一条消息按固定大小分帧,循环取帧即可.
pub struct FrameEncoder {
    inner: ur::Encoder<'static>,
}

impl FrameEncoder {
    pub fn new(message: &[u8], ur_type: &str, fragment_bytes: usize) -> Result<Self, Error> {
        let ur_type = UR_TYPES
            .into_iter()
            .find(|t| *t == ur_type)
            .ok_or(Error::UnknownUrType)?;
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::TooLarge);
        }
        let inner = ur::Encoder::new(message, fragment_bytes, ur_type).map_err(|_| Error::InvalidFrame)?;
        Ok(Self { inner })
    }

    pub fn next_part(&mut self) -> Result<String, Error> {
        self.inner.next_part().map_err(|_| Error::InvalidFrame)
    }

    pub fn fragment_count(&self) -> usize {
        self.inner.fragment_count()
    }
}

/// 接收端.帧的顺序不重要,收够就能解码.
#[derive(Default)]
pub struct FrameDecoder {
    inner: ur::Decoder,
    ur_type: Option<String>,
}

impl FrameDecoder {
    pub fn receive(&mut self, part: &str) -> Result<(), Error> {
        if part.len() > MAX_PART_CHARS {
            return Err(Error::TooLarge);
        }
        let part = part.to_ascii_lowercase();
        let ur_type = header(&part)?;
        if let Some(seen) = &self.ur_type
            && seen != ur_type
        {
            return Err(Error::InvalidFrame);
        }
        self.ur_type = Some(ur_type.to_string());
        self.inner.receive(&part).map_err(|_| Error::InvalidFrame)
    }

    pub fn ur_type(&self) -> Option<&str> {
        self.ur_type.as_deref()
    }

    pub fn complete(&self) -> bool {
        self.inner.complete()
    }

    /// 这条消息一共多少帧,收到第一帧之前是 0.
    pub fn fragment_count(&self) -> usize {
        self.inner.fragment_count()
    }

    /// 已解出的分片比例,0 到 1.
    pub fn progress(&self) -> f32 {
        match self.inner.fragment_count() {
            0 => 0.0,
            total => self.inner.resolved_fragment_count().unwrap_or(0) as f32 / total as f32,
        }
    }

    pub fn message(&self) -> Result<Vec<u8>, Error> {
        let message = self
            .inner
            .message()
            .map_err(|_| Error::InvalidFrame)?
            .ok_or(Error::InvalidFrame)?;
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(message)
    }
}

/// 校验 `ur:<类型>/<序号>-<总数>/<数据>` 或 `ur:<类型>/<数据>` 的头部,返回类型.
fn header(part: &str) -> Result<&str, Error> {
    let body = part.strip_prefix("ur:").ok_or(Error::InvalidFrame)?;
    let (ur_type, rest) = body.split_once('/').ok_or(Error::InvalidFrame)?;
    if !UR_TYPES.contains(&ur_type) {
        return Err(Error::UnknownUrType);
    }
    if let Some((sequence, _)) = rest.split_once('/') {
        let (_, total) = sequence.split_once('-').ok_or(Error::InvalidFrame)?;
        let total: usize = total.parse().map_err(|_| Error::InvalidFrame)?;
        if total == 0 || total > MAX_FRAMES {
            return Err(Error::TooManyFrames);
        }
    }
    Ok(ur_type)
}

/// 在顶层 map 末尾加一项.
#[cfg(test)]
pub(crate) fn with_field(bytes: &[u8], key: u8, value: u8) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[0] += 1;
    out.extend([key, value]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::DisplayHex;

    fn account(chain: Chain, kind: AccountKind, path: &str, key_len: usize) -> ExportedAccount {
        ExportedAccount {
            chain,
            kind,
            path: path.into(),
            key: vec![2; key_len],
        }
    }

    fn sample() -> AccountsExport {
        AccountsExport {
            version: VERSION,
            fingerprint: [0x73, 0xC5, 0xDA, 0x0A],
            accounts: vec![account(Chain::Evm, AccountKind::Single, "m/44'/60'/0'/0/0", 33)],
        }
    }

    fn with_accounts(accounts: Vec<ExportedAccount>) -> Result<AccountsExport, Error> {
        AccountsExport::decode(&AccountsExport { accounts, ..sample() }.encode())
    }

    #[test]
    fn accounts_round_trip() {
        let export = sample();
        assert_eq!(AccountsExport::decode(&export.encode()).unwrap(), export);
    }

    #[test]
    fn a_higher_version_still_decodes_and_zero_does_not() {
        let mut export = sample();
        export.version = 9;
        assert_eq!(AccountsExport::decode(&export.encode()), Ok(export.clone()));
        export.version = 0;
        assert_eq!(AccountsExport::decode(&export.encode()), Err(Error::UnsupportedVersion));
    }

    #[test]
    fn a_higher_address_check_still_decodes() {
        let check = AddressCheck {
            version: 9,
            ..AddressCheck::new([1, 2, 3, 4], Network::Tron, "m/44'/195'/0'/0/0", "T").unwrap()
        };
        assert_eq!(AddressCheck::decode(&check.encode()), Ok(check));
    }

    #[test]
    fn bitcoin_accounts_import_from_any_path_but_cannot_contradict_it() {
        for path in ["m/84'/0'/7'", "m/84'/0'/7'/9", "m/9'/1'/2'"] {
            let any = account(Chain::Bitcoin, AccountKind::SegWit, path, 78);
            assert!(with_accounts(vec![any]).is_ok(), "{path}");
        }
        let wrong_kind = account(Chain::Bitcoin, AccountKind::Taproot, "m/84'/0'/0'", 78);
        assert_eq!(with_accounts(vec![wrong_kind]), Err(Error::PathNotAllowed));
        let other_chain = account(Chain::Bitcoin, AccountKind::SegWit, "m/44'/60'/0'/0/0", 78);
        assert_eq!(with_accounts(vec![other_chain]), Err(Error::PathNotAllowed));
    }

    #[test]
    fn other_chains_keep_the_export_list() {
        let stray = account(Chain::Evm, AccountKind::Single, "m/44'/60'/1'/0/0", 33);
        assert_eq!(with_accounts(vec![stray]), Err(Error::PathNotAllowed));
    }

    #[test]
    fn rejects_empty_repeated_and_malformed_accounts() {
        assert_eq!(with_accounts(vec![]), Err(Error::Decode));
        let evm = account(Chain::Evm, AccountKind::Single, "m/44'/60'/0'/0/0", 33);
        assert_eq!(with_accounts(vec![evm.clone(), evm]), Err(Error::PathNotAllowed));
        let short = account(Chain::Evm, AccountKind::Single, "m/44'/60'/0'/0/0", 32);
        assert_eq!(with_accounts(vec![short]), Err(Error::InvalidKey));
    }

    #[test]
    fn address_check_bytes_are_stable() {
        let check = AddressCheck::new(
            [0x73, 0xC5, 0xDA, 0x0A],
            Network::Tron,
            "m/44'/195'/0'/0/0",
            "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH",
        )
        .unwrap();
        assert_eq!(
            check.encode().to_lower_hex_string(),
            "a50001014473c5da0a026474726f6e03716d2f3434272f313935272f30272f302f300478225455455a53644b736f4448514d655a77696874646f42694e34367a78684757596448"
        );
    }

    #[test]
    fn address_check_survives_a_qr_code_round_trip() {
        let check = AddressCheck::new(
            [1, 2, 3, 4],
            Network::Solana,
            "m/44'/501'/0'/0'",
            "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk",
        )
        .unwrap();
        let mut decoder = FrameDecoder::default();
        decoder.receive(&check.frame().unwrap().to_ascii_uppercase()).unwrap();
        assert!(decoder.complete());
        assert_eq!(decoder.ur_type(), Some(UR_ADDRESS));
        assert_eq!(AddressCheck::decode(&decoder.message().unwrap()).unwrap(), check);
    }

    #[test]
    fn address_check_refuses_a_path_from_another_network() {
        assert_eq!(
            AddressCheck::new([0; 4], Network::TronNile, "m/44'/195'/0'/0/0", "T").unwrap_err(),
            Error::PathNetworkMismatch
        );
        let forged = AddressCheck {
            version: VERSION,
            fingerprint: [0; 4],
            network: "solana-devnet".into(),
            path: "m/44'/501'/0'/0'".into(),
            address: "x".into(),
        };
        assert_eq!(AddressCheck::decode(&forged.encode()), Err(Error::PathNetworkMismatch));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = sample().encode();
        bytes.push(0);
        assert_eq!(AccountsExport::decode(&bytes), Err(Error::Decode));
    }

    #[test]
    fn accounts_tolerate_unknown_fields_and_reject_duplicates() {
        let bytes = sample().encode();
        for key in [2, 4] {
            assert_eq!(
                AccountsExport::decode(&with_field(&bytes, key, 0)),
                Ok(sample()),
                "{key}"
            );
        }
        assert_eq!(
            AccountsExport::decode(&with_field(&bytes, 0, VERSION)),
            Err(Error::Decode)
        );
    }

    #[test]
    fn address_check_tolerates_unknown_fields_and_rejects_duplicates() {
        let check = AddressCheck::new([1, 2, 3, 4], Network::Solana, "m/44'/501'/0'/0'", "x").unwrap();
        let bytes = check.encode();
        assert_eq!(AddressCheck::decode(&with_field(&bytes, 5, 0)), Ok(check));
        assert_eq!(
            AddressCheck::decode(&with_field(&bytes, 0, VERSION)),
            Err(Error::Decode)
        );
    }

    #[test]
    fn an_unknown_field_can_hold_a_nested_value() {
        let mut bytes = sample().encode();
        bytes[0] += 1;
        bytes.extend([0x09, 0xa1, 0x01, 0x82, 0x01, 0x02]);
        assert_eq!(AccountsExport::decode(&bytes), Ok(sample()));
    }

    #[test]
    fn repeated_unknown_keys_are_rejected() {
        let bytes = with_field(&with_field(&sample().encode(), 9, 0), 9, 1);
        assert_eq!(AccountsExport::decode(&bytes), Err(Error::Decode));
    }

    #[test]
    fn an_unknown_field_cannot_carry_trailing_bytes() {
        let mut bytes = with_field(&sample().encode(), 9, 0);
        bytes.push(0);
        assert_eq!(AccountsExport::decode(&bytes), Err(Error::Decode));
    }

    #[test]
    fn long_key_encodings_and_indefinite_maps_cannot_hide_a_field() {
        let mut long_key = sample().encode();
        long_key[0] += 1;
        long_key.extend([0x18, 0x00, VERSION]);
        assert_eq!(AccountsExport::decode(&long_key), Err(Error::Decode));
        let mut indefinite = sample().encode();
        indefinite[0] = 0xbf;
        indefinite.push(0xff);
        assert_eq!(AccountsExport::decode(&indefinite), Err(Error::Decode));
    }

    #[test]
    fn rejects_garbage() {
        assert!(AccountsExport::decode(&[0xff, 0x00, 0x13]).is_err());
        assert!(AccountsExport::decode(&[]).is_err());
    }

    #[test]
    fn depth_limit_stops_nested_arrays() {
        let mut deep = vec![0x81; 64];
        deep.push(0x01);
        assert_eq!(check_limits(&deep), Err(Error::TooDeep));
        let mut shallow = vec![0x81; 3];
        shallow.push(0x01);
        assert_eq!(check_limits(&shallow), Ok(()));
    }

    #[test]
    fn depth_limit_covers_indefinite_containers() {
        let mut deep = vec![0x9f; 64];
        deep.push(0x01);
        deep.extend(vec![0xff; 64]);
        assert_eq!(check_limits(&deep), Err(Error::TooDeep));
    }

    #[test]
    fn truncated_cbor_is_rejected() {
        assert_eq!(check_limits(&[0x82, 0x01]), Err(Error::Decode));
    }

    #[test]
    fn siblings_do_not_count_as_depth() {
        let mut flat = vec![0x90];
        for _ in 0..16 {
            flat.extend([0x81, 0x01]);
        }
        assert_eq!(check_limits(&flat), Ok(()));
    }

    #[test]
    fn oversized_message_is_rejected_before_parsing() {
        let big = vec![0u8; MAX_MESSAGE_BYTES + 1];
        assert_eq!(check_limits(&big), Err(Error::TooLarge));
        assert!(matches!(
            FrameEncoder::new(&big, UR_ACCOUNTS, 180),
            Err(Error::TooLarge)
        ));
    }

    #[test]
    fn frames_survive_loss_and_reordering() {
        let message: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        let mut encoder = FrameEncoder::new(&message, UR_ACCOUNTS, 100).unwrap();
        assert!(encoder.fragment_count() > 10);
        let mut decoder = FrameDecoder::default();
        let mut index = 0;
        while !decoder.complete() {
            let part = encoder.next_part().unwrap();
            index += 1;
            if index % 3 != 0 {
                decoder.receive(&part.to_ascii_uppercase()).unwrap();
            }
            assert!(index < 2000);
        }
        assert_eq!(decoder.ur_type(), Some(UR_ACCOUNTS));
        assert_eq!(decoder.message().unwrap(), message);
        assert_eq!(decoder.progress(), 1.0);
    }

    #[test]
    fn single_frame_message_decodes() {
        let export = sample();
        let mut encoder = FrameEncoder::new(&export.encode(), UR_ACCOUNTS, 1000).unwrap();
        let mut decoder = FrameDecoder::default();
        decoder.receive(&encoder.next_part().unwrap()).unwrap();
        assert!(decoder.complete());
        assert_eq!(AccountsExport::decode(&decoder.message().unwrap()).unwrap(), export);
    }

    #[test]
    fn rejects_foreign_ur_types_and_frame_bombs() {
        let mut decoder = FrameDecoder::default();
        assert_eq!(decoder.receive("ur:crypto-psbt/1-2/abc"), Err(Error::UnknownUrType));
        assert_eq!(
            decoder.receive("ur:bitone-accounts/1-999999/abc"),
            Err(Error::TooManyFrames)
        );
        assert_eq!(decoder.receive("not a ur"), Err(Error::InvalidFrame));
        assert_eq!(decoder.receive(&"a".repeat(MAX_PART_CHARS + 1)), Err(Error::TooLarge));
    }

    #[test]
    fn rejects_frames_of_a_different_type_mid_stream() {
        let message = vec![7u8; 500];
        let mut first = FrameEncoder::new(&message, UR_ACCOUNTS, 100).unwrap();
        let mut second = FrameEncoder::new(&message, UR_REQUEST, 100).unwrap();
        let mut decoder = FrameDecoder::default();
        decoder.receive(&first.next_part().unwrap()).unwrap();
        assert_eq!(decoder.receive(&second.next_part().unwrap()), Err(Error::InvalidFrame));
    }

    #[test]
    fn frames_fit_in_a_qr_code() {
        let mut encoder = FrameEncoder::new(&vec![1u8; 2000], UR_ACCOUNTS, DEFAULT_FRAGMENT_BYTES).unwrap();
        let matrix = crate::qr::matrix(&encoder.next_part().unwrap()).unwrap();
        assert_eq!(matrix.modules.len(), (matrix.size * matrix.size) as usize);
        assert!(matrix.size >= 21);
    }
}

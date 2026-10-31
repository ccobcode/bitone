//! 冷钱包的 UniFFI 绑定.助记词和种子都留在 Rust 对象里,Kotlin 和 Swift 只持有句柄.

use bitone_core::hex::DisplayHex;
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::{Manifest, VENDOR_KEY};
use bitone_core::protocol::{AddressCheck, DEFAULT_FRAGMENT_BYTES, FrameDecoder, FrameEncoder, UR_ACCOUNTS};
use bitone_core::request::SignRequest;
use bitone_core::review::{Confirm, Forbid, Review, Risk};
use bitone_core::tx::{self, HighestChange, ReviewContext};
use bitone_core::{AccountKind, Mode, Network, qr};
use bitone_signer::{keys::Wallet, mnemonic, vault};
use std::sync::Mutex;
use zeroize::Zeroizing;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    #[error("助记词无效")]
    InvalidMnemonic,
    #[error("PIN 格式错误")]
    InvalidPin,
    #[error("PIN 错误或数据损坏")]
    WrongPin,
    // 字段不能叫 message,它和 Kotlin 的 Throwable.message 冲突.
    #[error("{reason}")]
    Other { reason: String },
}

impl From<bitone_signer::Error> for WalletError {
    fn from(error: bitone_signer::Error) -> Self {
        use bitone_signer::Error as E;
        match error {
            E::InvalidMnemonic | E::InvalidWordCount => Self::InvalidMnemonic,
            E::InvalidPin => Self::InvalidPin,
            E::WrongPin => Self::WrongPin,
            other => Self::Other {
                reason: other.to_string(),
            },
        }
    }
}

impl From<bitone_core::Error> for WalletError {
    fn from(error: bitone_core::Error) -> Self {
        Self::Other {
            reason: error.to_string(),
        }
    }
}

/// 一道验证题:第 `index` 个词(从 0 起)是 `options` 里的哪一个.
#[derive(uniffi::Record)]
pub struct QuizItem {
    pub index: u32,
    pub options: Vec<String>,
}

#[derive(uniffi::Record)]
pub struct QrMatrix {
    pub size: u32,
    pub modules: Vec<bool>,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskLevel {
    Transfer,
    Recognized,
    Unknown,
    Approval,
    PermissionChange,
    Forbidden,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmMode {
    Tap,
    Hold,
    Refuse,
}

#[derive(uniffi::Record)]
pub struct ReviewRow {
    pub label: String,
    pub value: String,
    pub mono: bool,
}

#[derive(uniffi::Record)]
pub struct ReviewAmount {
    pub text: String,
    pub symbol: Option<String>,
    pub asset: Option<String>,
    /// 内置清单里的图标名,不在清单时为空.
    pub icon: Option<String>,
    pub unlimited: bool,
}

#[derive(uniffi::Record)]
pub struct ReviewAction {
    pub risk: RiskLevel,
    pub title: String,
    pub rows: Vec<ReviewRow>,
}

#[derive(uniffi::Record)]
pub struct ReviewRawLine {
    pub text: String,
    pub note: Option<String>,
}

/// 核对页要显示的全部内容.分类都在核心库里完成,界面只负责画出来.
#[derive(uniffi::Record)]
pub struct ReviewResult {
    pub risk: RiskLevel,
    pub confirm: ConfirmMode,
    pub network: String,
    pub title: String,
    pub amount: Option<ReviewAmount>,
    pub counterparty_label: Option<String>,
    pub counterparty: Option<String>,
    pub alerts: Vec<String>,
    pub actions: Vec<ReviewAction>,
    pub rows: Vec<ReviewRow>,
    pub raw: Vec<ReviewRawLine>,
    /// 指纹对不上,要提示检查 BIP39 密码.
    pub fingerprint_mismatch: bool,
    /// 请求里的主密钥指纹.解析失败时为空.
    pub request_fingerprint: Option<String>,
}

fn risk_level(risk: Risk) -> RiskLevel {
    match risk {
        Risk::Transfer => RiskLevel::Transfer,
        Risk::Recognized => RiskLevel::Recognized,
        Risk::Unknown => RiskLevel::Unknown,
        Risk::Approval => RiskLevel::Approval,
        Risk::PermissionChange => RiskLevel::PermissionChange,
        Risk::Forbidden => RiskLevel::Forbidden,
    }
}

fn rows(list: Vec<bitone_core::review::Row>) -> Vec<ReviewRow> {
    list.into_iter()
        .map(|r| ReviewRow {
            label: r.label,
            value: r.value,
            mono: r.mono,
        })
        .collect()
}

fn review_result(review: Review, manifest: &Manifest, request_fingerprint: Option<String>) -> ReviewResult {
    let icon = |asset: &Option<String>| {
        let key = asset.as_deref()?;
        manifest.assets.iter().find(|a| a.key() == key).map(|a| a.icon.clone())
    };
    ReviewResult {
        risk: risk_level(review.risk),
        confirm: match review.confirm {
            Confirm::Tap => ConfirmMode::Tap,
            Confirm::Hold => ConfirmMode::Hold,
            Confirm::Refuse => ConfirmMode::Refuse,
        },
        network: review.network,
        title: review.title,
        amount: review.amount.map(|a| ReviewAmount {
            icon: icon(&a.asset),
            text: a.text,
            symbol: a.symbol,
            asset: a.asset,
            unlimited: a.unlimited,
        }),
        counterparty_label: review.counterparty_label,
        counterparty: review.counterparty,
        alerts: review.alerts,
        actions: review
            .actions
            .into_iter()
            .map(|a| ReviewAction {
                risk: risk_level(a.risk),
                title: a.title,
                rows: rows(a.rows),
            })
            .collect(),
        rows: rows(review.rows),
        raw: review
            .raw
            .into_iter()
            .map(|l| ReviewRawLine {
                text: l.text,
                note: l.note,
            })
            .collect(),
        fingerprint_mismatch: review.forbidden == Some(Forbid::FingerprintMismatch),
        request_fingerprint,
    }
}

fn kind_name(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::SegWit => "Native SegWit",
        AccountKind::Taproot => "Taproot",
        AccountKind::NestedSegWit => "Nested SegWit",
        AccountKind::Legacy => "Legacy",
        AccountKind::Single => "单地址",
    }
}

/// A9 的内容.
#[derive(uniffi::Record)]
pub struct AddressReview {
    pub belongs: bool,
    pub network: String,
    pub path: String,
    pub address: String,
    pub derived: String,
    /// 地址类型,比如 Native SegWit.
    pub kind: String,
    pub fingerprint_mismatch: bool,
    /// 请求里的主密钥指纹.
    pub request_fingerprint: String,
}

/// A3 的一行账户.
#[derive(uniffi::Record)]
pub struct AccountRow {
    pub kind: String,
    pub path: String,
    pub address: String,
}

/// 签名结果.`change_index` 要写回本机,下一笔交易核对时用.
#[derive(uniffi::Record)]
pub struct SignOutcome {
    pub frames: std::sync::Arc<FrameStream>,
    pub change_index: Option<u32>,
    /// `change_index` 属于测试币账户.
    pub change_is_testnet: bool,
}

/// 最高找零索引.测试币和主网分开记,平台层只负责保存.
#[derive(uniffi::Record, Clone, Copy)]
pub struct ChangeIndexes {
    pub mainnet: u32,
    pub testnet: u32,
}

impl From<ChangeIndexes> for HighestChange {
    fn from(value: ChangeIndexes) -> Self {
        Self {
            mainnet: value.mainnet,
            testnet: value.testnet,
        }
    }
}

fn load_manifest(package: Option<Vec<u8>>) -> Result<std::borrow::Cow<'static, Manifest>, WalletError> {
    Ok(Manifest::active(package.as_deref(), &VENDOR_KEY)?)
}

/// 建钱包过程中的助记词.设置 PIN 之后就不再需要这个对象.
#[derive(uniffi::Object)]
pub struct WalletDraft {
    entropy: Zeroizing<Vec<u8>>,
    imported: bool,
}

#[uniffi::export]
impl WalletDraft {
    #[uniffi::constructor]
    pub fn generate(word_count: u32) -> Result<Self, WalletError> {
        Ok(Self {
            entropy: mnemonic::generate_entropy(word_count as usize)?,
            imported: false,
        })
    }

    #[uniffi::constructor]
    pub fn import_words(words: Vec<String>) -> Result<Self, WalletError> {
        Ok(Self {
            entropy: mnemonic::entropy_from_words(&words)?,
            imported: true,
        })
    }

    /// 只在 A2 显示时调用.字符串经过 JVM 后无法清除,不要长期持有.
    pub fn words(&self) -> Result<Vec<String>, WalletError> {
        Ok(mnemonic::words(&self.entropy)?)
    }

    pub fn imported(&self) -> bool {
        self.imported
    }

    pub fn quiz(&self, rounds: u32) -> Result<Vec<QuizItem>, WalletError> {
        let words = mnemonic::words(&self.entropy)?;
        let mut positions: Vec<usize> = (0..words.len()).collect();
        let mut items = Vec::new();
        for _ in 0..rounds.min(words.len() as u32) {
            let picked = positions.swap_remove(mnemonic::random_below(positions.len())?);
            let mut options = vec![words[picked].clone()];
            while options.len() < 4 {
                let word = mnemonic::random_word()?.to_string();
                if !options.contains(&word) && !words.contains(&word) {
                    options.push(word);
                }
            }
            let answer = mnemonic::random_below(options.len())?;
            options.swap(0, answer);
            items.push(QuizItem {
                index: picked as u32,
                options,
            });
        }
        Ok(items)
    }

    pub fn check_word(&self, index: u32, word: String) -> Result<bool, WalletError> {
        Ok(mnemonic::words(&self.entropy)?.get(index as usize) == Some(&word))
    }

    /// `tag` 是硬件对这个 PIN 算出的 HMAC.
    pub fn seal(&self, pin: String, tag: Vec<u8>) -> Result<Vec<u8>, WalletError> {
        Ok(vault::seal(&self.entropy, &pin, &tag, vault::KdfParams::DEFAULT)?)
    }
}

/// 解锁后的钱包,随对象销毁清除种子.
#[derive(uniffi::Object)]
pub struct UnlockedWallet {
    wallet: Wallet,
    entropy: Zeroizing<Vec<u8>>,
}

#[uniffi::export]
impl UnlockedWallet {
    #[uniffi::constructor]
    pub fn open(sealed: Vec<u8>, pin: String, tag: Vec<u8>, passphrase: String) -> Result<Self, WalletError> {
        let entropy = vault::open(&sealed, &pin, &tag)?;
        Ok(Self {
            wallet: Wallet::new(&entropy, &passphrase)?,
            entropy,
        })
    }

    /// 解开旧格式,只用于迁移.`sealed` 是去掉硬件那层之后的内容.
    #[uniffi::constructor]
    pub fn open_legacy(sealed: Vec<u8>, pin: String, passphrase: String) -> Result<Self, WalletError> {
        let entropy = vault::open_legacy(&sealed, &pin)?;
        Ok(Self {
            wallet: Wallet::new(&entropy, &passphrase)?,
            entropy,
        })
    }

    pub fn fingerprint(&self) -> Result<String, WalletError> {
        let bytes = self.wallet.fingerprint()?;
        Ok(bytes.iter().map(|b| format!("{b:02X}")).collect())
    }

    pub fn words(&self) -> Result<Vec<String>, WalletError> {
        Ok(mnemonic::words(&self.entropy)?)
    }

    /// 修改 PIN 和迁移用.助记词留在 Rust 里,不经过 JVM.
    pub fn seal(&self, pin: String, tag: Vec<u8>) -> Result<Vec<u8>, WalletError> {
        Ok(vault::seal(&self.entropy, &pin, &tag, vault::KdfParams::DEFAULT)?)
    }

    /// 公钥写入本机,之后核对交易不用解锁.BIP39 密码钱包不要落盘,只放内存.
    pub fn public_keys(&self, imported: bool) -> Result<Vec<u8>, WalletError> {
        Ok(self.wallet.public_keys(imported)?)
    }

    /// 重新核对后签名.核对不通过时返回错误,不接触私钥.
    pub fn sign_request(
        &self,
        message: Vec<u8>,
        public_keys: Vec<u8>,
        highest_change: ChangeIndexes,
        manifest: Option<Vec<u8>>,
    ) -> Result<SignOutcome, WalletError> {
        let request = SignRequest::decode(&message)?;
        let keys = PublicKeys::decode(&public_keys)?;
        let manifest = load_manifest(manifest)?;
        let (response, review) = self.wallet.sign(&request, &keys, &manifest, highest_change.into())?;
        let encoder = response.frames(DEFAULT_FRAGMENT_BYTES)?;
        Ok(SignOutcome {
            frames: std::sync::Arc::new(FrameStream {
                encoder: Mutex::new(encoder),
            }),
            change_index: review.change_index,
            change_is_testnet: Network::from_id(&request.network).is_some_and(|n| n.mode() == Mode::Testnet),
        })
    }

    pub fn export_accounts(&self, imported: bool) -> Result<FrameStream, WalletError> {
        let message = self.wallet.export(imported)?.encode();
        let encoder = FrameEncoder::new(&message, UR_ACCOUNTS, DEFAULT_FRAGMENT_BYTES)?;
        Ok(FrameStream {
            encoder: Mutex::new(encoder),
        })
    }
}

/// 循环取帧,交给二维码显示.
#[derive(uniffi::Object)]
pub struct FrameStream {
    encoder: Mutex<FrameEncoder>,
}

#[uniffi::export]
impl FrameStream {
    pub fn next_part(&self) -> Result<String, WalletError> {
        Ok(self.encoder.lock().expect("锁未中毒").next_part()?)
    }

    pub fn fragment_count(&self) -> u32 {
        self.encoder.lock().expect("锁未中毒").fragment_count() as u32
    }
}

#[derive(uniffi::Object, Default)]
pub struct FrameReceiver {
    decoder: Mutex<FrameDecoder>,
}

#[uniffi::export]
impl FrameReceiver {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn receive(&self, part: String) -> Result<f32, WalletError> {
        let mut decoder = self.decoder.lock().expect("锁未中毒");
        decoder.receive(&part)?;
        Ok(decoder.progress())
    }

    pub fn complete(&self) -> bool {
        self.decoder.lock().expect("锁未中毒").complete()
    }

    pub fn ur_type(&self) -> Option<String> {
        self.decoder.lock().expect("锁未中毒").ur_type().map(String::from)
    }

    /// 收齐后取出消息,交给 `review_request` 或 `review_address`.
    pub fn message(&self) -> Result<Vec<u8>, WalletError> {
        Ok(self.decoder.lock().expect("锁未中毒").message()?)
    }

    /// 这条消息一共多少帧,收到第一帧之前是 0.
    pub fn fragment_count(&self) -> u32 {
        self.decoder.lock().expect("锁未中毒").fragment_count() as u32
    }
}

/// 解析并分类.这一步不需要 PIN,所有失败都变成"禁止签名".
#[uniffi::export]
pub fn review_request(
    message: Vec<u8>,
    public_keys: Vec<u8>,
    highest_change: ChangeIndexes,
    manifest: Option<Vec<u8>>,
) -> Result<ReviewResult, WalletError> {
    let keys = PublicKeys::decode(&public_keys)?;
    let manifest = load_manifest(manifest)?;
    let Ok(request) = SignRequest::decode(&message) else {
        return Ok(review_result(
            Review::forbidden("", Forbid::ParseFailed),
            &manifest,
            None,
        ));
    };
    let ctx = ReviewContext {
        keys: &keys,
        manifest: &manifest,
        highest_change: highest_change.into(),
    };
    let fingerprint = request.fingerprint.to_upper_hex_string();
    Ok(review_result(tx::review(&request, &ctx), &manifest, Some(fingerprint)))
}

/// A3 的账户和地址.不需要 PIN.
#[uniffi::export]
pub fn account_rows(public_keys: Vec<u8>) -> Result<Vec<AccountRow>, WalletError> {
    let keys = PublicKeys::decode(&public_keys)?;
    Ok(keys
        .addresses()?
        .into_iter()
        .map(|a| AccountRow {
            kind: kind_name(a.kind).to_string(),
            path: a.path,
            address: a.address,
        })
        .collect())
}

#[uniffi::export]
pub fn review_address(message: Vec<u8>, public_keys: Vec<u8>) -> Result<AddressReview, WalletError> {
    let keys = PublicKeys::decode(&public_keys)?;
    let check = AddressCheck::decode(&message)?;
    let refused = |fingerprint_mismatch| AddressReview {
        belongs: false,
        network: check.network.clone(),
        path: check.path.clone(),
        address: check.address.clone(),
        derived: String::new(),
        kind: String::new(),
        fingerprint_mismatch,
        request_fingerprint: check.fingerprint.to_upper_hex_string(),
    };
    match tx::check_address(&check, &keys) {
        Ok(result) => Ok(AddressReview {
            belongs: result.belongs,
            network: result.network,
            path: result.path,
            address: result.address,
            derived: result.derived,
            kind: kind_name(result.kind).to_string(),
            fingerprint_mismatch: false,
            request_fingerprint: check.fingerprint.to_upper_hex_string(),
        }),
        Err(Forbid::FingerprintMismatch) => Ok(refused(true)),
        Err(reason) => Err(WalletError::Other {
            reason: reason.to_string(),
        }),
    }
}

/// 校验清单包,返回版本号.校验通过后由平台层保存原始字节.
#[uniffi::export]
pub fn verify_manifest(package: Vec<u8>, current_version: u32) -> Result<u32, WalletError> {
    Ok(Manifest::from_package(&package, &VENDOR_KEY, current_version)?.version)
}

#[uniffi::export]
pub fn manifest_version(package: Option<Vec<u8>>) -> Result<u32, WalletError> {
    Ok(load_manifest(package)?.version)
}

#[uniffi::export]
pub fn qr_matrix(part: String) -> Result<QrMatrix, WalletError> {
    let matrix = qr::matrix(&part)?;
    Ok(QrMatrix {
        size: matrix.size,
        modules: matrix.modules,
    })
}

/// 普通文本的二维码,比如给付款方扫的收款地址.内容不转大写.
#[uniffi::export]
pub fn qr_text(text: String) -> Result<QrMatrix, WalletError> {
    let matrix = qr::text_matrix(&text)?;
    Ok(QrMatrix {
        size: matrix.size,
        modules: matrix.modules,
    })
}

/// 账户的收款地址,从本机保存的公钥算出,不经过网页端.`account_path` 是 `AccountRow.path`.
#[uniffi::export]
pub fn receive_address(public_keys: Vec<u8>, account_path: String, index: u32) -> Result<String, WalletError> {
    let keys = PublicKeys::decode(&public_keys)?;
    Ok(keys.receive_address(&account_path, index)?)
}

/// 本机保存的密文是不是当前格式.不是时要先走迁移.
#[uniffi::export]
pub fn is_bound_vault(sealed: Vec<u8>) -> bool {
    vault::is_bound(&sealed)
}

/// 设置 PIN 时拒绝太容易猜的.
#[uniffi::export]
pub fn is_weak_pin(pin: String) -> bool {
    vault::is_weak_pin(&pin)
}

/// 内置清单的一项.
#[derive(uniffi::Record)]
pub struct ManifestEntry {
    pub network: String,
    pub network_label: String,
    pub symbol: String,
    pub contract: String,
    pub icon: String,
}

/// 内置清单的全部代币,用于 A12 展示.原生币也在里面.
#[uniffi::export]
pub fn manifest_entries(package: Option<Vec<u8>>) -> Result<Vec<ManifestEntry>, WalletError> {
    let manifest = load_manifest(package)?;
    Ok(manifest
        .assets
        .iter()
        .map(|asset| ManifestEntry {
            network: asset.network.clone(),
            network_label: bitone_core::Network::from_id(&asset.network)
                .map_or_else(|| asset.network.clone(), |n| n.label().to_string()),
            symbol: asset.symbol.clone(),
            contract: asset.contract.clone(),
            icon: asset.icon.clone(),
        })
        .collect())
}

#[uniffi::export]
pub fn is_valid_word(word: String) -> bool {
    mnemonic::is_valid_word(&word)
}

#[uniffi::export]
pub fn word_suggestions(prefix: String, limit: u32) -> Vec<String> {
    mnemonic::suggestions(&prefix, limit as usize)
        .into_iter()
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "123456";
    const TAG: [u8; 32] = [5; 32];
    const NO_CHANGE: ChangeIndexes = ChangeIndexes { mainnet: 0, testnet: 0 };

    #[test]
    fn create_seal_unlock_export() {
        let draft = WalletDraft::generate(12).unwrap();
        let sealed = draft.seal(PIN.into(), TAG.to_vec()).unwrap();
        let wallet = UnlockedWallet::open(sealed, PIN.into(), TAG.to_vec(), String::new()).unwrap();
        assert_eq!(wallet.words().unwrap(), draft.words().unwrap());
        assert_eq!(wallet.fingerprint().unwrap().len(), 8);

        let stream = wallet.export_accounts(false).unwrap();
        let mut decoder = FrameDecoder::default();
        while !decoder.complete() {
            decoder.receive(&stream.next_part().unwrap()).unwrap();
        }
        let export = bitone_core::protocol::AccountsExport::decode(&decoder.message().unwrap()).unwrap();
        assert_eq!(export.accounts.len(), 9);
    }

    #[test]
    fn review_needs_no_pin_and_sign_rechecks() {
        use bitone_core::Network;
        use bitone_core::request::{KIND_TRANSACTION, SignRequest};
        use bitone_core::tx::evm::{TransferParams, build_transfer};

        let draft = WalletDraft::import_words(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                .split(' ')
                .map(String::from)
                .collect(),
        )
        .unwrap();
        let sealed = draft.seal(PIN.into(), TAG.to_vec()).unwrap();
        let wallet = UnlockedWallet::open(sealed, PIN.into(), TAG.to_vec(), String::new()).unwrap();
        let keys = wallet.public_keys(false).unwrap();
        let payload = build_transfer(&TransferParams {
            network: Network::Ethereum,
            to: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            amount: "1",
            contract: None,
            nonce: 0,
            gas_limit: 21000,
            max_fee: 1,
            max_priority: 1,
        })
        .unwrap()
        .encode();
        let mut request = SignRequest {
            version: 1,
            id: [1; 16],
            fingerprint: [0x73, 0xc5, 0xda, 0x0a],
            network: "ethereum".into(),
            kind: KIND_TRANSACTION,
            paths: vec!["m/44'/60'/0'/0/0".into()],
            payload,
            hint: None,
        };
        let review = review_request(request.encode(), keys.clone(), NO_CHANGE, None).unwrap();
        assert_eq!((review.risk, review.confirm), (RiskLevel::Transfer, ConfirmMode::Tap));
        let outcome = wallet
            .sign_request(request.encode(), keys.clone(), NO_CHANGE, None)
            .unwrap();
        assert!(outcome.frames.next_part().unwrap().starts_with("ur:bitone-response/"));

        request.fingerprint = [0; 4];
        let refused = review_request(request.encode(), keys.clone(), NO_CHANGE, None).unwrap();
        assert!(refused.fingerprint_mismatch);
        assert_eq!(refused.confirm, ConfirmMode::Refuse);
        assert!(wallet.sign_request(request.encode(), keys, NO_CHANGE, None).is_err());
    }

    #[test]
    fn garbage_is_refused_not_an_error() {
        let wallet_keys = {
            let sealed = WalletDraft::generate(12)
                .unwrap()
                .seal(PIN.into(), TAG.to_vec())
                .unwrap();
            UnlockedWallet::open(sealed, PIN.into(), TAG.to_vec(), String::new())
                .unwrap()
                .public_keys(false)
                .unwrap()
        };
        let review = review_request(vec![1, 2, 3], wallet_keys, NO_CHANGE, None).unwrap();
        assert_eq!(review.risk, RiskLevel::Forbidden);
    }

    #[test]
    fn wrong_pin_is_reported() {
        let sealed = WalletDraft::generate(12)
            .unwrap()
            .seal(PIN.into(), TAG.to_vec())
            .unwrap();
        assert!(matches!(
            UnlockedWallet::open(sealed, "000000".into(), TAG.to_vec(), String::new()),
            Err(WalletError::WrongPin)
        ));
    }

    #[test]
    fn quiz_contains_the_answer_and_no_other_seed_word() {
        let draft = WalletDraft::generate(12).unwrap();
        let words = draft.words().unwrap();
        let items = draft.quiz(3).unwrap();
        assert_eq!(items.len(), 3);
        for item in items {
            assert_eq!(item.options.len(), 4);
            let answers: Vec<_> = item.options.iter().filter(|o| words.contains(o)).collect();
            assert_eq!(answers, vec![&words[item.index as usize]]);
        }
    }

    #[test]
    fn imported_flag_and_checksum() {
        let words: Vec<String> =
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                .split(' ')
                .map(String::from)
                .collect();
        assert!(WalletDraft::import_words(words).unwrap().imported());
        assert!(matches!(
            WalletDraft::import_words(vec!["abandon".to_string(); 12]),
            Err(WalletError::InvalidMnemonic)
        ));
    }
}

//! 种子加密.
//!
//! 解密密钥由 PIN 和硬件的 HMAC 结果一起派生.每猜一次 PIN 都要手机硬件算一次,
//! 所以密文拿走之后没法离线试 PIN.密文里是熵和 BIP39 密码,解开就是用户真正的钱包.

use crate::Error;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::{Zeroize, Zeroizing};

/// 最早的格式的第一个字节.解密密钥只来自 PIN,平台层外面再包一层硬件加密,密文里只有熵.
const LEGACY_FORMAT: u8 = 1;
/// 只存熵的格式的开头.解密密钥绑定硬件,BIP39 密码不在里面.
const ENTROPY_MAGIC: [u8; 8] = *b"BITONE02";
/// 当前格式的开头.最早的格式的文件以随机 IV 开头,要靠这 8 个字节区分.
const MAGIC: [u8; 8] = *b"BITONE03";
const PARAMS_LEN: usize = 12;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
/// 硬件 HMAC-SHA256 的结果.
const TAG_LEN: usize = 32;

/// Argon2id 参数.写进密文头,日后调整不影响已有钱包.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 初始值,要在低端手机上实测后校准.
    pub const DEFAULT: Self = Self {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// 内存和耗时的下限,防止篡改密文头来削弱暴力破解的成本.
    const MINIMUM: Self = Self {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    const MAXIMUM_MEMORY_KIB: u32 = 1024 * 1024;

    fn is_acceptable(self) -> bool {
        (Self::MINIMUM.memory_kib..=Self::MAXIMUM_MEMORY_KIB).contains(&self.memory_kib)
            && (Self::MINIMUM.iterations..=16).contains(&self.iterations)
            && (Self::MINIMUM.parallelism..=4).contains(&self.parallelism)
    }
}

/// 当前格式密文里的秘密.没有启用 BIP39 密码时 `passphrase` 是空串.
pub struct Secret {
    pub entropy: Zeroizing<Vec<u8>>,
    pub passphrase: Zeroizing<String>,
}

fn check_pin(pin: &str) -> Result<(), Error> {
    if pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Error::InvalidPin)
    }
}

/// 太容易猜的 PIN:最多两种数字,或者逐位加一,减一.设置时拒绝,已有的钱包不受影响.
pub fn is_weak_pin(pin: &str) -> bool {
    let digits: Vec<i8> = pin.bytes().map(|b| b as i8 - b'0' as i8).collect();
    let mut seen = digits.clone();
    seen.sort_unstable();
    seen.dedup();
    let step = |delta: i8| digits.windows(2).all(|pair| pair[1] - pair[0] == delta);
    seen.len() <= 2 || step(1) || step(-1)
}

fn derive_key(password: &[u8], salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>, Error> {
    let argon_params = Params::new(params.memory_kib, params.iterations, params.parallelism, Some(32))
        .map_err(|_| Error::UnsupportedVault)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params)
        .hash_password_into(password, salt, key.as_mut())
        .map_err(|_| Error::UnsupportedVault)?;
    Ok(key)
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("密钥长度固定为 32 字节")
}

/// 头部不加密,但参与认证,改动头部会解密失败.
fn seal_with(prefix: &[u8], plain: &[u8], password: &[u8], params: KdfParams) -> Result<Vec<u8>, Error> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|_| Error::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;

    let mut sealed = prefix.to_vec();
    sealed.extend_from_slice(&params.memory_kib.to_le_bytes());
    sealed.extend_from_slice(&params.iterations.to_le_bytes());
    sealed.extend_from_slice(&params.parallelism.to_le_bytes());
    sealed.extend_from_slice(&salt);
    sealed.extend_from_slice(&nonce);

    let key = derive_key(password, &salt, params)?;
    let ciphertext = cipher(&key)
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plain,
                aad: &sealed,
            },
        )
        .map_err(|_| Error::UnsupportedVault)?;
    sealed.extend_from_slice(&ciphertext);
    Ok(sealed)
}

/// `prefix` 是头部里参数之前的长度.密码错误和数据损坏无法区分,都返回 `WrongPin`.
fn open_with(prefix: usize, sealed: &[u8], password: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let salt_at = prefix + PARAMS_LEN;
    let nonce_at = salt_at + SALT_LEN;
    let header_len = nonce_at + NONCE_LEN;
    if sealed.len() <= header_len {
        return Err(Error::UnsupportedVault);
    }
    let field = |at: usize| u32::from_le_bytes(sealed[at..at + 4].try_into().expect("4 字节"));
    let params = KdfParams {
        memory_kib: field(prefix),
        iterations: field(prefix + 4),
        parallelism: field(prefix + 8),
    };
    if !params.is_acceptable() {
        return Err(Error::UnsupportedVault);
    }
    let (header, ciphertext) = sealed.split_at(header_len);
    let nonce: [u8; NONCE_LEN] = header[nonce_at..].try_into().expect("12 字节");
    let key = derive_key(password, &header[salt_at..nonce_at], params)?;
    let mut plain = cipher(&key)
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map_err(|_| Error::WrongPin)?;
    let opened = Zeroizing::new(plain.clone());
    plain.zeroize();
    Ok(opened)
}

/// PIN 和硬件结果拼在一起作为 Argon2id 的密码.结果长度不对时拒绝,不能退化成只靠 PIN.
fn bound_password(pin: &str, tag: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    check_pin(pin)?;
    if tag.len() != TAG_LEN {
        return Err(Error::UnsupportedVault);
    }
    let mut password = Zeroizing::new(Vec::with_capacity(pin.len() + TAG_LEN));
    password.extend_from_slice(pin.as_bytes());
    password.extend_from_slice(tag);
    Ok(password)
}

/// 解密密钥绑定了硬件.不是时要走 [`open_legacy`].
pub fn is_bound(sealed: &[u8]) -> bool {
    is_current(sealed) || sealed.starts_with(&ENTROPY_MAGIC)
}

/// 密文是不是当前格式.不是时解开之后要换成当前格式,BIP39 密码这时才封进去.
pub fn is_current(sealed: &[u8]) -> bool {
    sealed.starts_with(&MAGIC)
}

/// 用 PIN 和硬件结果加密熵和 BIP39 密码.明文是 1 字节的熵长度,熵,密码的 UTF-8.
pub fn seal(entropy: &[u8], passphrase: &str, pin: &str, tag: &[u8], params: KdfParams) -> Result<Vec<u8>, Error> {
    let password = bound_password(pin, tag)?;
    let length = u8::try_from(entropy.len()).map_err(|_| Error::InvalidMnemonic)?;
    // 容量一次给够,扩容会在堆上留下没清除的副本.
    let mut plain = Zeroizing::new(Vec::with_capacity(1 + entropy.len() + passphrase.len()));
    plain.push(length);
    plain.extend_from_slice(entropy);
    plain.extend_from_slice(passphrase.as_bytes());
    seal_with(&MAGIC, &plain, &password, params)
}

/// 解开当前格式.
pub fn open(sealed: &[u8], pin: &str, tag: &[u8]) -> Result<Secret, Error> {
    let password = bound_password(pin, tag)?;
    if !is_current(sealed) {
        return Err(Error::UnsupportedVault);
    }
    let plain = open_with(MAGIC.len(), sealed, &password)?;
    let (&length, rest) = plain.split_first().ok_or(Error::UnsupportedVault)?;
    let (entropy, passphrase) = rest
        .split_at_checked(usize::from(length))
        .ok_or(Error::UnsupportedVault)?;
    let passphrase = std::str::from_utf8(passphrase).map_err(|_| Error::UnsupportedVault)?;
    Ok(Secret {
        entropy: Zeroizing::new(entropy.to_vec()),
        passphrase: Zeroizing::new(passphrase.to_owned()),
    })
}

/// 解开只存熵的格式,只用于迁移.
pub fn open_entropy_only(sealed: &[u8], pin: &str, tag: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let password = bound_password(pin, tag)?;
    if !sealed.starts_with(&ENTROPY_MAGIC) {
        return Err(Error::UnsupportedVault);
    }
    open_with(ENTROPY_MAGIC.len(), sealed, &password)
}

/// 解开最早的格式,只用于迁移.这里拿到的是平台层已经去掉硬件那层的内容.
pub fn open_legacy(sealed: &[u8], pin: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    check_pin(pin)?;
    if sealed.first() != Some(&LEGACY_FORMAT) {
        return Err(Error::UnsupportedVault);
    }
    open_with(1, sealed, pin.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    const TAG: [u8; TAG_LEN] = [5; TAG_LEN];

    /// 只存熵的格式,写法和它还是当前格式时一样.
    fn entropy_only(entropy: &[u8]) -> Vec<u8> {
        seal_with(&ENTROPY_MAGIC, entropy, &bound_password("123456", &TAG).unwrap(), FAST).unwrap()
    }

    #[test]
    fn easy_pins_are_weak() {
        for pin in [
            "000000", "111111", "123456", "654321", "012345", "987654", "121212", "111222",
        ] {
            assert!(is_weak_pin(pin), "{pin}");
        }
        for pin in ["739264", "120583", "112233"] {
            assert!(!is_weak_pin(pin), "{pin}");
        }
    }

    #[test]
    fn seal_and_open_with_and_without_a_passphrase() {
        for passphrase in ["", "TREZOR", "P\u{e4}ss w\u{f6}rd 1!"] {
            for length in [16, 20, 24, 28, 32] {
                let entropy = vec![7u8; length];
                let sealed = seal(&entropy, passphrase, "123456", &TAG, FAST).unwrap();
                assert!(is_bound(&sealed) && is_current(&sealed));
                let secret = open(&sealed, "123456", &TAG).unwrap();
                assert_eq!(*secret.entropy, entropy);
                assert_eq!(secret.passphrase.as_str(), passphrase);
            }
        }
    }

    #[test]
    fn wrong_pin_fails() {
        let sealed = seal(&[7u8; 32], "TREZOR", "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "654321", &TAG).err(), Some(Error::WrongPin));
    }

    /// 只有 PIN 解不开.这是硬件绑定的全部意义:离线试 PIN 拿不到硬件结果.
    #[test]
    fn the_right_pin_needs_the_hardware_result() {
        let sealed = seal(&[7u8; 32], "", "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "123456", &[6; TAG_LEN]).err(), Some(Error::WrongPin));
        assert_eq!(open_legacy(&sealed, "123456").unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn a_short_hardware_result_is_refused() {
        for tag in [&[][..], &[5; 31][..], &[5; 33][..]] {
            assert_eq!(
                seal(&[1u8; 16], "", "123456", tag, FAST).unwrap_err(),
                Error::UnsupportedVault
            );
        }
        let sealed = seal(&[7u8; 32], "", "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "123456", &[]).err(), Some(Error::UnsupportedVault));
    }

    /// 迁移:用同一个 PIN 和硬件结果重新封成当前格式,这次输入的 BIP39 密码这时才封进去.
    #[test]
    fn entropy_only_vaults_migrate() {
        let old = entropy_only(&[7u8; 32]);
        assert!(is_bound(&old) && !is_current(&old));
        assert_eq!(open(&old, "123456", &TAG).err(), Some(Error::UnsupportedVault));
        assert_eq!(open_entropy_only(&old, "654321", &TAG).unwrap_err(), Error::WrongPin);
        let entropy = open_entropy_only(&old, "123456", &TAG).unwrap();
        assert_eq!(*entropy, vec![7u8; 32]);

        let migrated = seal(&entropy, "TREZOR", "123456", &TAG, FAST).unwrap();
        let secret = open(&migrated, "123456", &TAG).unwrap();
        assert_eq!(*secret.entropy, vec![7u8; 32]);
        assert_eq!(secret.passphrase.as_str(), "TREZOR");
        assert_eq!(
            open_entropy_only(&migrated, "123456", &TAG).unwrap_err(),
            Error::UnsupportedVault
        );
    }

    #[test]
    fn legacy_vaults_migrate() {
        let sealed = seal_with(&[LEGACY_FORMAT], &[7u8; 32], b"123456", FAST).unwrap();
        assert!(!is_bound(&sealed) && !is_current(&sealed));
        assert_eq!(open_legacy(&sealed, "654321").unwrap_err(), Error::WrongPin);
        assert_eq!(open(&sealed, "123456", &TAG).err(), Some(Error::UnsupportedVault));
        assert_eq!(
            open_entropy_only(&sealed, "123456", &TAG).unwrap_err(),
            Error::UnsupportedVault
        );
        let entropy = open_legacy(&sealed, "123456").unwrap();
        assert_eq!(*entropy, vec![7u8; 32]);

        let migrated = seal(&entropy, "", "123456", &TAG, FAST).unwrap();
        let secret = open(&migrated, "123456", &TAG).unwrap();
        assert_eq!(*secret.entropy, vec![7u8; 32]);
        assert!(secret.passphrase.is_empty());
    }

    #[test]
    fn ciphertext_does_not_contain_the_secret() {
        let sealed = seal(&[0xABu8; 32], "TREZOR", "123456", &TAG, FAST).unwrap();
        assert!(!sealed.windows(8).any(|w| w == [0xAB; 8]));
        assert!(!sealed.windows(6).any(|w| w == b"TREZOR"));
    }

    #[test]
    fn same_input_produces_different_ciphertext() {
        assert_ne!(
            seal(&[1u8; 16], "TREZOR", "123456", &TAG, FAST).unwrap(),
            seal(&[1u8; 16], "TREZOR", "123456", &TAG, FAST).unwrap()
        );
    }

    #[test]
    fn tampering_is_detected() {
        let header = MAGIC.len() + PARAMS_LEN + SALT_LEN + NONCE_LEN;
        let sealed = seal(&[7u8; 32], "TREZOR", "123456", &TAG, FAST).unwrap();
        for at in [MAGIC.len() + PARAMS_LEN, header - 1, header, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[at] ^= 1;
            assert_eq!(open(&bad, "123456", &TAG).err(), Some(Error::WrongPin), "offset {at}");
        }
    }

    #[test]
    fn weakened_kdf_header_is_rejected() {
        let mut sealed = seal(&[7u8; 32], "", "123456", &TAG, FAST).unwrap();
        sealed[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&1024u32.to_le_bytes());
        assert_eq!(open(&sealed, "123456", &TAG).err(), Some(Error::UnsupportedVault));
    }

    /// 明文的长度和编码不对时拒绝.这样的明文只可能来自程序错误,认证通过也不能当成钱包.
    #[test]
    fn a_malformed_plaintext_is_refused() {
        let password = bound_password("123456", &TAG).unwrap();
        for plain in [&[][..], &[17, 7, 7][..], &[1, 7, 0xff][..]] {
            let sealed = seal_with(&MAGIC, plain, &password, FAST).unwrap();
            assert_eq!(open(&sealed, "123456", &TAG).err(), Some(Error::UnsupportedVault));
        }
    }

    #[test]
    fn rejects_bad_pin_formats_and_vaults() {
        for pin in ["12345", "1234567", "12345a", ""] {
            assert_eq!(seal(&[1u8; 16], "", pin, &TAG, FAST).unwrap_err(), Error::InvalidPin);
        }
        assert_eq!(open(&[9u8; 80], "123456", &TAG).err(), Some(Error::UnsupportedVault));
        assert_eq!(open(&MAGIC, "123456", &TAG).err(), Some(Error::UnsupportedVault));
        assert_eq!(
            open_entropy_only(&ENTROPY_MAGIC, "123456", &TAG).unwrap_err(),
            Error::UnsupportedVault
        );
        assert_eq!(open_legacy(&[1u8; 4], "123456").unwrap_err(), Error::UnsupportedVault);
    }
}

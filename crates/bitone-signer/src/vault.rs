//! 种子加密.
//!
//! 解密密钥由 PIN 和硬件的 HMAC 结果一起派生.每猜一次 PIN 都要手机硬件算一次,
//! 所以密文拿走之后没法离线试 PIN.

use crate::Error;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::{Zeroize, Zeroizing};

/// 旧格式的第一个字节.解密密钥只来自 PIN,平台层外面再包一层硬件加密.
const LEGACY_FORMAT: u8 = 1;
/// 当前格式的开头.旧格式的文件以随机 IV 开头,要靠这 8 个字节区分.
const MAGIC: [u8; 8] = *b"BITONE02";
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
fn seal_with(prefix: &[u8], entropy: &[u8], password: &[u8], params: KdfParams) -> Result<Vec<u8>, Error> {
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
                msg: entropy,
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
    let entropy = Zeroizing::new(plain.clone());
    plain.zeroize();
    Ok(entropy)
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

/// 密文是不是当前格式.不是时要走 [`open_legacy`].
pub fn is_bound(sealed: &[u8]) -> bool {
    sealed.starts_with(&MAGIC)
}

/// 用 PIN 和硬件结果加密熵.
pub fn seal(entropy: &[u8], pin: &str, tag: &[u8], params: KdfParams) -> Result<Vec<u8>, Error> {
    seal_with(&MAGIC, entropy, &bound_password(pin, tag)?, params)
}

pub fn open(sealed: &[u8], pin: &str, tag: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let password = bound_password(pin, tag)?;
    if !is_bound(sealed) {
        return Err(Error::UnsupportedVault);
    }
    open_with(MAGIC.len(), sealed, &password)
}

/// 解开旧格式,只用于迁移.这里拿到的是平台层已经去掉硬件那层的内容.
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
    fn seal_and_open() {
        let sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        assert!(is_bound(&sealed));
        assert_eq!(*open(&sealed, "123456", &TAG).unwrap(), vec![7u8; 32]);
    }

    #[test]
    fn wrong_pin_fails() {
        let sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "654321", &TAG).unwrap_err(), Error::WrongPin);
    }

    /// 只有 PIN 解不开.这是硬件绑定的全部意义:离线试 PIN 拿不到硬件结果.
    #[test]
    fn the_right_pin_needs_the_hardware_result() {
        let sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "123456", &[6; TAG_LEN]).unwrap_err(), Error::WrongPin);
        assert_eq!(open_legacy(&sealed, "123456").unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn a_short_hardware_result_is_refused() {
        for tag in [&[][..], &[5; 31][..], &[5; 33][..]] {
            assert_eq!(
                seal(&[1u8; 16], "123456", tag, FAST).unwrap_err(),
                Error::UnsupportedVault
            );
        }
        let sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        assert_eq!(open(&sealed, "123456", &[]).unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn legacy_vaults_still_open() {
        let sealed = seal_with(&[LEGACY_FORMAT], &[7u8; 32], b"123456", FAST).unwrap();
        assert!(!is_bound(&sealed));
        assert_eq!(*open_legacy(&sealed, "123456").unwrap(), vec![7u8; 32]);
        assert_eq!(open_legacy(&sealed, "654321").unwrap_err(), Error::WrongPin);
        assert_eq!(open(&sealed, "123456", &TAG).unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn ciphertext_does_not_contain_the_entropy() {
        let sealed = seal(&[0xABu8; 32], "123456", &TAG, FAST).unwrap();
        assert!(!sealed.windows(8).any(|w| w == [0xAB; 8]));
    }

    #[test]
    fn same_input_produces_different_ciphertext() {
        assert_ne!(
            seal(&[1u8; 16], "123456", &TAG, FAST).unwrap(),
            seal(&[1u8; 16], "123456", &TAG, FAST).unwrap()
        );
    }

    #[test]
    fn tampering_is_detected() {
        let header = MAGIC.len() + PARAMS_LEN + SALT_LEN + NONCE_LEN;
        let sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        for at in [MAGIC.len() + PARAMS_LEN, header - 1, header, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[at] ^= 1;
            assert_eq!(open(&bad, "123456", &TAG).unwrap_err(), Error::WrongPin, "offset {at}");
        }
    }

    #[test]
    fn weakened_kdf_header_is_rejected() {
        let mut sealed = seal(&[7u8; 32], "123456", &TAG, FAST).unwrap();
        sealed[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&1024u32.to_le_bytes());
        assert_eq!(open(&sealed, "123456", &TAG).unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn rejects_bad_pin_formats_and_vaults() {
        for pin in ["12345", "1234567", "12345a", ""] {
            assert_eq!(seal(&[1u8; 16], pin, &TAG, FAST).unwrap_err(), Error::InvalidPin);
        }
        assert_eq!(open(&[9u8; 80], "123456", &TAG).unwrap_err(), Error::UnsupportedVault);
        assert_eq!(open(&MAGIC, "123456", &TAG).unwrap_err(), Error::UnsupportedVault);
        assert_eq!(open_legacy(&[1u8; 4], "123456").unwrap_err(), Error::UnsupportedVault);
    }
}

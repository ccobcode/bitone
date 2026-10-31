//! PIN 这一层加密，见 docs/04-security.md 第 2.3 节。
//!
//! 种子先用 PIN 派生的密钥加密，再由平台层用硬件密钥包一层。这里只管第一层。

use crate::Error;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::{Zeroize, Zeroizing};

const FORMAT: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 1 + 12 + SALT_LEN + NONCE_LEN;

/// Argon2id 参数。写进密文头，日后调整不影响已有钱包。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 初始值，要在低端手机上实测后校准。
    pub const DEFAULT: Self = Self {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// 内存和耗时的下限，防止篡改密文头来削弱暴力破解的成本。
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

/// 太容易猜的 PIN：最多两种数字，或者逐位加一、减一。设置时拒绝，已有的钱包不受影响。
pub fn is_weak_pin(pin: &str) -> bool {
    let digits: Vec<i8> = pin.bytes().map(|b| b as i8 - b'0' as i8).collect();
    let mut seen = digits.clone();
    seen.sort_unstable();
    seen.dedup();
    let step = |delta: i8| digits.windows(2).all(|pair| pair[1] - pair[0] == delta);
    seen.len() <= 2 || step(1) || step(-1)
}

fn derive_key(pin: &str, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>, Error> {
    let argon_params = Params::new(params.memory_kib, params.iterations, params.parallelism, Some(32))
        .map_err(|_| Error::UnsupportedVault)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params)
        .hash_password_into(pin.as_bytes(), salt, key.as_mut())
        .map_err(|_| Error::UnsupportedVault)?;
    Ok(key)
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("密钥长度固定为 32 字节")
}

/// 用 PIN 加密熵。头部不加密，但参与认证，改动头部会解密失败。
pub fn seal(entropy: &[u8], pin: &str, params: KdfParams) -> Result<Vec<u8>, Error> {
    check_pin(pin)?;
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|_| Error::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;

    let mut sealed = Vec::with_capacity(HEADER_LEN + entropy.len() + 16);
    sealed.push(FORMAT);
    sealed.extend_from_slice(&params.memory_kib.to_le_bytes());
    sealed.extend_from_slice(&params.iterations.to_le_bytes());
    sealed.extend_from_slice(&params.parallelism.to_le_bytes());
    sealed.extend_from_slice(&salt);
    sealed.extend_from_slice(&nonce);

    let key = derive_key(pin, &salt, params)?;
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

/// 用 PIN 解开。PIN 错误和数据损坏无法区分，都返回 `WrongPin`。
pub fn open(sealed: &[u8], pin: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    check_pin(pin)?;
    if sealed.len() <= HEADER_LEN || sealed[0] != FORMAT {
        return Err(Error::UnsupportedVault);
    }
    let field = |at: usize| u32::from_le_bytes(sealed[at..at + 4].try_into().expect("4 字节"));
    let params = KdfParams {
        memory_kib: field(1),
        iterations: field(5),
        parallelism: field(9),
    };
    if !params.is_acceptable() {
        return Err(Error::UnsupportedVault);
    }
    let (header, ciphertext) = sealed.split_at(HEADER_LEN);
    let salt = &header[13..13 + SALT_LEN];
    let nonce: [u8; NONCE_LEN] = header[13 + SALT_LEN..].try_into().expect("12 字节");
    let key = derive_key(pin, salt, params)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };

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
        let sealed = seal(&[7u8; 32], "123456", FAST).unwrap();
        assert_eq!(*open(&sealed, "123456").unwrap(), vec![7u8; 32]);
    }

    #[test]
    fn wrong_pin_fails() {
        let sealed = seal(&[7u8; 32], "123456", FAST).unwrap();
        assert_eq!(open(&sealed, "654321").unwrap_err(), Error::WrongPin);
    }

    #[test]
    fn ciphertext_does_not_contain_the_entropy() {
        let sealed = seal(&[0xABu8; 32], "123456", FAST).unwrap();
        assert!(!sealed.windows(8).any(|w| w == [0xAB; 8]));
    }

    #[test]
    fn same_input_produces_different_ciphertext() {
        assert_ne!(
            seal(&[1u8; 16], "123456", FAST).unwrap(),
            seal(&[1u8; 16], "123456", FAST).unwrap()
        );
    }

    #[test]
    fn tampering_is_detected() {
        let sealed = seal(&[7u8; 32], "123456", FAST).unwrap();
        for at in [13, HEADER_LEN - 1, HEADER_LEN, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[at] ^= 1;
            assert_eq!(open(&bad, "123456").unwrap_err(), Error::WrongPin, "offset {at}");
        }
    }

    #[test]
    fn weakened_kdf_header_is_rejected() {
        let mut sealed = seal(&[7u8; 32], "123456", FAST).unwrap();
        sealed[1..5].copy_from_slice(&1024u32.to_le_bytes());
        assert_eq!(open(&sealed, "123456").unwrap_err(), Error::UnsupportedVault);
    }

    #[test]
    fn rejects_bad_pin_formats_and_vaults() {
        for pin in ["12345", "1234567", "12345a", ""] {
            assert_eq!(seal(&[1u8; 16], pin, FAST).unwrap_err(), Error::InvalidPin);
        }
        assert_eq!(open(&[9u8; 80], "123456").unwrap_err(), Error::UnsupportedVault);
        assert_eq!(open(&[1u8; 4], "123456").unwrap_err(), Error::UnsupportedVault);
    }
}

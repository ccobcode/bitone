//! BIP39 助记词.保存的是熵和 BIP39 密码,不是种子,种子只在需要时算出.

use crate::Error;
use bip39::{Language, Mnemonic};
use zeroize::Zeroizing;

/// 12 词对应 16 字节熵,24 词对应 32 字节.
pub fn generate_entropy(word_count: usize) -> Result<Zeroizing<Vec<u8>>, Error> {
    let bytes = match word_count {
        12 => 16,
        24 => 32,
        _ => return Err(Error::InvalidWordCount),
    };
    let mut entropy = Zeroizing::new(vec![0u8; bytes]);
    getrandom::fill(&mut entropy).map_err(|_| Error::Random)?;
    Ok(entropy)
}

fn from_entropy(entropy: &[u8]) -> Result<Mnemonic, Error> {
    Mnemonic::from_entropy_in(Language::English, entropy).map_err(|_| Error::InvalidMnemonic)
}

pub fn words(entropy: &[u8]) -> Result<Vec<String>, Error> {
    Ok(from_entropy(entropy)?.words().map(String::from).collect())
}

/// 校验并转换用户输入的助记词.BIP39 的五种长度都接受,别处生成的钱包也要能导入.
pub fn entropy_from_words(words: &[String]) -> Result<Zeroizing<Vec<u8>>, Error> {
    if !matches!(words.len(), 12 | 15 | 18 | 21 | 24) {
        return Err(Error::InvalidWordCount);
    }
    let phrase = Zeroizing::new(words.join(" "));
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &phrase).map_err(|_| Error::InvalidMnemonic)?;
    Ok(Zeroizing::new(mnemonic.to_entropy()))
}

pub fn is_valid_word(word: &str) -> bool {
    Language::English.find_word(word).is_some()
}

/// 以 `prefix` 开头的词,给内置键盘做联想.
pub fn suggestions(prefix: &str, limit: usize) -> Vec<&'static str> {
    if prefix.is_empty() {
        return Vec::new();
    }
    Language::English.words_by_prefix_iter(prefix).take(limit).collect()
}

/// 词表里随机取一个词,作为验证助记词时的干扰项.
pub fn random_word() -> Result<&'static str, Error> {
    let mut bytes = [0u8; 2];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    let index = u16::from_le_bytes(bytes) as usize % 2048;
    Ok(Language::English.word_list()[index])
}

pub fn random_below(limit: usize) -> Result<usize, Error> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    Ok(u32::from_le_bytes(bytes) as usize % limit)
}

pub fn seed(entropy: &[u8], passphrase: &str) -> Result<Zeroizing<[u8; 64]>, Error> {
    Ok(Zeroizing::new(from_entropy(entropy)?.to_seed_normalized(passphrase)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn trezor_vectors() {
        let cases: [(&[u8], &str, [u8; 64]); 2] = [
            (
                &[0u8; 16],
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
                hex!(
                    "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04"
                ),
            ),
            (
                &[0xffu8; 16],
                "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
                hex!(
                    "ac27495480225222079d7be181583751e86f571027b0497b5b5d11218e0a8a13332572917f0f8e5a589620c6f15b11c61dee327651a14c34e18231052e48c069"
                ),
            ),
        ];
        for (entropy, phrase, expected) in cases {
            assert_eq!(words(entropy).unwrap().join(" "), phrase);
            assert_eq!(*seed(entropy, "TREZOR").unwrap(), expected);
        }
    }

    #[test]
    fn generated_entropy_has_the_requested_size_and_differs() {
        assert_eq!(generate_entropy(12).unwrap().len(), 16);
        assert_eq!(generate_entropy(24).unwrap().len(), 32);
        assert_ne!(*generate_entropy(24).unwrap(), *generate_entropy(24).unwrap());
        assert_eq!(generate_entropy(18).unwrap_err(), Error::InvalidWordCount);
    }

    #[test]
    fn import_round_trip_and_checksum() {
        let entropy = generate_entropy(24).unwrap();
        let list = words(&entropy).unwrap();
        assert_eq!(*entropy_from_words(&list).unwrap(), *entropy);

        let mut bad: Vec<String> = vec!["abandon".into(); 12];
        assert_eq!(entropy_from_words(&bad).unwrap_err(), Error::InvalidMnemonic);
        bad.pop();
        assert_eq!(entropy_from_words(&bad).unwrap_err(), Error::InvalidWordCount);
    }

    #[test]
    fn every_bip39_length_imports() {
        for (count, bytes) in [(15, 20), (18, 24), (21, 28)] {
            let entropy = vec![7u8; bytes];
            let list = words(&entropy).unwrap();
            assert_eq!(list.len(), count);
            assert_eq!(*entropy_from_words(&list).unwrap(), entropy);
        }
        assert_eq!(
            entropy_from_words(&vec!["abandon".to_string(); 13]).unwrap_err(),
            Error::InvalidWordCount
        );
    }

    #[test]
    fn keyboard_helpers() {
        assert!(is_valid_word("abandon"));
        assert!(!is_valid_word("bitone"));
        assert_eq!(suggestions("aban", 3), vec!["abandon"]);
        assert!(suggestions("", 3).is_empty());
    }
}

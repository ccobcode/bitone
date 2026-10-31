//! 签名.签名前一定重新核对一次,核对不通过的请求到不了私钥.

use crate::Error;
use crate::keys::Wallet;
use ::bitcoin::secp256k1::Message;
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::request::SignRequest;
use bitone_core::request::SignResponse;
use bitone_core::review::Review;
use bitone_core::tx::{self, HighestChange, ReviewContext};
use bitone_core::{Chain, Mode, Network};

mod bitcoin;
mod evm;
mod solana;
mod tron;

impl Wallet {
    /// 全部账户的公钥,写入本机后不用解锁也能核对.
    pub fn public_keys(&self, imported: bool) -> Result<Vec<u8>, Error> {
        Ok(PublicKeys::encode(&self.export(imported)?))
    }

    /// 核对后签名.`Review` 不可签名时返回 `Refused`,不接触私钥.
    pub fn sign(
        &self,
        request: &SignRequest,
        keys: &PublicKeys,
        manifest: &Manifest,
        highest_change: HighestChange,
    ) -> Result<(SignResponse, Review), Error> {
        let ctx = ReviewContext {
            keys,
            manifest,
            highest_change,
        };
        let review = tx::review(request, &ctx);
        if !review.is_signable() {
            return Err(Error::Refused);
        }
        let network = Network::from_id(&request.network).ok_or(Error::Refused)?;
        let signatures = match network.chain() {
            Chain::Evm => vec![self.sign_evm(request, network)?],
            Chain::Bitcoin => self.sign_bitcoin(request, network)?,
            Chain::Tron => vec![self.sign_tron(request, network)?],
            Chain::Solana => vec![self.sign_solana(request, network)?],
        };
        let response = SignResponse {
            version: bitone_core::protocol::VERSION,
            id: request.id,
            signatures: signatures.into_iter().map(Into::into).collect(),
        };
        Ok((response, review))
    }

    /// RFC 6979 的确定性签名,低 s.返回 r,s,恢复位.
    pub(crate) fn sign_recoverable(&self, path: &str, digest: [u8; 32], mode: Mode) -> Result<Vec<u8>, Error> {
        let mut key = self.secp_key(path, mode)?.private_key;
        let signature = self.secp().sign_ecdsa_recoverable(&Message::from_digest(digest), &key);
        key.non_secure_erase();
        let (recovery, compact) = signature.serialize_compact();
        let mut out = compact.to_vec();
        out.push(recovery.to_i32() as u8);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitone_core::review::Forbid;

    const EVM_PATH: &str = "m/44'/60'/0'/0/0";
    const PEER: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

    fn keys() -> PublicKeys {
        let wallet = Wallet::new(&[0u8; 16], "").unwrap();
        PublicKeys::decode(&wallet.public_keys(false).unwrap()).unwrap()
    }

    #[test]
    fn cold_wallet_recognises_its_own_addresses_only() {
        use bitone_core::protocol::AddressCheck;
        let keys = keys();
        let cases = [
            (
                Network::Bitcoin,
                "m/84'/0'/0'/0/1",
                "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g",
            ),
            (
                Network::Bitcoin,
                "m/86'/0'/0'/1/0",
                "bc1p3qkhfews2uk44qtvauqyr2ttdsw7svhkl9nkm9s9c3x4ax5h60wqwruhk7",
            ),
            (
                Network::Ethereum,
                EVM_PATH,
                "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
            ),
            (Network::Tron, "m/44'/195'/0'/0/0", "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH"),
            (
                Network::SolanaDevnet,
                "m/44'/1'/0'/0'",
                "9BbGG7UMnNRJGMk1m52F4Q4gCSUnSVBosiT9kJhmgWvQ",
            ),
        ];
        for (network, path, address) in cases {
            let good = AddressCheck::new(keys.fingerprint, network, path, address).unwrap();
            assert!(tx::check_address(&good, &keys).unwrap().belongs, "{path}");
            let bad = AddressCheck::new(keys.fingerprint, network, path, "bc1qattacker").unwrap();
            let result = tx::check_address(&bad, &keys).unwrap();
            assert!(!result.belongs);
            assert_eq!(result.derived, address);
        }
        let stranger = AddressCheck::new([0; 4], Network::Ethereum, EVM_PATH, PEER).unwrap();
        assert_eq!(tx::check_address(&stranger, &keys), Err(Forbid::FingerprintMismatch));
    }

    #[test]
    fn accounts_list_their_first_address() {
        let keys = keys();
        let main = keys.addresses().unwrap();
        assert_eq!(main.len(), 5);
        assert_eq!(main[0].address, "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        assert_eq!(main[4].address, "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk");
    }

    #[test]
    fn receive_addresses_come_from_the_cold_wallet_keys() {
        let keys = keys();
        let second = keys.receive_address("m/84'/0'/0'", 1).unwrap();
        assert_eq!(second, "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g");
        let evm = keys.receive_address(EVM_PATH, 0).unwrap();
        assert_eq!(evm, "0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
        assert!(keys.receive_address(EVM_PATH, 1).is_err());
        assert!(keys.receive_address("m/84'/0'/9'", 0).is_err());
        assert!(keys.receive_address("m/84'/0'/0'", 100_000).is_err());
    }
}

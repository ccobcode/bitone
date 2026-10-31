//! 签名。签名前一定重新核对一次，核对不通过的请求到不了私钥。

use crate::Error;
use crate::keys::Wallet;
use bitcoin::secp256k1::Message;
use bitone_core::keys::PublicKeys;
use bitone_core::manifest::Manifest;
use bitone_core::request::SignRequest;
use bitone_core::request::SignResponse;
use bitone_core::review::Review;
use bitone_core::tx::{self, HighestChange, ReviewContext, evm};
use bitone_core::{Chain, DerivationPath, Mode, Network};

mod bitcoin_tx;
mod solana;
mod tron;

impl Wallet {
    /// 全部账户的公钥，写入本机后不用解锁也能核对。
    pub fn public_keys(&self, imported: bool) -> Result<Vec<u8>, Error> {
        Ok(PublicKeys::encode(&self.export(imported)?))
    }

    /// 核对后签名。`Review` 不可签名时返回 `Refused`，不接触私钥。
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

    /// RFC 6979 的确定性签名，低 s。返回 r、s、恢复位。
    pub(crate) fn sign_recoverable(&self, path: &str, digest: [u8; 32], mode: Mode) -> Result<Vec<u8>, Error> {
        let mut key = self.secp_key(path, mode)?.private_key;
        let signature = self.secp().sign_ecdsa_recoverable(&Message::from_digest(digest), &key);
        key.non_secure_erase();
        let (recovery, compact) = signature.serialize_compact();
        let mut out = compact.to_vec();
        out.push(recovery.to_i32() as u8);
        Ok(out)
    }

    fn sign_evm(&self, request: &SignRequest, network: Network) -> Result<Vec<u8>, Error> {
        let [path] = request.paths.as_slice() else {
            return Err(Error::Refused);
        };
        DerivationPath::parse(path)?;
        let digest = evm::signing_hash(request).map_err(|_| Error::Refused)?;
        self.sign_recoverable(path, digest, network.mode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{KIND_TRANSACTION, KIND_TYPED_DATA};
    use bitone_core::review::{Confirm, Forbid, Risk};
    use bitone_core::tx::evm::{TransferParams, UnsignedTx, build_transfer};

    const EVM_PATH: &str = "m/44'/60'/0'/0/0";
    const PEER: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
    const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

    struct Setup {
        wallet: Wallet,
        keys: PublicKeys,
    }

    fn setup() -> Setup {
        let wallet = Wallet::new(&[0u8; 16], "").unwrap();
        let keys = PublicKeys::decode(&wallet.public_keys(false).unwrap()).unwrap();
        Setup { wallet, keys }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn request(s: &Setup, network: &str, payload: Vec<u8>) -> SignRequest {
        SignRequest {
            version: VERSION,
            id: [9; 16],
            fingerprint: s.keys.fingerprint,
            network: network.into(),
            kind: KIND_TRANSACTION,
            paths: vec![EVM_PATH.into()],
            payload,
            hint: None,
        }
    }

    fn review(s: &Setup, request: &SignRequest) -> Review {
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange::default(),
        };
        tx::review(request, &ctx)
    }

    fn native(nonce: u64) -> UnsignedTx {
        build_transfer(&TransferParams {
            network: Network::Ethereum,
            to: PEER,
            amount: "1500000000000000000",
            contract: None,
            nonce,
            gas_limit: 21000,
            max_fee: 30_000_000_000,
            max_priority: 2_000_000_000,
        })
        .unwrap()
    }

    fn token() -> UnsignedTx {
        build_transfer(&TransferParams {
            network: Network::Ethereum,
            to: PEER,
            amount: "1250000000",
            contract: Some(USDT),
            nonce: 8,
            gas_limit: 65000,
            max_fee: 20_000_000_000,
            max_priority: 1_000_000_000,
        })
        .unwrap()
    }

    #[test]
    fn plain_native_transfer() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", native(7).encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.5", Some("ETH")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
    }

    #[test]
    fn plain_token_transfer() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", token().encode()));
        assert_eq!(review.risk, Risk::Transfer);
        assert_eq!(review.amount.unwrap().symbol.as_deref(), Some("USDT"));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
    }

    /// 期望值由 viem 用同一助记词算出，是独立实现。
    #[test]
    fn matches_viem_byte_for_byte() {
        let s = setup();
        let key = s.keys.secp256k1(&DerivationPath::parse(EVM_PATH).unwrap()).unwrap();
        let cases = [
            (
                native(7),
                "02f0010784773594008506fc23ac008252089470997970c51812dc3a010c7d01b50e0d17dc79c88814d1120d7b16000080c0",
                "02f873010784773594008506fc23ac008252089470997970c51812dc3a010c7d01b50e0d17dc79c88814d1120d7b16000080c001a04e219f5592dfa970f0d663ceef21498f55cde778b9a91afef1bdb4638853e12aa019215a49c9a5d76e9f1ef7ec7c879d3721f9ac5ad88643e11c863f0b8e096fb1",
                "e5021c2daa3e051acced4d3478dc03ee8e4cff17f822e9e7cf8f64744ae05e9c",
            ),
            (
                token(),
                "02f86d0108843b9aca008504a817c80082fde894dac17f958d2ee523a2206206994597c13d831ec780b844a9059cbb00000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c8000000000000000000000000000000000000000000000000000000004a817c80c0",
                "02f8b00108843b9aca008504a817c80082fde894dac17f958d2ee523a2206206994597c13d831ec780b844a9059cbb00000000000000000000000070997970c51812dc3a010c7d01b50e0d17dc79c8000000000000000000000000000000000000000000000000000000004a817c80c001a07cc650c0ab908591c727b377e247af85b2e3808632025d204474f0c1d75b2883a018ebfeefa919fc318a0b6e32791407eebb39b586a3ea412c3ed4d6b230255449",
                "674ed52ba0604c097b93322109672dfacc9c7213424c83fc43a2de7e5d9c14de",
            ),
        ];
        for (tx, unsigned, raw, hash) in cases {
            assert_eq!(hex(&tx.encode()), unsigned);
            let req = request(&s, "ethereum", tx.encode());
            let (response, _) = s
                .wallet
                .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
                .unwrap();
            let verified = tx::verify(&req, &response, &key).unwrap();
            assert_eq!(hex(&verified.raw), raw);
            assert_eq!(verified.hash, format!("0x{hash}"));
        }
    }

    #[test]
    fn legacy_with_chain_id_matches_viem() {
        let s = setup();
        let key = s.keys.secp256k1(&DerivationPath::parse(EVM_PATH).unwrap()).unwrap();
        let unsigned = unhex("e30384b2d05e008252089470997970c51812dc3a010c7d01b50e0d17dc79c80180388080");
        let req = request(&s, "bsc", unsigned);
        let (response, review) = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap();
        assert_eq!(review.risk, Risk::Transfer);
        let verified = tx::verify(&req, &response, &key).unwrap();
        assert_eq!(
            hex(&verified.raw),
            "f8640384b2d05e008252089470997970c51812dc3a010c7d01b50e0d17dc79c801808193a09e863fe043cb922c59e09d6f0b490659f23e31dbf5300619323a88a7a969c969a01b1614c8a93b08d7508876dc88a591ee8ff861694580c52e0e26f1d193b7592c"
        );
    }

    #[test]
    fn signature_verifies_and_rejects_tampering() {
        let s = setup();
        let req = request(&s, "ethereum", native(7).encode());
        let (response, _) = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap();
        let key = s.keys.secp256k1(&DerivationPath::parse(EVM_PATH).unwrap()).unwrap();
        let verified = tx::verify(&req, &response, &key).unwrap();
        assert!(verified.hash.starts_with("0x"));

        let mut other = req.clone();
        other.payload = native(8).encode();
        assert!(tx::verify(&other, &response, &key).is_err());

        let mut wrong_id = response.clone();
        wrong_id.id = [1; 16];
        assert!(tx::verify(&req, &wrong_id, &key).is_err());

        let stranger = [2u8; 33];
        assert!(tx::verify(&req, &response, &stranger).is_err());
    }

    fn forbidden(review: Review) -> Option<Forbid> {
        assert_eq!(review.confirm, Confirm::Refuse);
        review.forbidden
    }

    #[test]
    fn wrong_chain_id_is_refused_and_cannot_be_signed() {
        let s = setup();
        let req = request(&s, "polygon", native(1).encode());
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ChainIdMismatch));
        let result = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default());
        assert_eq!(result.unwrap_err(), Error::Refused);
    }

    #[test]
    fn legacy_without_chain_id_is_refused() {
        // viem 的 serializeTransaction({ type: 'legacy' }) 不带 chainId 时的结果。
        let payload = unhex("e780843b9aca008252089470997970c51812dc3a010c7d01b50e0d17dc79c88701c6bf5263400080");
        let s = setup();
        assert_eq!(
            forbidden(review(&s, &request(&s, "ethereum", payload))),
            Some(Forbid::LegacyWithoutChainId)
        );
    }

    #[test]
    fn non_canonical_encoding_is_refused() {
        let s = setup();
        let mut trailing = native(1).encode();
        trailing.push(0);
        assert_eq!(
            forbidden(review(&s, &request(&s, "ethereum", trailing))),
            Some(Forbid::ParseFailed)
        );
        assert_eq!(
            forbidden(review(&s, &request(&s, "ethereum", vec![]))),
            Some(Forbid::ParseFailed)
        );
        assert_eq!(
            forbidden(review(&s, &request(&s, "ethereum", vec![0x03, 0xc0]))),
            Some(Forbid::Unsupported)
        );
    }

    #[test]
    fn fingerprint_network_and_path_checks_come_first() {
        let s = setup();
        let mut req = request(&s, "ethereum", native(1).encode());
        req.fingerprint = [0; 4];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::FingerprintMismatch));

        let mut req = request(&s, "dogecoin", native(1).encode());
        req.network = "dogecoin".into();
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::UnknownNetwork));

        let mut req = request(&s, "ethereum", native(1).encode());
        req.paths = vec!["m/44'/60'/0'/0/1".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));

        req.paths = vec!["m/84'/0'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));

        req.paths = vec![];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
    }

    #[test]
    fn high_fee_needs_a_long_press() {
        let s = setup();
        let mut tx = native(1);
        tx.gas_limit = 10_000_000;
        tx.fees = bitone_core::tx::evm::Fees::Dynamic {
            max_priority: 1,
            max_fee: 100_000_000_000,
        };
        let review = review(&s, &request(&s, "ethereum", tx.encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
    }

    #[test]
    fn unknown_contract_shows_raw_data() {
        let s = setup();
        let mut tx = native(1);
        tx.data = unhex("deadbeef000000000000000000000000000000000000000000000000000000000000002a");
        let review = review(&s, &request(&s, "ethereum", tx.encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(review.raw.len(), 2);
        assert!(review.rows.iter().any(|r| r.label == "附带金额"));
    }

    #[test]
    fn contract_creation_is_unknown() {
        let s = setup();
        let mut tx = native(1);
        tx.to = None;
        tx.data = vec![0x60];
        assert_eq!(review(&s, &request(&s, "ethereum", tx.encode())).risk, Risk::Unknown);
    }

    fn with_authorization(chain_id: u8) -> Vec<u8> {
        let mut tx = native(1);
        tx.tx_type = 4;
        tx.authorizations = vec![];
        // 授权元组：chain_id、地址、nonce、y、r、s。
        let mut tuple = vec![if chain_id == 0 { 0x80 } else { chain_id }, 0x94];
        tuple.extend_from_slice(&[0x33; 20]);
        tuple.extend_from_slice(&[0x80, 0x80, 0x01, 0x01]);
        let mut list = vec![0xc0 + tuple.len() as u8];
        list.extend_from_slice(&tuple);
        let mut outer = vec![0xc0 + list.len() as u8];
        outer.extend_from_slice(&list);
        tx.authorization_list = outer;
        tx.encode()
    }

    #[test]
    fn eip7702_delegation_is_a_permission_change() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", with_authorization(1)));
        assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
        assert_eq!(review.counterparty_label.as_deref(), Some("新控制方"));
    }

    /// 委托的同时转账时，页面顶部是新控制方，收款方不能因此消失。
    #[test]
    fn eip7702_keeps_the_outer_transfer_visible() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", with_authorization(1)));
        let shown = |value: &str| review.actions.iter().any(|a| a.rows.iter().any(|r| r.value == value));
        assert!(shown(PEER));
        assert!(shown("1.5 ETH"));
    }

    #[test]
    fn universal_eip7702_delegation_is_refused() {
        let s = setup();
        assert_eq!(
            forbidden(review(&s, &request(&s, "ethereum", with_authorization(0)))),
            Some(Forbid::UniversalAuthorization)
        );
    }

    fn typed(s: &Setup, primary: &str, chain_id: serde_json::Value) -> SignRequest {
        let data = serde_json::json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                primary: [
                    {"name": "owner", "type": "address"},
                    {"name": "spender", "type": "address"},
                    {"name": "value", "type": "uint256"}
                ]
            },
            "primaryType": primary,
            "domain": {"name": "Tether", "chainId": chain_id, "verifyingContract": USDT},
            "message": {"owner": PEER, "spender": PEER, "value": "115792089237316195423570985008687907853269984665640564039457584007913129639935"}
        });
        let mut req = request(s, "ethereum", serde_json::to_vec(&data).unwrap());
        req.kind = KIND_TYPED_DATA;
        req
    }

    #[test]
    fn permit_is_an_unlimited_approval() {
        let s = setup();
        let review = review(&s, &typed(&s, "Permit", 1.into()));
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
        assert!(review.amount.unwrap().unlimited);
    }

    #[test]
    fn permit_for_another_chain_is_refused() {
        let s = setup();
        assert_eq!(
            forbidden(review(&s, &typed(&s, "Permit", 137.into()))),
            Some(Forbid::ChainIdMismatch)
        );
    }

    #[test]
    fn other_typed_messages_are_refused() {
        let s = setup();
        assert_eq!(
            forbidden(review(&s, &typed(&s, "Order", 1.into()))),
            Some(Forbid::MessageSigning)
        );
    }

    #[test]
    fn unknown_request_kinds_are_refused() {
        let s = setup();
        let mut req = request(&s, "ethereum", vec![0; 32]);
        req.kind = 9;
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::RawHashSigning));
    }

    #[test]
    fn cold_wallet_recognises_its_own_addresses_only() {
        use bitone_core::protocol::AddressCheck;
        let s = setup();
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
            let good = AddressCheck::new(s.keys.fingerprint, network, path, address).unwrap();
            assert!(tx::check_address(&good, &s.keys).unwrap().belongs, "{path}");
            let bad = AddressCheck::new(s.keys.fingerprint, network, path, "bc1qattacker").unwrap();
            let result = tx::check_address(&bad, &s.keys).unwrap();
            assert!(!result.belongs);
            assert_eq!(result.derived, address);
        }
        let stranger = AddressCheck::new([0; 4], Network::Ethereum, EVM_PATH, PEER).unwrap();
        assert_eq!(tx::check_address(&stranger, &s.keys), Err(Forbid::FingerprintMismatch));
    }

    #[test]
    fn accounts_list_their_first_address() {
        let s = setup();
        let main = s.keys.addresses().unwrap();
        assert_eq!(main.len(), 5);
        assert_eq!(main[0].address, "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        assert_eq!(main[4].address, "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk");
    }

    #[test]
    fn receive_addresses_come_from_the_cold_wallet_keys() {
        let s = setup();
        let second = s.keys.receive_address("m/84'/0'/0'", 1).unwrap();
        assert_eq!(second, "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g");
        let evm = s.keys.receive_address(EVM_PATH, 0).unwrap();
        assert_eq!(evm, "0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
        assert!(s.keys.receive_address(EVM_PATH, 1).is_err());
        assert!(s.keys.receive_address("m/84'/0'/9'", 0).is_err());
        assert!(s.keys.receive_address("m/84'/0'/0'", 100_000).is_err());
    }

    #[test]
    fn public_keys_review_without_unlocking() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", native(7).encode()));
        assert!(
            review
                .rows
                .iter()
                .any(|r| r.value == "0x9858EfFD232B4033E47d90003D41EC34EcaEda94")
        );
    }
}

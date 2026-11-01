use crate::Error;
use crate::keys::Wallet;
use bitone_core::request::SignRequest;
use bitone_core::tx::evm;
use bitone_core::{DerivationPath, Network};

impl Wallet {
    pub(crate) fn sign_evm(&self, request: &SignRequest, network: Network) -> Result<Vec<u8>, Error> {
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
    use bitone_core::keys::PublicKeys;
    use bitone_core::manifest::Manifest;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{KIND_TRANSACTION, KIND_TYPED_DATA};
    use bitone_core::review::{Confirm, Forbid, Review, Risk};
    use bitone_core::tx::evm::{TransferParams, UnsignedTx, build_transfer};
    use bitone_core::tx::{self, HighestChange, ReviewContext};

    const EVM_PATH: &str = "m/44'/60'/0'/0/0";
    const PEER: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
    const USDT: &str = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
    const PERMIT2: &str = "0x000000000022D473030F116dDEE9F6B43aC78BA3";
    const UINT160_MAX: &str = "1461501637330902918203684832716283019655932542975";

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
            chain: None,
        }
    }

    fn review(s: &Setup, request: &SignRequest) -> Review {
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange::default(),
        };
        let review = tx::review(request, &ctx);
        assert!(review.fields.is_empty(), "这条链还没迁移");
        review
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

    /// 期望值由 viem 用同一助记词算出,是独立实现.
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

    /// 清单包可以整体覆盖内置清单.少了原生币这一行也要能签,不能卡住资金.
    #[test]
    fn a_manifest_without_the_native_row_still_signs_with_a_warning() {
        let s = setup();
        let mut manifest = Manifest::builtin().clone();
        manifest.assets.retain(|a| a.network != "ethereum");
        let req = request(&s, "ethereum", native(1).encode());
        let (_, review) = s
            .wallet
            .sign(&req, &s.keys, &manifest, HighestChange::default())
            .unwrap();
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(review.alerts, ["清单里没有这个网络的原生币,金额按链本身的数据显示"]);
        assert_eq!(review.amount.unwrap().symbol.as_deref(), Some("ETH"));
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
        // viem 的 serializeTransaction({ type: 'legacy' }) 不带 chainId 时的结果.
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

    #[test]
    fn contract_creation_shows_its_value() {
        let s = setup();
        for data in [Vec::new(), vec![0x60]] {
            let mut tx = native(1);
            tx.to = None;
            tx.data = data;
            let result = review(&s, &request(&s, "ethereum", tx.encode()));
            assert_eq!(result.risk, Risk::Unknown);
            assert_eq!(result.amount.as_ref().unwrap().text, "1.5");
            assert_eq!(row(&result, "附带金额"), "1.5 ETH");
        }
        let mut free = native(1);
        free.to = None;
        free.value = Vec::new();
        assert_eq!(review(&s, &request(&s, "ethereum", free.encode())).amount, None);
    }

    fn with_authorization(chain_id: u8) -> Vec<u8> {
        let mut tx = native(1);
        tx.tx_type = 4;
        tx.authorizations = vec![];
        // 授权元组:chain_id,地址,nonce,y,r,s.
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

    /// 委托的同时转账时,页面顶部是新控制方,收款方不能因此消失.
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

    fn typed_data(primary: &str, chain_id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                primary: [
                    {"name": "owner", "type": "address"},
                    {"name": "spender", "type": "address"},
                    {"name": "value", "type": "uint256"},
                    {"name": "nonce", "type": "uint256"},
                    {"name": "deadline", "type": "uint256"}
                ]
            },
            "primaryType": primary,
            "domain": {"name": "Tether", "chainId": chain_id, "verifyingContract": USDT},
            "message": {
                "owner": PEER,
                "spender": PEER,
                "value": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
                "nonce": 0,
                "deadline": 1
            }
        })
    }

    fn typed_request(s: &Setup, data: &serde_json::Value) -> SignRequest {
        let mut req = request(s, "ethereum", serde_json::to_vec(data).unwrap());
        req.kind = KIND_TYPED_DATA;
        req
    }

    fn typed(s: &Setup, primary: &str, chain_id: serde_json::Value) -> SignRequest {
        typed_request(s, &typed_data(primary, chain_id))
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

    fn permit2(primary: &str, details_type: &str, details: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                primary: [
                    {"name": "details", "type": details_type},
                    {"name": "spender", "type": "address"},
                    {"name": "sigDeadline", "type": "uint256"}
                ],
                "PermitDetails": [
                    {"name": "token", "type": "address"},
                    {"name": "amount", "type": "uint160"},
                    {"name": "expiration", "type": "uint48"},
                    {"name": "nonce", "type": "uint48"}
                ]
            },
            "primaryType": primary,
            "domain": {"name": "Permit2", "chainId": 1, "verifyingContract": PERMIT2},
            "message": {"details": details, "spender": PEER, "sigDeadline": 2}
        })
    }

    fn grant(token: &str, amount: &str) -> serde_json::Value {
        serde_json::json!({"token": token, "amount": amount, "expiration": 1, "nonce": 0})
    }

    fn row<'a>(review: &'a Review, label: &str) -> &'a str {
        review
            .rows
            .iter()
            .chain(review.actions.iter().flat_map(|a| &a.rows))
            .find(|r| r.label == label)
            .map_or("", |r| r.value.as_str())
    }

    #[test]
    fn permit2_is_an_approval() {
        let s = setup();
        let data = permit2("PermitSingle", "PermitDetails", grant(USDT, "1000000"));
        let review = review(&s, &typed_request(&s, &data));
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
        assert_eq!(review.amount.as_ref().unwrap().text, "1");
        assert!(review.alerts.is_empty());
    }

    #[test]
    fn permit_batch_has_no_headline_amount() {
        let s = setup();
        let details = serde_json::json!([grant(USDT, "1000000"), grant(PEER, "5")]);
        let data = permit2("PermitBatch", "PermitDetails[]", details);
        let review = review(&s, &typed_request(&s, &data));
        assert_eq!(review.risk, Risk::Approval);
        assert_eq!(review.amount, None);
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(review.actions.len(), 2);
        assert_eq!(review.alerts, vec!["这笔签名包含 2 个授权,请逐个核对"]);
    }

    #[test]
    fn permit_batch_names_its_unlimited_grants() {
        let s = setup();
        let details = serde_json::json!([grant(USDT, "1"), grant(PEER, UINT160_MAX)]);
        let data = permit2("PermitBatch", "PermitDetails[]", details);
        let review = review(&s, &typed_request(&s, &data));
        assert_eq!(review.amount, None);
        assert_eq!(
            review.alerts,
            vec![
                "这笔签名包含 2 个授权,请逐个核对",
                "其中 1 个是无限额度,对方可随时转走全部这种资产"
            ]
        );
    }

    #[test]
    fn typed_rows_show_the_contract_and_the_deadlines() {
        let s = setup();
        let result = review(&s, &typed(&s, "Permit", 1.into()));
        assert_eq!(row(&result, "验证合约"), USDT);
        assert_eq!(row(&result, "签名截止时间"), "1970-01-01 00:00:01 UTC");

        let single = permit2("PermitSingle", "PermitDetails", grant(USDT, "1000000"));
        let result = review(&s, &typed_request(&s, &single));
        assert_eq!(row(&result, "验证合约"), PERMIT2);
        assert_eq!(row(&result, "签名截止时间"), "1970-01-01 00:00:02 UTC");
        assert_eq!(row(&result, "额度过期时间"), "1970-01-01 00:00:01 UTC");
    }

    #[test]
    fn deadlines_without_a_date_stay_raw_integers() {
        let s = setup();
        let mut data = typed_data("Permit", 1.into());
        data["message"]["deadline"] = UINT160_MAX.into();
        let result = review(&s, &typed_request(&s, &data));
        assert_eq!(row(&result, "签名截止时间"), UINT160_MAX);
    }

    /// DAI 的 Permit 同名不同字段,`allowed` 才是实际授权,`value` 不进哈希.曾经显示额度 1,实际签下无限授权.
    #[test]
    fn dai_style_permit_is_refused() {
        let s = setup();
        let mut data = typed_data("Permit", 1.into());
        data["types"]["Permit"] = serde_json::json!([
            {"name": "holder", "type": "address"},
            {"name": "spender", "type": "address"},
            {"name": "nonce", "type": "uint256"},
            {"name": "expiry", "type": "uint256"},
            {"name": "allowed", "type": "bool"}
        ]);
        data["message"] = serde_json::json!({
            "holder": PEER, "spender": PEER, "nonce": 0, "expiry": 0, "allowed": true, "value": "1"
        });
        assert_eq!(
            forbidden(review(&s, &typed_request(&s, &data))),
            Some(Forbid::MessageSigning)
        );
    }

    #[test]
    fn fields_outside_the_hash_are_refused() {
        let s = setup();
        let mut extra_message = typed_data("Permit", 1.into());
        extra_message["message"]["memo"] = "hidden".into();
        let mut extra_domain = typed_data("Permit", 1.into());
        extra_domain["domain"]["salt"] = "hidden".into();
        for data in [extra_message, extra_domain] {
            assert_eq!(
                forbidden(review(&s, &typed_request(&s, &data))),
                Some(Forbid::ParseFailed)
            );
        }
    }

    /// chainId 不在域的类型里时不进哈希,校验 JSON 里的值没有意义,签名在所有链上都有效.
    #[test]
    fn domain_must_commit_to_the_chain_and_the_contract() {
        let s = setup();
        let without = |field: &str| {
            let mut data = typed_data("Permit", 1.into());
            data["types"]["EIP712Domain"]
                .as_array_mut()
                .unwrap()
                .retain(|f| f["name"] != field);
            data
        };
        assert_eq!(
            forbidden(review(&s, &typed_request(&s, &without("chainId")))),
            Some(Forbid::ChainIdMismatch)
        );
        assert_eq!(
            forbidden(review(&s, &typed_request(&s, &without("verifyingContract")))),
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

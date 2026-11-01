use crate::Error;
use crate::keys::Wallet;
use bitone_core::request::SignRequest;
use bitone_core::tx::evm;
use bitone_core::{DerivationPath, Mode};

impl Wallet {
    /// EVM 的路径不分主网和测试网.私钥类型只决定扩展私钥的版本前缀,派生结果和它无关.
    pub(crate) fn sign_evm(&self, request: &SignRequest) -> Result<Vec<u8>, Error> {
        let [path] = request.paths.as_slice() else {
            return Err(Error::Refused);
        };
        DerivationPath::parse(path)?;
        let digest = evm::signing_hash(request).map_err(|_| Error::Refused)?;
        self.sign_recoverable(path, digest, Mode::Mainnet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitone_core::keys::PublicKeys;
    use bitone_core::manifest::Manifest;
    use bitone_core::path::all_account_specs;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{KIND_PERSONAL_SIGN, KIND_RAW_HASH, KIND_TRANSACTION, KIND_TYPED_DATA};
    use bitone_core::review::{Confirm, FieldRow, Forbid, Review, Risk};
    use bitone_core::tx::evm::types::{
        AccessList, AccessListItem, Address, Authorization, B256, Bytes, Signature, SignedAuthorization, TxEip1559,
        TxEip2930, TxEip4844, TxEip7702, TxKind, U256, rlp_encode,
    };
    use bitone_core::tx::evm::{TransferParams, UnsignedTx, build_transfer, keccak};
    use bitone_core::tx::{self, HighestChange, ReviewContext};
    use bitone_core::{Chain, Network};

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
        tx::review(request, &ctx)
    }

    fn peer() -> Address {
        PEER.parse().unwrap()
    }

    fn eip1559(nonce: u64) -> TxEip1559 {
        TxEip1559 {
            chain_id: 1,
            nonce,
            gas_limit: 21000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 2_000_000_000,
            to: TxKind::Call(peer()),
            value: U256::from(1_500_000_000_000_000_000u64),
            access_list: AccessList::default(),
            input: Bytes::new(),
        }
    }

    fn changed(change: impl FnOnce(&mut TxEip1559)) -> Vec<u8> {
        let mut tx = eip1559(1);
        change(&mut tx);
        UnsignedTx::new(tx).encode()
    }

    fn field<'a>(review: &'a Review, name: &str) -> &'a FieldRow {
        review.fields.iter().find(|row| row.name == name).unwrap()
    }

    fn alerted(review: &Review, text: &str) -> bool {
        review.alerts.iter().any(|alert| alert.contains(text))
    }

    fn sign(s: &Setup, req: &SignRequest) -> Result<(bitone_core::request::SignResponse, Review), Error> {
        s.wallet
            .sign(req, &s.keys, Manifest::builtin(), HighestChange::default())
    }

    fn account_key(s: &Setup) -> [u8; 33] {
        s.keys.secp256k1(&DerivationPath::parse(EVM_PATH).unwrap()).unwrap()
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
        assert_eq!(field(&review, "chain_id").value, "56");
        let verified = tx::verify(&req, &response, &key).unwrap();
        assert_eq!(
            hex(&verified.raw),
            "f8640384b2d05e008252089470997970c51812dc3a010c7d01b50e0d17dc79c801808193a09e863fe043cb922c59e09d6f0b490659f23e31dbf5300619323a88a7a969c969a01b1614c8a93b08d7508876dc88a591ee8ff861694580c52e0e26f1d193b7592c"
        );
    }

    #[test]
    fn evm_review_fills_every_field_of_the_payload() {
        let s = setup();
        let result = review(&s, &request(&s, "ethereum", native(7).encode()));
        assert_eq!(result.field_count as usize, result.fields.len());
        assert_eq!(field(&result, "network").value, "ethereum");
        assert_eq!(field(&result, "derivation_path").value, EVM_PATH);
        assert_eq!(
            field(&result, "signer").value,
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
        );
        assert!(field(&result, "signer").own);
        assert_eq!(field(&result, "nonce").value, "7");
        assert_eq!(field(&result, "value").value, "1.5 ETH");
        assert_eq!(field(&result, "value").label, "金额");
        assert_eq!(field(&result, "fee_cap").value, "0.00063 ETH");
    }

    #[test]
    fn a_token_call_keeps_the_old_classification_and_names_the_method() {
        let s = setup();
        let result = review(&s, &request(&s, "ethereum", token().encode()));
        assert_eq!(result.risk, Risk::Transfer);
        let note = field(&result, "data").note.clone().unwrap();
        assert!(note.contains("transfer"), "{note}");
        assert_eq!(field(&result, "data").label, "调用数据");
    }

    fn call_to(contract: &str, data: Vec<u8>) -> Vec<u8> {
        UnsignedTx::new(TxEip1559 {
            to: TxKind::Call(contract.parse().unwrap()),
            value: U256::ZERO,
            input: data.into(),
            ..eip1559(1)
        })
        .encode()
    }

    fn approve_data(amount: &[u8]) -> Vec<u8> {
        evm::approve_data(&peer().into_array(), amount)
    }

    /// 钓鱼的主要入口:清单外的合约上,被授权方和额度一样要看得到.
    #[test]
    fn an_unlisted_approval_still_shows_the_spender_and_the_amount() {
        let s = setup();
        let req = request(&s, "ethereum", call_to(PEER, approve_data(&[0xff; 32])));
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(result.counterparty.as_deref(), Some(PEER));
        assert_eq!(result.counterparty_label.as_deref(), Some("被授权方"));
        assert!(result.amount.as_ref().unwrap().unlimited);
        assert!(alerted(&result, "不在内置清单"));
        assert!(alerted(&result, "额度是无限"));
        let spender = field(&result, "Spender");
        assert_eq!((spender.value.as_str(), spender.label.as_str()), (PEER, "被授权方"));
        assert_eq!(spender.source, bitone_core::decode::SourceKind::Descriptor);
        assert_eq!(field(&result, "data").value.len(), 136);
        assert!(sign(&s, &req).is_ok());
    }

    /// 第 7.1 节:描述文件只通过验签的清单到达,请求里的内容不算.
    #[test]
    fn a_descriptor_in_the_hint_means_nothing() {
        let s = setup();
        let mut req = request(&s, "ethereum", call_to(PEER, vec![0x12, 0x34, 0x56, 0x78]));
        let plain = review(&s, &req);
        req.hint = Some(
            "{\"network\":\"ethereum\",\"signature\":\"stake()\",\"intent\":\"Stake\",\"contract\":\"0x\"}".to_string(),
        );
        let hinted = review(&s, &req);
        assert_eq!(hinted.risk, Risk::Unknown);
        assert_eq!(hinted.review_digest, plain.review_digest);
    }

    #[test]
    fn a_listed_approval_is_an_approval_with_its_fields_in_the_list() {
        let s = setup();
        let result = review(&s, &request(&s, "ethereum", call_to(USDT, approve_data(&[1]))));
        assert_eq!((result.risk, result.confirm), (Risk::Approval, Confirm::Hold));
        assert_eq!(field(&result, "Amount").value, "0.000001 USDT");
        assert_eq!(result.field_count as usize, result.fields.len());
        assert!(result.alerts.is_empty());
    }

    /// 选择器对得上,参数解不开时不按偏移读,告警说出方法.
    #[test]
    fn an_irregular_approval_names_the_method() {
        let s = setup();
        let mut data = approve_data(&[1]);
        data.push(0);
        let result = review(&s, &request(&s, "ethereum", call_to(USDT, data)));
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "approve(address,uint256)"));
        assert_eq!(result.counterparty.as_deref(), Some(USDT));
        assert!(result.fields.iter().all(|row| row.name != "Spender"));
    }

    /// 普通类型的每一种都要能装回可以广播的字节.
    #[test]
    fn every_type_assembles_to_valid_signed_bytes() {
        let s = setup();
        let key = account_key(&s);
        let entry = AccessListItem {
            address: peer(),
            storage_keys: vec![B256::repeat_byte(1)],
        };
        let eip2930 = TxEip2930 {
            chain_id: 1,
            nonce: 1,
            gas_price: 2,
            gas_limit: 21000,
            to: TxKind::Call(peer()),
            value: U256::from(1),
            access_list: AccessList(vec![entry]),
            input: Bytes::new(),
        };
        let eip4844 = TxEip4844 {
            chain_id: 1,
            nonce: 1,
            gas_limit: 21000,
            max_fee_per_gas: 2,
            max_priority_fee_per_gas: 1,
            to: peer(),
            value: U256::ZERO,
            access_list: AccessList::default(),
            blob_versioned_hashes: vec![B256::repeat_byte(1)],
            max_fee_per_blob_gas: 1,
            input: Bytes::new(),
        };
        let payloads = [
            UnsignedTx::new(eip2930).encode(),
            native(1).encode(),
            UnsignedTx::new(eip4844).encode(),
            UnsignedTx::new(delegation(&s, 1, peer())).encode(),
        ];
        for payload in payloads {
            let req = request(&s, "ethereum", payload.clone());
            let (response, _) = sign(&s, &req).unwrap();
            let verified = tx::verify(&req, &response, &key).unwrap();
            assert!(!verified.signature_only);
            assert_eq!(verified.raw[0], payload[0], "类型字节");
            assert_ne!(verified.raw, payload);
        }
    }

    fn pre_155() -> Vec<u8> {
        // viem 的 serializeTransaction({ type: 'legacy' }) 不带 chainId 时的结果.
        unhex("e780843b9aca008252089470997970c51812dc3a010c7d01b50e0d17dc79c88701c6bf5263400080")
    }

    #[test]
    fn a_transaction_without_a_chain_id_warns_about_replay_and_signs() {
        let s = setup();
        let req = request(&s, "ethereum", pre_155());
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "每条 EVM 链上都能重放"));
        assert_eq!(field(&result, "chain_id").value, "无");
        assert!(sign(&s, &req).is_ok());
    }

    #[test]
    fn pre_155_signatures_use_27_or_28() {
        let s = setup();
        let req = request(&s, "ethereum", pre_155());
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        let signature = response.signatures[0].as_slice();
        let parity = signature[64];
        // 签名后的尾部是 v,r,s,r 和 s 都是 32 字节时 v 在倒数第 67 个字节.
        let v = verified.raw[verified.raw.len() - 67];
        assert_eq!(v, 27 + parity);
        let restored = UnsignedTx::decode(&pre_155()).unwrap();
        assert_eq!(restored.chain_id(), None);
    }

    #[test]
    fn a_replayable_transaction_gets_the_mainnet_prompt_even_on_a_testnet_name() {
        let s = setup();
        let result = review(&s, &request(&s, "sepolia", pre_155()));
        assert!(result.mainnet);
        let bound = changed(|tx| tx.chain_id = 11155111);
        assert!(!review(&s, &request(&s, "sepolia", bound)).mainnet);
    }

    #[test]
    fn blob_transactions_are_unknown_with_a_warning_and_sign() {
        let s = setup();
        let tx = TxEip4844 {
            chain_id: 1,
            nonce: 1,
            gas_limit: 21000,
            max_fee_per_gas: 2,
            max_priority_fee_per_gas: 1,
            to: peer(),
            value: U256::from(1),
            access_list: AccessList::default(),
            blob_versioned_hashes: vec![B256::repeat_byte(1)],
            max_fee_per_blob_gas: 3,
            input: Bytes::new(),
        };
        let req = request(&s, "ethereum", UnsignedTx::new(tx).encode());
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "blob 内容不在交易里"));
        assert_eq!(field(&result, "fee_cap").value, "0.000000000000435216 ETH");
        assert!(sign(&s, &req).is_ok());
    }

    /// 第 1.1 节:未知类型可以签,签的是 keccak256(载荷),长按之后还要二次确认.
    #[test]
    fn a_future_type_is_opaque_and_needs_the_second_confirmation() {
        let s = setup();
        let payload = vec![0x07, 0xc1, 0x01];
        let req = request(&s, "ethereum", payload.clone());
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::HoldConfirm));
        assert_eq!(result.fields[0].kind, bitone_core::decode::Kind::Opaque);
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        assert!(verified.signature_only);
        assert_eq!(verified.hash, format!("0x{}", hex(&keccak(&payload))));
    }

    fn permit_digest() -> Vec<u8> {
        [&[0x19, 0x01][..], &[0xaa; 32], &[0xbb; 32]].concat()
    }

    /// 这段字节的 keccak 就是一个 Permit 的签名原文,走 opaque 路径等于盲签授权.
    #[test]
    fn a_permit_digest_cannot_be_signed_as_opaque() {
        let s = setup();
        let result = review(&s, &request(&s, "ethereum", permit_digest()));
        assert_eq!(result.fields[0].name, "eip712_digest");
        assert!(
            result
                .fields
                .iter()
                .all(|row| row.kind != bitone_core::decode::Kind::Opaque)
        );
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::HoldConfirm));
        assert!(alerted(&result, "EIP-712"));
        assert!(!alerted(&result, "本机不认识这笔交易"));

        let mut short = permit_digest();
        short.pop();
        let req = request(&s, "ethereum", short);
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ParseFailed));
        assert_eq!(sign(&s, &req).unwrap_err(), Error::Refused);
    }

    /// `0x05 rlp(...)` 的 keccak 是账户委托的签名原文.
    #[test]
    fn an_eip7702_authorization_cannot_be_signed_as_opaque() {
        let s = setup();
        let inner = Authorization {
            chain_id: U256::from(1),
            address: peer(),
            nonce: 0,
        };
        let payload = [vec![0x05], rlp_encode(&inner)].concat();
        let req = request(&s, "ethereum", payload);
        let result = review(&s, &req);
        assert_eq!(result.fields[0].name, "eip7702_authorization");
        assert!(
            result
                .fields
                .iter()
                .all(|row| row.kind != bitone_core::decode::Kind::Opaque)
        );
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "EIP-7702 授权"));
        assert_eq!(evm::signing_hash(&req).unwrap(), inner.signature_hash().0);

        let garbage = request(&s, "ethereum", vec![0x05, 0x01, 0x02]);
        assert_eq!(forbidden(review(&s, &garbage)), Some(Forbid::ParseFailed));
        assert_eq!(sign(&s, &garbage).unwrap_err(), Error::Refused);
    }

    #[test]
    fn a_universal_authorization_message_warns_about_every_chain() {
        let s = setup();
        let inner = Authorization {
            chain_id: U256::ZERO,
            address: peer(),
            nonce: 0,
        };
        let payload = [vec![0x05], rlp_encode(&inner)].concat();
        let result = review(&s, &request(&s, "sepolia", payload));
        assert!(alerted(&result, "每条 EVM 链上都生效"));
        assert!(result.mainnet, "这个签名不限于测试网");
    }

    #[test]
    fn a_full_personal_message_under_the_transaction_kind_is_a_message() {
        let s = setup();
        let payload = [b"\x19Ethereum Signed Message:\n5".as_slice(), b"hello"].concat();
        let result = review(&s, &request(&s, "ethereum", payload));
        assert_eq!(field(&result, "message").value, "hello");
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
    }

    #[test]
    fn personal_sign_hashes_the_prefixed_message() {
        let s = setup();
        let mut req = request(&s, "ethereum", b"hello".to_vec());
        req.kind = KIND_PERSONAL_SIGN;
        let result = review(&s, &req);
        assert_eq!(field(&result, "message").value, "hello");
        assert_eq!(result.confirm, Confirm::Hold);
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        assert!(verified.signature_only);
        // viem 的 hashMessage("hello").
        assert_eq!(
            verified.hash,
            "0x50b2c43fd39106bafbba0da34fc430e1f91e3c96ea2acee2bc34119f92b37750"
        );
    }

    #[test]
    fn a_raw_hash_is_signed_as_it_is_after_the_second_confirmation() {
        let s = setup();
        let mut req = request(&s, "ethereum", vec![7; 32]);
        req.kind = KIND_RAW_HASH;
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::HoldConfirm));
        assert!(alerted(&result, "裸哈希"));
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        assert_eq!(verified.hash, format!("0x{}", hex(&[7; 32])));

        req.payload.pop();
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ParseFailed));
    }

    #[test]
    fn an_added_alert_keeps_the_second_confirmation() {
        let s = setup();
        let mut req = request(&s, "ethereum", vec![7; 32]);
        req.kind = KIND_RAW_HASH;
        req.version += 1;
        let result = review(&s, &req);
        assert_eq!(result.confirm, Confirm::HoldConfirm);
    }

    #[test]
    fn a_made_up_name_cannot_relabel_a_known_chain() {
        let s = setup();
        let req = request(&s, "made-up-mainnet", native(1).encode());
        let result = review(&s, &req);
        assert_eq!(result.network, "ethereum");
        assert_eq!(field(&result, "network").value, "ethereum");
        assert!(result.mainnet);
        assert_eq!(result.amount.as_ref().unwrap().symbol.as_deref(), Some("ETH"));
        assert_eq!((result.risk, result.confirm), (Risk::Transfer, Confirm::Hold));
        assert!(alerted(&result, "made-up-mainnet"));
        assert!(sign(&s, &req).is_ok());
    }

    #[test]
    fn a_made_up_name_on_a_testnet_chain_id_has_no_mainnet_prompt() {
        let s = setup();
        let payload = changed(|tx| tx.chain_id = 11155111);
        let result = review(&s, &request(&s, "made-up", payload));
        assert_eq!(result.network, "sepolia");
        assert!(!result.mainnet);
    }

    #[test]
    fn a_manifest_without_the_native_row_does_not_stop_a_renamed_network() {
        let s = setup();
        let mut manifest = Manifest::builtin().clone();
        manifest.assets.retain(|a| a.network != "ethereum");
        let req = request(&s, "made-up", native(1).encode());
        let (_, result) = s
            .wallet
            .sign(&req, &s.keys, &manifest, HighestChange::default())
            .unwrap();
        assert!(alerted(&result, "清单里没有这个网络的原生币"));
    }

    #[test]
    fn a_control_character_in_the_supplied_name_is_escaped() {
        let s = setup();
        let result = review(&s, &request(&s, "evil\u{202E}net", native(1).encode()));
        assert!(alerted(&result, "evil\\u{202e}net"));
    }

    #[test]
    fn a_chain_id_nobody_knows_shows_the_supplied_name_and_the_number() {
        let s = setup();
        let payload = changed(|tx| tx.chain_id = 31337);
        let req = request(&s, "local-debug", payload);
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(result.network, "local-debug");
        assert!(result.mainnet);
        assert_eq!(field(&result, "chain_id").value, "31337");
        assert!(alerted(&result, "Chain ID 31337"));
        assert!(alerted(&result, "local-debug"));
        assert_eq!(field(&result, "value").value, "1500000000000000000");
        assert_eq!(result.amount.as_ref().unwrap().symbol, None);
        let (response, _) = sign(&s, &req).unwrap();
        assert!(tx::verify(&req, &response, &account_key(&s)).is_ok());
    }

    #[test]
    fn a_known_network_with_an_unknown_chain_id_is_a_contradiction() {
        let s = setup();
        let payload = changed(|tx| tx.chain_id = 31337);
        let req = request(&s, "ethereum", payload);
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ChainIdMismatch));
    }

    #[test]
    fn an_unknown_name_needs_the_chain_field_when_nothing_else_says_evm() {
        let s = setup();
        let mut req = request(&s, "local-debug", vec![7; 32]);
        req.paths = vec!["m/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::UnknownNetwork));
        req.chain = Some(1);
        assert_eq!(review(&s, &req).confirm, Confirm::HoldConfirm);
    }

    #[test]
    fn a_whole_transaction_routes_to_evm_on_any_path() {
        let s = setup();
        let mut req = request(&s, "local-debug", native(1).encode());
        req.paths = vec!["m/0".into()];
        assert_eq!(review(&s, &req).network, "ethereum");
    }

    #[test]
    fn a_path_outside_the_export_signs_with_a_warning_and_no_address() {
        let s = setup();
        for path in ["m/44'/60'/0'/0/1", "m/44'/60'/7'/0/0", "m/44'/966'/0'/0/0", "m/0"] {
            let mut req = request(&s, "ethereum", native(1).encode());
            req.paths = vec![path.into()];
            let result = review(&s, &req);
            assert_eq!(result.confirm, Confirm::Hold, "{path}");
            assert_eq!(field(&result, "derivation_path").value, path);
            assert!(alerted(&result, path), "{path}");
            assert!(result.fields.iter().all(|row| row.name != "signer"), "{path}");
            assert!(sign(&s, &req).is_ok(), "{path}");
        }
    }

    #[test]
    fn a_different_path_signs_with_a_different_key() {
        let s = setup();
        let mut other = request(&s, "ethereum", native(1).encode());
        other.paths = vec!["m/44'/60'/0'/0/1".into()];
        let req = request(&s, "ethereum", native(1).encode());
        let (one, _) = sign(&s, &req).unwrap();
        let (two, _) = sign(&s, &other).unwrap();
        assert_ne!(one.signatures, two.signatures);
    }

    #[test]
    fn the_exported_path_has_no_path_warning() {
        let s = setup();
        assert!(
            all_account_specs(false)
                .iter()
                .any(|spec| spec.chain == Chain::Evm && spec.path == EVM_PATH)
        );
        let result = review(&s, &request(&s, "ethereum", native(1).encode()));
        assert_eq!((result.risk, result.confirm), (Risk::Transfer, Confirm::Tap));
        assert!(result.alerts.is_empty());
    }

    /// 这些私钥上有真实资产,裸哈希能签下它们的交易,所以这是矛盾,不是不认识.
    #[test]
    fn the_mainnet_keys_of_other_chains_stay_refused_for_a_raw_hash() {
        let s = setup();
        for path in ["m/84'/0'/0'/0/0", "m/44'/195'/0'/0/0", "m/44'/501'/0'/0'"] {
            let mut req = request(&s, "ethereum", vec![7; 32]);
            req.kind = KIND_RAW_HASH;
            req.paths = vec![path.into()];
            assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch), "{path}");
        }
    }

    /// 网络名是请求写的,改个名字不能绕过这项检查.
    #[test]
    fn an_unknown_network_name_cannot_reach_another_chains_key() {
        let s = setup();
        let mut req = request(&s, "made-up", vec![7; 32]);
        req.kind = KIND_RAW_HASH;
        req.chain = Some(1);
        req.paths = vec!["m/84'/0'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
    }

    #[test]
    fn two_paths_are_refused() {
        let s = setup();
        let mut req = request(&s, "ethereum", native(1).encode());
        req.paths = vec![EVM_PATH.into(), EVM_PATH.into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
    }

    #[test]
    fn a_path_deeper_than_bip32_allows_is_refused() {
        let s = setup();
        let mut req = request(&s, "ethereum", native(1).encode());
        req.paths = vec![format!("m{}", "/0".repeat(256))];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
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
            Some(Forbid::ParseFailed)
        );
    }

    #[test]
    fn fingerprint_and_path_checks_come_first() {
        let s = setup();
        let mut req = request(&s, "ethereum", native(1).encode());
        req.fingerprint = [0; 4];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::FingerprintMismatch));

        let mut req = request(&s, "ethereum", native(1).encode());
        req.paths = vec!["m/84'/0'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));

        req.paths = vec![];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
    }

    #[test]
    fn high_fee_needs_a_long_press() {
        let s = setup();
        let payload = changed(|tx| {
            tx.gas_limit = 10_000_000;
            tx.max_priority_fee_per_gas = 1;
            tx.max_fee_per_gas = 100_000_000_000;
        });
        let review = review(&s, &request(&s, "ethereum", payload));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
    }

    #[test]
    fn unknown_contract_shows_raw_data() {
        let s = setup();
        let data = unhex("deadbeef000000000000000000000000000000000000000000000000000000000000002a");
        let payload = changed(|tx| tx.input = data.clone().into());
        let review = review(&s, &request(&s, "ethereum", payload));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(review.raw.len(), 2);
        assert_eq!(field(&review, "data").value, hex(&data));
        assert_eq!(field(&review, "value").value, "1.5 ETH");
        assert_eq!(field(&review, "data").risk, Some(Risk::Unknown));
    }

    #[test]
    fn contract_creation_is_unknown() {
        let s = setup();
        let payload = changed(|tx| {
            tx.to = TxKind::Create;
            tx.input = vec![0x60].into();
        });
        assert_eq!(review(&s, &request(&s, "ethereum", payload)).risk, Risk::Unknown);
    }

    #[test]
    fn contract_creation_shows_its_value() {
        let s = setup();
        for data in [Vec::new(), vec![0x60]] {
            let payload = changed(|tx| {
                tx.to = TxKind::Create;
                tx.input = data.clone().into();
            });
            let result = review(&s, &request(&s, "ethereum", payload));
            assert_eq!(result.risk, Risk::Unknown);
            assert_eq!(result.amount.as_ref().unwrap().text, "1.5");
            assert_eq!(field(&result, "value").value, "1.5 ETH");
        }
        let free = changed(|tx| {
            tx.to = TxKind::Create;
            tx.value = U256::ZERO;
        });
        assert_eq!(review(&s, &request(&s, "ethereum", free)).amount, None);
    }

    /// 本钱包给 `delegate` 的授权,放在一笔发给 `PEER` 的交易里.
    fn delegation(s: &Setup, chain_id: u64, delegate: Address) -> TxEip7702 {
        let inner = Authorization {
            chain_id: U256::from(chain_id),
            address: delegate,
            nonce: 0,
        };
        let raw = s
            .wallet
            .sign_recoverable(EVM_PATH, inner.signature_hash().0, bitone_core::Mode::Mainnet)
            .unwrap();
        let signature = Signature::from_raw(&raw).unwrap();
        let authorization: SignedAuthorization = inner.into_signed(signature);
        TxEip7702 {
            chain_id: 1,
            nonce: 1,
            gas_limit: 50_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 2_000_000_000,
            to: peer(),
            value: U256::from(1_500_000_000_000_000_000u64),
            access_list: AccessList::default(),
            authorization_list: vec![authorization],
            input: Bytes::new(),
        }
    }

    fn with_authorization(s: &Setup, chain_id: u64) -> Vec<u8> {
        UnsignedTx::new(delegation(s, chain_id, peer())).encode()
    }

    #[test]
    fn eip7702_delegation_is_a_permission_change() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", with_authorization(&s, 1)));
        assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
        assert_eq!(review.counterparty_label.as_deref(), Some("新控制方"));
        assert_eq!(field(&review, "authority").value, field(&review, "signer").value);
        assert!(field(&review, "authority").own);
    }

    #[test]
    fn clearing_a_delegation_is_still_a_permission_change() {
        let s = setup();
        let payload = UnsignedTx::new(delegation(&s, 1, Address::ZERO)).encode();
        let review = review(&s, &request(&s, "ethereum", payload));
        assert_eq!(review.risk, Risk::PermissionChange);
        assert!(
            review
                .fields
                .iter()
                .any(|row| row.note.as_deref() == Some("地址为零,这是清除委托"))
        );
    }

    /// 委托的同时转账时,页面顶部是新控制方,收款方不能因此消失.
    #[test]
    fn eip7702_keeps_the_outer_transfer_visible() {
        let s = setup();
        let review = review(&s, &request(&s, "ethereum", with_authorization(&s, 1)));
        let shown = |value: &str| review.actions.iter().any(|a| a.rows.iter().any(|r| r.value == value));
        assert!(shown(PEER));
        assert!(shown("1.5 ETH"));
    }

    #[test]
    fn a_universal_delegation_is_unknown_and_says_every_chain() {
        let s = setup();
        let req = request(&s, "ethereum", with_authorization(&s, 0));
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "每条 EVM 链上都生效"));
        assert!(sign(&s, &req).is_ok());
    }

    #[test]
    fn a_delegation_for_another_chain_is_shown_not_refused() {
        let s = setup();
        let req = request(&s, "ethereum", with_authorization(&s, 137));
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "这里只显示,不拒绝"));
        assert!(result.fields.iter().any(|row| row.value == "137"));
        assert!(sign(&s, &req).is_ok());
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

    /// 第 5.1 节:这个签名在别的链上有效,所以只告警,不拒绝.
    #[test]
    fn permit_for_another_chain_is_unknown_with_an_alert() {
        let s = setup();
        let req = typed(&s, "Permit", 137.into());
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "chainId 137"));
        assert!(result.amount.unwrap().unlimited);
        assert_eq!(result.counterparty.as_deref(), Some(PEER));
        assert!(sign(&s, &req).is_ok());
    }

    /// 域说别的链时不能确认是测试网,测试网的名字也不能去掉主网提示.
    #[test]
    fn a_chain_that_differs_keeps_the_mainnet_prompt() {
        let s = setup();
        for (network, chain_id) in [("sepolia", 1), ("ethereum", 11155111)] {
            let mut req = typed(&s, "Permit", chain_id.into());
            req.network = network.into();
            assert!(review(&s, &req).mainnet, "{network}");
        }
        let mut req = typed(&s, "Permit", 11155111.into());
        req.network = "sepolia".into();
        assert!(!review(&s, &req).mainnet);
    }

    #[test]
    fn other_typed_messages_decode_and_sign_as_unknown() {
        let s = setup();
        let req = typed(&s, "Order", 1.into());
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&result, "EIP-712"));
        for name in ["domain", "types", "message", "primary_type", "signing_hash"] {
            assert!(result.fields.iter().any(|row| row.name == name), "{name}");
        }
        assert_eq!(field(&result, "spender").value, PEER);
        assert_eq!(field(&result, "name").value, "Tether");
        assert_eq!(result.field_count as usize, result.fields.len());
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        assert!(verified.signature_only);
        assert_eq!(verified.raw.len(), 65);
    }

    /// 屏幕上的 `signing_hash` 就是实际签名的摘要.
    #[test]
    fn the_signed_digest_is_the_one_on_the_screen() {
        let s = setup();
        let req = typed(&s, "Order", 1.into());
        let result = review(&s, &req);
        let (response, _) = sign(&s, &req).unwrap();
        let verified = tx::verify(&req, &response, &account_key(&s)).unwrap();
        assert_eq!(verified.hash, format!("0x{}", field(&result, "signing_hash").value));
    }

    /// 自引用类型的值是有限的,嵌套超过解码树的上限时才拒绝.
    #[test]
    fn nesting_past_the_limit_is_refused() {
        let s = setup();
        let nested = |depth: usize| {
            let mut value = serde_json::json!({"children": []});
            for _ in 0..depth {
                value = serde_json::json!({"children": [value]});
            }
            serde_json::json!({
                "types": {"EIP712Domain": [], "Node": [{"name": "children", "type": "Node[]"}]},
                "primaryType": "Node",
                "domain": {},
                "message": value
            })
        };
        assert_eq!(review(&s, &typed_request(&s, &nested(3))).risk, Risk::Unknown);
        assert_eq!(
            forbidden(review(&s, &typed_request(&s, &nested(40)))),
            Some(Forbid::ParseFailed)
        );
    }

    #[test]
    fn typed_data_signs_on_a_network_nobody_knows() {
        let s = setup();
        let mut req = typed(&s, "Order", 31337.into());
        req.network = "local-debug".into();
        let result = review(&s, &req);
        assert_eq!(result.risk, Risk::Unknown);
        assert!(result.mainnet);
        assert!(alerted(&result, "Chain ID 31337"));
        assert!(sign(&s, &req).is_ok());
    }

    #[test]
    fn a_known_chain_id_names_the_network_for_typed_data_too() {
        let s = setup();
        let mut req = typed(&s, "Permit", 11155111.into());
        req.network = "made-up".into();
        let result = review(&s, &req);
        assert_eq!(result.network, "sepolia");
        assert!(!result.mainnet);
        assert!(alerted(&result, "made-up"));
    }

    #[test]
    fn a_descriptor_recognizes_a_typed_message() {
        use bitone_core::manifest::{Descriptor, DisplayField, DisplayFormat, DisplayParams};
        let s = setup();
        let mut manifest = Manifest::builtin().clone();
        manifest.descriptors.push(Descriptor {
            network: "ethereum".into(),
            contract: USDT.into(),
            signature: "Order(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)".into(),
            format: DisplayFormat {
                intent: "下单".into(),
                fields: vec![DisplayField {
                    path: "value".into(),
                    label: "下单金额".into(),
                    format: None,
                    params: DisplayParams::default(),
                }],
            },
        });
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: &manifest,
            highest_change: HighestChange::default(),
        };
        let req = typed(&s, "Order", 1.into());
        let result = tx::review(&req, &ctx);
        assert_eq!(result.risk, Risk::Recognized);
        assert!(result.fields.iter().any(|row| row.name == "下单金额"));
        // 绑定的是这条链上的这个合约,别的链上没有这个描述文件.
        let elsewhere = typed(&s, "Order", 137.into());
        assert_eq!(tx::review(&elsewhere, &ctx).risk, Risk::Unknown);
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

    /// DAI 的 Permit 同名不同字段,`allowed` 才是实际授权.类型串不符,所以不走 Permit 的页面,额度不会显示错.
    #[test]
    fn dai_style_permit_is_unknown_and_shows_its_real_fields() {
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
            "holder": PEER, "spender": PEER, "nonce": 0, "expiry": 0, "allowed": true
        });
        let req = typed_request(&s, &data);
        let result = review(&s, &req);
        assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(result.amount, None);
        assert_eq!(field(&result, "allowed").value, "true");
        assert!(sign(&s, &req).is_ok());
    }

    /// `value` 不进哈希时曾经显示额度 1,实际签下无限授权.这种成员是矛盾,拒绝.
    #[test]
    fn a_hidden_member_in_a_dai_style_permit_is_refused() {
        let s = setup();
        let mut data = typed_data("Permit", 1.into());
        data["types"]["Permit"].as_array_mut().unwrap().remove(2);
        assert_eq!(
            forbidden(review(&s, &typed_request(&s, &data))),
            Some(Forbid::ParseFailed)
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

    /// 域不绑定链或合约时,签名在别处也可能有效.第 5.1 节:可以签,告警要说清后果.
    #[test]
    fn an_unbound_domain_is_unknown_and_says_what_it_means() {
        let s = setup();
        let without = |field: &str| {
            let mut data = typed_data("Permit", 1.into());
            data["types"]["EIP712Domain"]
                .as_array_mut()
                .unwrap()
                .retain(|f| f["name"] != field);
            data["domain"].as_object_mut().unwrap().remove(field);
            data
        };
        for (name, text) in [("chainId", "不绑定链"), ("verifyingContract", "不绑定合约")] {
            let req = typed_request(&s, &without(name));
            let result = review(&s, &req);
            assert_eq!((result.risk, result.confirm), (Risk::Unknown, Confirm::Hold), "{name}");
            assert!(alerted(&result, text), "{name}");
            assert_eq!(field(&result, name).value, "无");
            assert!(result.mainnet);
            assert!(sign(&s, &req).is_ok());
        }
    }

    /// 类型里没有而 JSON 里有的成员不进哈希,页面上的值和签名无关,这是矛盾.
    #[test]
    fn a_domain_member_outside_its_type_is_refused() {
        let s = setup();
        for name in ["chainId", "verifyingContract"] {
            let mut data = typed_data("Permit", 1.into());
            data["types"]["EIP712Domain"]
                .as_array_mut()
                .unwrap()
                .retain(|f| f["name"] != name);
            assert_eq!(
                forbidden(review(&s, &typed_request(&s, &data))),
                Some(Forbid::ParseFailed)
            );
        }
    }

    #[test]
    fn a_chain_id_that_is_not_a_uint_does_not_bind() {
        let s = setup();
        let mut data = typed_data("Permit", "1".into());
        data["types"]["EIP712Domain"][1]["type"] = "string".into();
        let result = review(&s, &typed_request(&s, &data));
        assert_eq!(result.risk, Risk::Unknown);
        assert!(alerted(&result, "不绑定链"));
    }

    #[test]
    fn unknown_request_kinds_are_refused() {
        let s = setup();
        let mut req = request(&s, "ethereum", vec![0; 32]);
        req.kind = 9;
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::Unsupported));
    }

    #[test]
    fn a_path_outside_the_export_can_sign_a_permit() {
        let s = setup();
        let mut req = typed(&s, "Permit", 1.into());
        req.paths = vec!["m/44'/60'/0'/0/1".into()];
        let result = review(&s, &req);
        assert_eq!(result.risk, Risk::Approval);
        assert!(result.rows.iter().all(|row| row.label != "付款账户"));
        assert!(alerted(&result, "m/44'/60'/0'/0/1"));
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

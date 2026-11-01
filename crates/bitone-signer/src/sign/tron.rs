use crate::Error;
use crate::keys::Wallet;
use bitone_core::request::{KIND_TRANSACTION, SignRequest};
use bitone_core::tx::tron;
use bitone_core::{DerivationPath, Mode};

impl Wallet {
    /// 私钥类型只决定扩展私钥的版本前缀,派生结果和它无关.
    pub(crate) fn sign_tron(&self, request: &SignRequest, mode: Mode) -> Result<Vec<u8>, Error> {
        let [path] = request.paths.as_slice() else {
            return Err(Error::Refused);
        };
        DerivationPath::parse(path)?;
        let digest = tron::signing_hash(request).map_err(|_| Error::Refused)?;
        if request.kind == KIND_TRANSACTION {
            // 保存的公钥算不出的路径到这里才有结论.
            let mut xpriv = self.secp_key(path, mode)?;
            let signer = xpriv.private_key.public_key(self.secp()).serialize();
            xpriv.private_key.non_secure_erase();
            tron::check_signer(&request.payload, &signer).map_err(|_| Error::Refused)?;
        }
        self.sign_recoverable(path, digest, mode)
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;
    use crate::keys::Wallet;
    use bitone_core::keys::PublicKeys;
    use bitone_core::manifest::Manifest;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{
        KIND_PERSONAL_SIGN, KIND_RAW_HASH, KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse,
    };
    use bitone_core::review::{Confirm, Forbid, Review, Risk};
    use bitone_core::tx::tron::types::{Any, Contract, ContractType, Raw, TransferContract, TriggerSmartContract};
    use bitone_core::tx::tron::{TransferParams, UnsignedTx, build_transfer};
    use bitone_core::tx::{self, HighestChange, ReviewContext};
    use bitone_core::{DerivationPath, Mode, Network};
    use hex_literal::hex;
    use prost::Message;
    use serde_json::Value;

    const PATH: &str = "m/44'/195'/0'/0/0";
    const NILE_PATH: &str = "m/44'/1'/0'/0/0";
    const OWNER: &str = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";
    const OWNER_BYTES: [u8; 21] = hex!("41c8599111f29c1e1e061265b4af93ea1f274ad78a");
    const PEER: &str = "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE";
    const PEER_BYTES: [u8; 21] = hex!("41a2726afbecbd8e936000ed684cef5e2f5cf43008");
    const USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const USDT_BYTES: [u8; 21] = hex!("41a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    const FAKE: [u8; 21] = hex!("413333333333333333333333333333333333333333");
    const TYPE_URL_PREFIX: &str = "type.googleapis.com/protocol.";
    /// tronweb 生成的向量,41 个合约类型各有一笔,发起方都是 `OWNER`.
    const VECTORS: &str = include_str!("../../../../tools/crosscheck/vectors/tron.json");
    /// tronweb 生成的 TIP-191,TIP-712 向量,签名者是 `OWNER`.
    const MESSAGE_VECTORS: &str = include_str!("../../../../tools/crosscheck/vectors/tron-messages.json");
    const NOW: u64 = 1_800_000_000_000;
    const LATER: u64 = NOW + 1_800_000;

    struct Setup {
        wallet: Wallet,
        keys: PublicKeys,
    }

    fn setup() -> Setup {
        let wallet = Wallet::new(&[0u8; 16], "").unwrap();
        let keys = PublicKeys::decode(&wallet.public_keys(false).unwrap()).unwrap();
        Setup { wallet, keys }
    }

    fn hex_text(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn request(s: &Setup, network: &str, path: &str, payload: Vec<u8>) -> SignRequest {
        SignRequest {
            version: VERSION,
            id: [9; 16],
            fingerprint: s.keys.fingerprint,
            network: network.into(),
            kind: KIND_TRANSACTION,
            paths: vec![path.into()],
            payload,
            hint: None,
            chain: None,
        }
    }

    fn main_request(s: &Setup, payload: Vec<u8>) -> SignRequest {
        request(s, "tron", PATH, payload)
    }

    fn review(s: &Setup, request: &SignRequest) -> Review {
        let ctx = ReviewContext {
            keys: &s.keys,
            manifest: Manifest::builtin(),
            highest_change: HighestChange::default(),
        };
        tx::review(request, &ctx)
    }

    fn sign(s: &Setup, request: &SignRequest) -> (SignResponse, Review) {
        s.wallet
            .sign(request, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap()
    }

    fn public_key(s: &Setup) -> [u8; 33] {
        s.keys.secp256k1(&DerivationPath::parse(PATH).unwrap()).unwrap()
    }

    fn forbidden(review: Review) -> Option<Forbid> {
        assert_eq!(review.confirm, Confirm::Refuse);
        review.forbidden
    }

    fn field<'a>(review: &'a Review, name: &str) -> &'a str {
        review
            .fields
            .iter()
            .find(|row| row.name == name)
            .map_or("", |row| row.value.as_str())
    }

    fn alerted(review: &Review, wanted: &str) -> bool {
        review.alerts.iter().any(|alert| alert.contains(wanted))
    }

    fn action_has(review: &Review, label: &str, wanted: &str) -> bool {
        review
            .actions
            .iter()
            .flat_map(|a| &a.rows)
            .any(|r| r.label == label && r.value.contains(wanted))
    }

    fn params<'a>(amount: &'a str, contract: Option<&'a str>, fee_limit: u64) -> TransferParams<'a> {
        TransferParams {
            network: Network::Tron,
            from: OWNER,
            to: PEER,
            amount,
            contract,
            ref_block_bytes: hex!("abcd"),
            ref_block_hash: hex!("0102030405060708"),
            expiration_ms: LATER,
            timestamp_ms: NOW,
            fee_limit,
        }
    }

    fn native() -> Vec<u8> {
        build_transfer(&params("1500000", None, 0)).unwrap()
    }

    fn token() -> Vec<u8> {
        build_transfer(&params("1250000", Some(USDT), 100_000_000)).unwrap()
    }

    fn wrap(kind: ContractType, value: Vec<u8>) -> Raw {
        Raw {
            ref_block_bytes: hex!("abcd").to_vec(),
            ref_block_hash: hex!("0102030405060708").to_vec(),
            expiration: LATER as i64,
            contract: vec![Contract {
                r#type: kind as i32,
                parameter: Some(Any {
                    type_url: [TYPE_URL_PREFIX, kind.as_str_name()].concat(),
                    value,
                }),
                ..Contract::default()
            }],
            timestamp: NOW as i64,
            ..Raw::default()
        }
    }

    fn transfer_from(owner: &[u8]) -> Raw {
        let value = TransferContract {
            owner_address: owner.to_vec(),
            to_address: PEER_BYTES.to_vec(),
            amount: 5,
        };
        wrap(ContractType::TransferContract, value.encode_to_vec())
    }

    fn trigger(contract: [u8; 21], call_value: i64, data: Vec<u8>) -> Raw {
        let value = TriggerSmartContract {
            owner_address: OWNER_BYTES.to_vec(),
            contract_address: contract.to_vec(),
            call_value,
            data,
            ..TriggerSmartContract::default()
        };
        wrap(ContractType::TriggerSmartContract, value.encode_to_vec())
    }

    fn word(tail: &[u8]) -> Vec<u8> {
        let mut out = vec![0; 32 - tail.len()];
        out.extend_from_slice(tail);
        out
    }

    fn abi(selector: [u8; 4], words: &[Vec<u8>]) -> Vec<u8> {
        let mut data = selector.to_vec();
        words.iter().for_each(|w| data.extend_from_slice(w));
        data
    }

    const TRANSFER: [u8; 4] = hex!("a9059cbb");
    const APPROVE: [u8; 4] = hex!("095ea7b3");

    // 下面的期望值由 tronweb 6.5.1 用同一助记词算出:transactionBuilder.sendTrx,triggerSmartContract
    // 生成 raw_data_hex 和 txID,utils.crypto.signTransaction 生成签名,是独立实现.
    // tronweb 的 v 是 27 或 28,协议里的恢复位是 0 或 1,节点两种都接受,r 和 s 必须完全相同.
    const TRONWEB_NATIVE_RAW: &str = "0a02abcd2208010203040506070840c08edfc3b1345a67080112630a2d747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e73666572436f6e747261637412320a1541c8599111f29c1e1e061265b4af93ea1f274ad78a121541a2726afbecbd8e936000ed684cef5e2f5cf4300818e0c65b7080a0f1c2b134";
    const TRONWEB_NATIVE_TXID: &str = "511d43288922077444e1a92eceedd31d83a8e0a1c7d76bfb4bff5e37c5437997";
    const TRONWEB_NATIVE_SIGNATURE: &str = "eeb5513427c675e071ef2d09a7eda39eab7624f0d2907e17062770b01f1d20ba7ca63d7d20be7033827efd79afaebc6b98c288048140b204ab8cac553a54694f1b";
    const TRONWEB_TOKEN_RAW: &str = "0a02abcd2208010203040506070840c08edfc3b1345aae01081f12a9010a31747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e54726967676572536d617274436f6e747261637412740a1541c8599111f29c1e1e061265b4af93ea1f274ad78a121541a614f803b6fd780986a42c78ec9c7f77e6ded13c2244a9059cbb000000000000000000000000a2726afbecbd8e936000ed684cef5e2f5cf4300800000000000000000000000000000000000000000000000000000000001312d07080a0f1c2b134900180c2d72f";
    const TRONWEB_TOKEN_TXID: &str = "2abc61a85cd39ef02a558748ffbf88655f9e449d7aab7aa50f112f947c2a36d1";
    const TRONWEB_TOKEN_SIGNATURE: &str = "ba9ed57603ec6a1944b5907d0423766d530913ec1acfe9cb39dbaa727665859e660a999bb912f994d7f17ba78024f47a0ce27e8ebba7b953a0ee8dd859ab883b1c";

    fn check_against_tronweb(s: &Setup, payload: Vec<u8>, txid: &str, signature: &str) {
        let req = main_request(s, payload.clone());
        let (response, _) = sign(s, &req);
        let produced = response.signatures[0].to_vec();
        let expected = unhex(signature);
        assert_eq!(hex_text(&produced[..64]), hex_text(&expected[..64]));
        assert_eq!(produced[64] + 27, expected[64]);
        let verified = tx::verify(&req, &response, &public_key(s)).unwrap();
        assert_eq!(verified.hash, txid);
        // Transaction:字段 1 是 raw_data,字段 2 是签名.
        let mut raw = vec![0x0a];
        let mut len = payload.len();
        while len >= 0x80 {
            raw.push(len as u8 | 0x80);
            len >>= 7;
        }
        raw.push(len as u8);
        raw.extend_from_slice(&payload);
        raw.extend_from_slice(&[0x12, 65]);
        raw.extend_from_slice(&produced);
        assert_eq!(verified.raw, raw);
    }

    #[test]
    fn transfers_match_tronweb() {
        let s = setup();
        for (payload, raw, txid, signature) in [
            (
                native(),
                TRONWEB_NATIVE_RAW,
                TRONWEB_NATIVE_TXID,
                TRONWEB_NATIVE_SIGNATURE,
            ),
            (token(), TRONWEB_TOKEN_RAW, TRONWEB_TOKEN_TXID, TRONWEB_TOKEN_SIGNATURE),
        ] {
            assert_eq!(hex_text(&payload), raw);
            check_against_tronweb(&s, payload, txid, signature);
        }
    }

    #[test]
    fn plain_native_transfer() {
        let s = setup();
        let review = review(&s, &main_request(&s, native()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.5", Some("TRX")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(field(&review, "network"), "tron");
        assert_eq!(field(&review, "signer"), OWNER);
        assert_eq!(field(&review, "derivation_path"), PATH);
        assert_eq!(field(&review, "expiration"), "2027-01-15T08:30:00Z");
        assert_eq!(field(&review, "timestamp"), "2027-01-15T08:00:00Z");
        assert_eq!(field(&review, "type"), "TransferContract");
        assert_eq!(field(&review, "amount"), "1.5 TRX");
        assert_eq!(field(&review, "to_address"), PEER);
        assert!(review.alerts.is_empty());
        assert_eq!(review.field_count as usize, review.fields.len());
        assert_eq!(review.rows[0].value, OWNER);
        let owner = review.fields.iter().find(|row| row.name == "owner_address").unwrap();
        assert!(owner.own);
    }

    #[test]
    fn plain_token_transfer() {
        let s = setup();
        let review = review(&s, &main_request(&s, token()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.25", Some("USDT")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(field(&review, "type"), "TriggerSmartContract");
        assert_eq!(field(&review, "fee_limit"), "100 TRX");
        assert!(action_has(&review, "调用方法", "transfer"));
        assert!(action_has(&review, "合约", USDT));
        assert!(action_has(&review, "合约清单", "在内置清单"));
        assert_eq!(review.raw[0].note.as_deref(), Some("方法选择器"));
        assert_eq!(review.raw.len(), 3);
    }

    #[test]
    fn token_outside_the_manifest_is_not_a_plain_transfer() {
        let s = setup();
        let data = abi(TRANSFER, &[word(&PEER_BYTES[1..]), word(&[0x12, 0xd6, 0x90])]);
        let review = review(&s, &main_request(&s, trigger(FAKE, 0, data).encode_to_vec()));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1234576", None));
    }

    #[test]
    fn trc20_approve_is_an_approval() {
        let s = setup();
        let data = abi(APPROVE, &[word(&PEER_BYTES[1..]), vec![0xff; 32]]);
        let review = review(&s, &main_request(&s, trigger(USDT_BYTES, 0, data).encode_to_vec()));
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
        assert!(review.amount.unwrap().unlimited);
        assert_eq!(review.counterparty_label.as_deref(), Some("被授权方"));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
    }

    #[test]
    fn an_unlisted_trc20_approve_shows_the_spender_in_base58() {
        let s = setup();
        let data = abi(APPROVE, &[word(&PEER_BYTES[1..]), vec![0xff; 32]]);
        let reviewed = review(&s, &main_request(&s, trigger(FAKE, 0, data).encode_to_vec()));
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&reviewed, "Spender"), PEER);
        assert_eq!(reviewed.counterparty.as_deref(), Some(PEER));
        assert!(alerted(&reviewed, "不在内置清单"));
        assert!(alerted(&reviewed, "额度是无限"));
        assert!(reviewed.amount.unwrap().unlimited);
    }

    #[test]
    fn call_value_makes_a_transfer_unknown_and_is_shown() {
        let s = setup();
        let data = abi(TRANSFER, &[word(&PEER_BYTES[1..]), word(&[1])]);
        let review = review(
            &s,
            &main_request(&s, trigger(USDT_BYTES, 2_000_000, data).encode_to_vec()),
        );
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&review, "call_value"), "2 TRX");
        assert!(alerted(&review, "2 TRX"));
    }

    #[test]
    fn unknown_method_shows_the_data() {
        let s = setup();
        let unknown = review(
            &s,
            &main_request(&s, trigger(FAKE, 0, hex!("deadbeef").to_vec()).encode_to_vec()),
        );
        assert_eq!(unknown.risk, Risk::Unknown);
        assert_eq!(unknown.raw[0].text, "deadbeef");
        let empty = review(
            &s,
            &main_request(&s, trigger(USDT_BYTES, 1, Vec::new()).encode_to_vec()),
        );
        assert_eq!(empty.risk, Risk::Unknown);
    }

    #[test]
    fn replaced_owner_address_is_refused() {
        let s = setup();
        let stranger = PEER_BYTES;
        let transfer = transfer_from(&stranger).encode_to_vec();
        assert_eq!(
            forbidden(review(&s, &main_request(&s, transfer.clone()))),
            Some(Forbid::AddressMismatch)
        );
        let call = TriggerSmartContract {
            owner_address: stranger.to_vec(),
            contract_address: USDT_BYTES.to_vec(),
            data: vec![1],
            ..TriggerSmartContract::default()
        };
        let trigger = wrap(ContractType::TriggerSmartContract, call.encode_to_vec()).encode_to_vec();
        assert_eq!(
            forbidden(review(&s, &main_request(&s, trigger))),
            Some(Forbid::AddressMismatch)
        );
        let asset = bitone_core::tx::tron::types::TransferAssetContract {
            asset_name: b"1002000".to_vec(),
            owner_address: stranger.to_vec(),
            to_address: OWNER_BYTES.to_vec(),
            amount: 1,
        };
        let asset = wrap(ContractType::TransferAssetContract, asset.encode_to_vec()).encode_to_vec();
        assert_eq!(
            forbidden(review(&s, &main_request(&s, asset))),
            Some(Forbid::AddressMismatch)
        );
        let result = s.wallet.sign(
            &main_request(&s, transfer),
            &s.keys,
            Manifest::builtin(),
            HighestChange::default(),
        );
        assert_eq!(result.unwrap_err(), Error::Refused);
    }

    #[test]
    fn path_rules() {
        let s = setup();
        let mut req = main_request(&s, native());
        req.paths = vec!["m/44'/60'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec!["m/84'/0'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![NILE_PATH.into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
        let nile = request(&s, "tron-nile", PATH, native());
        assert_eq!(forbidden(review(&s, &nile)), Some(Forbid::PathNetworkMismatch));
    }

    fn address_at(s: &Setup, path: &str) -> [u8; 21] {
        let key = s.wallet.secp_key(path, Mode::Mainnet).unwrap();
        let public = key.private_key.public_key(s.wallet.secp()).serialize();
        let text = bitone_core::address::tron_address(&public).unwrap();
        let bytes = bitone_core::address::parse_tron_address(&text).unwrap();
        [&[0x41], bytes.as_slice()].concat().try_into().unwrap()
    }

    #[test]
    fn an_unexported_path_signs_after_the_owner_is_rechecked() {
        let s = setup();
        const OTHER: &str = "m/44'/195'/0'/0/1";
        let own = address_at(&s, OTHER);
        let req = request(&s, "tron", OTHER, transfer_from(&own).encode_to_vec());
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Transfer, Confirm::Hold));
        assert!(alerted(&reviewed, OTHER));
        assert_eq!(field(&reviewed, "signer"), "");
        let (response, _) = sign(&s, &req);
        let key = s.wallet.secp_key(OTHER, Mode::Mainnet).unwrap();
        let public = key.private_key.public_key(s.wallet.secp()).serialize();
        tx::verify(&req, &response, &public).unwrap();

        // 核对结果到解锁之后才知道,这条路径算不出 `owner_address`.
        let wrong = request(&s, "tron", OTHER, transfer_from(&PEER_BYTES).encode_to_vec());
        assert!(review(&s, &wrong).is_signable());
        let result = s
            .wallet
            .sign(&wrong, &s.keys, Manifest::builtin(), HighestChange::default());
        assert_eq!(result.unwrap_err(), Error::Refused);
    }

    #[test]
    fn an_unexported_path_with_a_permission_id_is_unknown() {
        let s = setup();
        let mut tx = transfer_from(&PEER_BYTES);
        tx.contract[0].permission_id = 2;
        let req = request(&s, "tron", "m/44'/195'/0'/0/1", tx.encode_to_vec());
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&reviewed, PEER));
        sign(&s, &req);
    }

    /// 第 1.1 节:`owner_address` 是别人时,`Permission_id` 不为 0 就是 TRON 的多签.
    #[test]
    fn a_foreign_owner_signs_only_with_a_permission_id() {
        let s = setup();
        let mut tx = transfer_from(&PEER_BYTES);
        tx.contract[0].permission_id = 2;
        let req = main_request(&s, tx.encode_to_vec());
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert!(alerted(&reviewed, PEER));
        assert!(alerted(&reviewed, "权限 2"));
        let (response, _) = sign(&s, &req);
        let verified = tx::verify(&req, &response, &public_key(&s)).unwrap();
        assert_eq!(verified.hash.len(), 64);
    }

    #[test]
    fn permission_id_is_decoded_and_shown() {
        let s = setup();
        let mut tx = wrap_native();
        tx.contract[0].permission_id = 3;
        let reviewed = review(&s, &main_request(&s, tx.encode_to_vec()));
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(field(&reviewed, "Permission_id"), "3");
        assert!(alerted(&reviewed, "权限 3"));
    }

    fn wrap_native() -> Raw {
        transfer_from(&OWNER_BYTES)
    }

    #[test]
    fn a_memo_signs_with_an_alert() {
        let s = setup();
        let mut tx = wrap_native();
        tx.data = "备注".as_bytes().to_vec();
        let req = main_request(&s, tx.encode_to_vec());
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(field(&reviewed, "data"), "备注");
        assert!(alerted(&reviewed, "额外扣费"));
        sign(&s, &req);
    }

    #[test]
    fn call_token_value_is_decoded_and_shown() {
        let s = setup();
        let data = abi(TRANSFER, &[word(&PEER_BYTES[1..]), word(&[1])]);
        let call = TriggerSmartContract {
            owner_address: OWNER_BYTES.to_vec(),
            contract_address: USDT_BYTES.to_vec(),
            data,
            call_token_value: 7,
            token_id: 1_000_001,
            ..TriggerSmartContract::default()
        };
        let tx = wrap(ContractType::TriggerSmartContract, call.encode_to_vec());
        let reviewed = review(&s, &main_request(&s, tx.encode_to_vec()));
        // 附带了 TRC-10 的 `transfer` 不是普通转账.
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&reviewed, "call_token_value"), "7");
        assert_eq!(field(&reviewed, "token_id"), "1000001");
        assert!(alerted(&reviewed, "TRC-10"));
    }

    #[test]
    fn an_unknown_contract_type_signs_after_the_second_confirmation() {
        let s = setup();
        let mut tx = wrap_native();
        tx.contract[0].r#type = 99;
        tx.contract[0].parameter.as_mut().unwrap().type_url = [TYPE_URL_PREFIX, "FutureContract"].concat();
        let req = main_request(&s, tx.encode_to_vec());
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::HoldConfirm));
        assert!(reviewed.is_signable());
        assert_eq!(field(&reviewed, "type"), "99");
        let (response, _) = sign(&s, &req);
        tx::verify(&req, &response, &public_key(&s)).unwrap();
    }

    #[test]
    fn unknown_fields_sign_after_a_long_press() {
        let s = setup();
        let mut payload = native();
        payload.extend_from_slice(&[0xd8, 0x04, 0x05]); // 字段 75,varint.
        let req = main_request(&s, payload);
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&reviewed, "field_75"), "05");
        let (response, _) = sign(&s, &req);
        let verified = tx::verify(&req, &response, &public_key(&s)).unwrap();
        // 未知字段也在签名原文里,装回去时不能丢.
        assert!(hex_text(&verified.raw).contains("d80405"));
    }

    /// 第 5.2 节:离线设备没有时钟,节点比较的是区块时间,所以这里只能告警.
    #[test]
    fn an_odd_expiration_is_an_alert_not_a_refusal() {
        let s = setup();
        for expiration in [1, NOW as i64, NOW as i64 + 86_400_001, i64::MAX] {
            let mut tx = wrap_native();
            tx.expiration = expiration;
            let req = main_request(&s, tx.encode_to_vec());
            let reviewed = review(&s, &req);
            assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Transfer, Confirm::Hold));
            assert!(alerted(&reviewed, "过期时间"));
            sign(&s, &req);
        }
    }

    #[test]
    fn a_negative_amount_is_unknown() {
        let s = setup();
        let value = TransferContract {
            owner_address: OWNER_BYTES.to_vec(),
            to_address: PEER_BYTES.to_vec(),
            amount: -1,
        };
        let tx = wrap(ContractType::TransferContract, value.encode_to_vec());
        let reviewed = review(&s, &main_request(&s, tx.encode_to_vec()));
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&reviewed, "amount"), "-1");
    }

    fn foreign_request(s: &Setup, chain: Option<u8>, path: &str, payload: Vec<u8>) -> SignRequest {
        SignRequest {
            chain,
            ..request(s, "future-tron", path, payload)
        }
    }

    #[test]
    fn an_unknown_network_only_warns() {
        let s = setup();
        for (chain, payload) in [(Some(2), native()), (None, native())] {
            let req = foreign_request(&s, chain, PATH, payload);
            let reviewed = review(&s, &req);
            assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
            assert!(reviewed.mainnet);
            assert_eq!(field(&reviewed, "amount"), "1500000");
            assert_eq!(reviewed.network, "future-tron");
            let (response, _) = sign(&s, &req);
            tx::verify(&req, &response, &public_key(&s)).unwrap();
        }
    }

    #[test]
    fn coin_type_one_needs_the_payload_or_the_chain_field() {
        let s = setup();
        let garbage = vec![9, 9];
        assert_eq!(
            forbidden(review(&s, &foreign_request(&s, None, NILE_PATH, garbage.clone()))),
            Some(Forbid::UnknownNetwork)
        );
        assert_eq!(
            forbidden(review(&s, &foreign_request(&s, Some(2), NILE_PATH, garbage))),
            Some(Forbid::ParseFailed)
        );
        let reviewed = review(&s, &foreign_request(&s, None, NILE_PATH, native()));
        // 这个载荷的发起方是主网账户,这条路径派生的是测试币账户.
        assert_eq!(forbidden(reviewed), Some(Forbid::AddressMismatch));
    }

    /// 第 1.1 节:这些私钥上有真实资产,改个网络名不能绕过.
    #[test]
    fn an_unknown_network_cannot_reach_the_keys_of_other_chains() {
        let s = setup();
        for path in [
            "m/84'/0'/0'/0/0",
            "m/44'/60'/0'/0/0",
            "m/44'/0'/0'/0/0",
            "m/86'/0'/1'/0/0",
        ] {
            let req = foreign_request(&s, Some(2), path, native());
            assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch), "{path}");
        }
    }

    #[test]
    fn nile_uses_its_own_key() {
        let s = setup();
        const NILE_OWNER: &str = "TS8utE69gMxZo48E2fhTBNtkWBsEPZWLGc";
        let mut p = params("1", None, 0);
        p.network = Network::TronNile;
        // 主网地址在 Nile 路径下对不上.
        let req = request(&s, "tron-nile", NILE_PATH, build_transfer(&p).unwrap());
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::AddressMismatch));
        p.from = NILE_OWNER;
        let req = request(&s, "tron-nile", NILE_PATH, build_transfer(&p).unwrap());
        let reviewed = review(&s, &req);
        assert_eq!(reviewed.risk, Risk::Transfer);
        assert_eq!(field(&reviewed, "network"), "tron-nile");
        assert!(!reviewed.mainnet);
        let (response, _) = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap();
        let key = s.keys.secp256k1(&DerivationPath::parse(NILE_PATH).unwrap()).unwrap();
        tx::verify(&req, &response, &key).unwrap();
    }

    #[test]
    fn fee_limit_over_the_ceiling_needs_a_long_press() {
        let s = setup();
        let at_ceiling = review(
            &s,
            &main_request(&s, build_transfer(&params("1", Some(USDT), 200_000_000)).unwrap()),
        );
        assert_eq!((at_ceiling.risk, at_ceiling.confirm), (Risk::Transfer, Confirm::Tap));
        let over = review(
            &s,
            &main_request(&s, build_transfer(&params("1", Some(USDT), 200_000_001)).unwrap()),
        );
        assert_eq!((over.risk, over.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(over.alerts.len(), 1);
    }

    /// tronweb 的 txJsonToPb 编码:owner 和 active 都换成 PEER.
    const TRONWEB_PERMISSION: &str = "0a02abcd2208010203040506070840c08edfc3b1345ad001082e12cb010a3c747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e4163636f756e745065726d697373696f6e557064617465436f6e7472616374128a010a1541c8599111f29c1e1e061265b4af93ea1f274ad78a12241a056f776e657220013a190a1541a2726afbecbd8e936000ed684cef5e2f5cf430081001224b080210021a06616374697665200132207fff1fc0033e00000000000000000000000000000000000000000000000000003a190a1541a2726afbecbd8e936000ed684cef5e2f5cf4300810017080a0f1c2b134";
    /// 第一个地址还是自己,第二个地址和 active 权限都是 PEER.
    const TRONWEB_PERMISSION_HIDDEN: &str = "0a02abcd2208010203040506070840c08edfc3b1345aeb01082e12e6010a3c747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e4163636f756e745065726d697373696f6e557064617465436f6e747261637412a5010a1541c8599111f29c1e1e061265b4af93ea1f274ad78a123f1a056f776e657220013a190a1541c8599111f29c1e1e061265b4af93ea1f274ad78a10013a190a1541a2726afbecbd8e936000ed684cef5e2f5cf430081001224b080210021a06616374697665200132207fff1fc0033e00000000000000000000000000000000000000000000000000003a190a1541a2726afbecbd8e936000ed684cef5e2f5cf4300810017080a0f1c2b134";

    #[test]
    fn account_permission_update_is_a_permission_change() {
        let s = setup();
        let review = review(&s, &main_request(&s, unhex(TRONWEB_PERMISSION)));
        assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
        assert_eq!(review.counterparty_label.as_deref(), Some("新控制方"));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert!(action_has(&review, "当前控制方", OWNER));
        assert!(action_has(&review, "新控制方", PEER));
        assert!(action_has(&review, "active 权限地址", PEER));
        assert_eq!(field(&review, "type"), "AccountPermissionUpdateContract");
        assert!(review.alerts.is_empty());
    }

    #[test]
    fn extra_controllers_cannot_hide_behind_the_first_address() {
        let s = setup();
        let review = review(&s, &main_request(&s, unhex(TRONWEB_PERMISSION_HIDDEN)));
        assert_eq!(review.risk, Risk::PermissionChange);
        assert_eq!(review.counterparty.as_deref(), Some(OWNER));
        assert_eq!(review.alerts.len(), 1);
        assert!(action_has(&review, "owner 权限地址", PEER));
        assert!(action_has(&review, "active 权限地址", PEER));
    }

    /// 第 5.2 节的矩阵,逐类型.样本写了 `receiver_address` 和 `call_value`,所以这两种合约要长按.
    fn expected_class(name: &str) -> (Risk, Confirm) {
        match name {
            "TransferAssetContract"
            | "ParticipateAssetIssueContract"
            | "WitnessCreateContract"
            | "AssetIssueContract"
            | "ExchangeCreateContract"
            | "ExchangeInjectContract"
            | "ExchangeTransactionContract"
            | "MarketSellAssetContract"
            | "FreezeBalanceContract"
            | "UnfreezeBalanceContract" => (Risk::Recognized, Confirm::Hold),
            "VoteAssetContract"
            | "CustomContract"
            | "CreateSmartContract"
            | "GetContract"
            | "TriggerSmartContract"
            | "ShieldedTransferContract" => (Risk::Unknown, Confirm::Hold),
            "AccountPermissionUpdateContract" => (Risk::PermissionChange, Confirm::Hold),
            "DelegateResourceContract" => (Risk::Approval, Confirm::Hold),
            "TransferContract" => (Risk::Transfer, Confirm::Tap),
            _ => (Risk::Recognized, Confirm::Tap),
        }
    }

    #[test]
    fn every_contract_type_has_the_class_of_the_matrix() {
        let s = setup();
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(vectors["owner"], OWNER);
        let cases: Vec<&Value> = vectors["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["case"] == case["contract"])
            .collect();
        assert_eq!(cases.len(), 41);
        for case in cases {
            let name = case["contract"].as_str().unwrap();
            let req = main_request(&s, unhex(case["raw"].as_str().unwrap()));
            let reviewed = review(&s, &req);
            assert_eq!((reviewed.risk, reviewed.confirm), expected_class(name), "{name}");
            assert_eq!(reviewed.field_count as usize, reviewed.fields.len(), "{name}");
            assert_eq!(field(&reviewed, "type"), name);
            assert!(reviewed.fields.iter().all(|row| !row.label.is_empty()), "{name}");
            let (response, _) = sign(&s, &req);
            let verified = tx::verify(&req, &response, &public_key(&s)).unwrap();
            assert_eq!(verified.hash, case["txid"].as_str().unwrap(), "{name}");
            assert_eq!(reviewed.actions.len(), 1, "{name}");
            // 原始数据是 raw_data,不是 ABI,不能标方法选择器.
            if name != "TriggerSmartContract" {
                assert_eq!(reviewed.raw[0].note, None, "{name}");
            }
        }
    }

    fn message_request(s: &Setup, kind: u8, payload: Vec<u8>) -> SignRequest {
        SignRequest {
            kind,
            ..main_request(s, payload)
        }
    }

    /// 期望值由 tronweb 用同一助记词算出.tronweb 的 v 是 27 或 28,协议里的恢复位是 0 或 1.
    fn matches_tronweb(s: &Setup, req: &SignRequest, hash: &str, signature: &str) {
        let (response, reviewed) = sign(s, req);
        assert_eq!(field(&reviewed, "signing_hash"), hash, "{}", hex_text(&req.payload));
        assert_eq!(reviewed.field_count as usize, reviewed.fields.len());
        let produced = response.signatures[0].to_vec();
        let expected = unhex(signature);
        assert_eq!(hex_text(&produced[..64]), hex_text(&expected[..64]));
        assert_eq!(produced[64] + 27, expected[64]);
        let verified = tx::verify(req, &response, &public_key(s)).unwrap();
        assert_eq!(verified.hash, hash);
        assert!(verified.signature_only);
        assert_eq!(verified.raw, produced);
    }

    #[test]
    fn personal_messages_match_tronweb() {
        let s = setup();
        let vectors: Value = serde_json::from_str(MESSAGE_VECTORS).unwrap();
        for case in vectors["personal"].as_array().unwrap() {
            let payload = unhex(case["payload"].as_str().unwrap());
            let req = message_request(&s, KIND_PERSONAL_SIGN, payload);
            let hash = case["hash"].as_str().unwrap();
            matches_tronweb(&s, &req, hash, case["signature"].as_str().unwrap());
            let reviewed = review(&s, &req);
            assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
            assert!(reviewed.mainnet);
        }
    }

    #[test]
    fn typed_messages_match_tronweb() {
        let s = setup();
        let vectors: Value = serde_json::from_str(MESSAGE_VECTORS).unwrap();
        for case in vectors["typed"].as_array().unwrap() {
            let payload = case["payload"].as_str().unwrap().as_bytes().to_vec();
            let req = message_request(&s, KIND_TYPED_DATA, payload);
            let hash = case["hash"].as_str().unwrap();
            matches_tronweb(&s, &req, hash, case["signature"].as_str().unwrap());
            let reviewed = review(&s, &req);
            assert_eq!(field(&reviewed, "domain_separator"), case["domain_separator"]);
            assert_eq!(field(&reviewed, "hash_struct"), case["hash_struct"]);
        }
    }

    fn typed_payload(index: usize) -> Value {
        let vectors: Value = serde_json::from_str(MESSAGE_VECTORS).unwrap();
        let payload = vectors["typed"][index]["payload"].as_str().unwrap();
        serde_json::from_str(payload).unwrap()
    }

    #[test]
    fn tip712_addresses_show_in_base58_and_trc_token_is_a_number() {
        let s = setup();
        let bytes = serde_json::to_vec(&typed_payload(0)).unwrap();
        let reviewed = review(&s, &message_request(&s, KIND_TYPED_DATA, bytes));
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(field(&reviewed, "from"), OWNER);
        assert_eq!(field(&reviewed, "to"), PEER);
        assert_eq!(field(&reviewed, "id"), "1000001");
        assert_eq!(field(&reviewed, "verifyingContract"), USDT);
        assert!(reviewed.fields.iter().any(|row| row.name == "from" && row.own));
        assert!(alerted(&reviewed, "TIP-712"));
    }

    #[test]
    fn a_tip712_permit_is_an_approval_with_its_chain_id() {
        let s = setup();
        let mut data = typed_payload(2);
        data["domain"]["chainId"] = 728126428u64.into();
        let bytes = serde_json::to_vec(&data).unwrap();
        let reviewed = review(&s, &message_request(&s, KIND_TYPED_DATA, bytes));
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Approval, Confirm::Hold));
        assert_eq!(reviewed.counterparty.as_deref(), Some(PEER));
        assert!(reviewed.mainnet);
    }

    #[test]
    fn a_tip712_chain_that_differs_is_unknown_and_keeps_the_prompt() {
        let s = setup();
        // 向量里的 Permit 属于 Nile.
        let bytes = serde_json::to_vec(&typed_payload(2)).unwrap();
        let reviewed = review(&s, &message_request(&s, KIND_TYPED_DATA, bytes.clone()));
        assert_eq!(reviewed.risk, Risk::Unknown);
        assert!(alerted(&reviewed, "chainId 3448148188"));
        assert!(reviewed.mainnet);
        let nile = SignRequest {
            kind: KIND_TYPED_DATA,
            ..request(&s, "tron-nile", NILE_PATH, bytes)
        };
        let reviewed = review(&s, &nile);
        assert_eq!(reviewed.risk, Risk::Approval);
        assert!(!reviewed.mainnet);
    }

    #[test]
    fn an_unbound_tip712_domain_warns_and_signs() {
        let s = setup();
        let mut data = typed_payload(0);
        data["types"]["EIP712Domain"]
            .as_array_mut()
            .unwrap()
            .retain(|f| f["name"] != "chainId" && f["name"] != "verifyingContract");
        let domain = data["domain"].as_object_mut().unwrap();
        domain.remove("chainId");
        domain.remove("verifyingContract");
        let req = message_request(&s, KIND_TYPED_DATA, serde_json::to_vec(&data).unwrap());
        let reviewed = review(&s, &req);
        assert_eq!(reviewed.risk, Risk::Unknown);
        assert!(alerted(&reviewed, "不绑定链"));
        assert!(alerted(&reviewed, "不绑定合约"));
        let (response, _) = sign(&s, &req);
        assert!(tx::verify(&req, &response, &public_key(&s)).unwrap().signature_only);
    }

    #[test]
    fn a_raw_hash_needs_the_second_confirmation_and_signs_as_it_is() {
        let s = setup();
        let req = message_request(&s, KIND_RAW_HASH, vec![7; 32]);
        let reviewed = review(&s, &req);
        assert_eq!((reviewed.risk, reviewed.confirm), (Risk::Unknown, Confirm::HoldConfirm));
        assert!(alerted(&reviewed, "裸哈希"));
        let (response, _) = sign(&s, &req);
        let verified = tx::verify(&req, &response, &public_key(&s)).unwrap();
        assert_eq!(verified.hash, hex_text(&[7; 32]));
        assert!(verified.signature_only);
    }

    #[test]
    fn malformed_messages_are_refused() {
        let s = setup();
        for (kind, payload) in [
            (KIND_TYPED_DATA, native()),
            (KIND_TYPED_DATA, b"{\"a\":1,\"a\":2}".to_vec()),
            (KIND_RAW_HASH, vec![7; 31]),
        ] {
            let req = message_request(&s, kind, payload);
            assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ParseFailed));
            assert_eq!(
                s.wallet
                    .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
                    .unwrap_err(),
                Error::Refused
            );
        }
        let unknown = message_request(&s, 9, vec![0; 32]);
        assert_eq!(forbidden(review(&s, &unknown)), Some(Forbid::Unsupported));
    }

    /// 消息不绑定发起方,路径的矛盾照样拒绝.
    #[test]
    fn messages_keep_the_path_rules() {
        let s = setup();
        let mut req = message_request(&s, KIND_RAW_HASH, vec![7; 32]);
        req.paths = vec!["m/84'/0'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
    }

    #[test]
    fn a_tampered_message_signature_is_rejected() {
        let s = setup();
        let req = message_request(&s, KIND_PERSONAL_SIGN, b"hello".to_vec());
        let (response, _) = sign(&s, &req);
        let mut other = req.clone();
        other.payload = b"hellp".to_vec();
        assert!(tx::verify(&other, &response, &public_key(&s)).is_err());
    }

    #[test]
    fn malformed_payloads_cannot_be_signed() {
        let s = setup();
        let mut tail = native();
        tail.push(0);
        for payload in [tail, vec![], vec![0xff; 40]] {
            let req = main_request(&s, payload);
            assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ParseFailed));
            assert_eq!(
                s.wallet
                    .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
                    .unwrap_err(),
                Error::Refused
            );
        }
    }

    /// 随机改字节,删字节,插字节.不能 panic,能进入确认的载荷必须能原样重新编码.
    #[test]
    fn mutated_payloads_never_panic() {
        let s = setup();
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let cases = vectors["transactions"].as_array().unwrap();
        let mut seeds = vec![native(), token(), unhex(TRONWEB_PERMISSION)];
        seeds.extend(cases.iter().map(|case| unhex(case["raw"].as_str().unwrap())));
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut signable = 0;
        for _ in 0..20_000 {
            let mut payload = seeds[next() as usize % seeds.len()].clone();
            for _ in 0..=next() % 3 {
                let at = next() as usize % payload.len();
                match next() % 3 {
                    0 => payload[at] = next() as u8,
                    1 => drop(payload.remove(at)),
                    _ => payload.insert(at, next() as u8),
                }
            }
            let reviewed = review(&s, &main_request(&s, payload.clone()));
            if reviewed.is_signable() {
                signable += 1;
                assert!(UnsignedTx::decode(&payload).is_ok());
                assert_eq!(reviewed.field_count as usize, reviewed.fields.len());
            }
        }
        assert!(signable > 0);
    }

    #[test]
    fn verify_rejects_tampering() {
        let s = setup();
        let req = main_request(&s, native());
        let (response, _) = sign(&s, &req);
        let key = public_key(&s);
        assert!(tx::verify(&req, &response, &key).is_ok());

        let mut other = req.clone();
        other.payload = build_transfer(&params("1500001", None, 0)).unwrap();
        assert_eq!(
            tx::verify(&other, &response, &key).unwrap_err(),
            bitone_core::Error::BadSignature
        );

        let mut wrong_id = response.clone();
        wrong_id.id = [1; 16];
        assert_eq!(
            tx::verify(&req, &wrong_id, &key).unwrap_err(),
            bitone_core::Error::ResponseMismatch
        );

        assert!(tx::verify(&req, &response, &[2u8; 33]).is_err());

        // 另一把合法公钥:签名恢复不出它,发起方也对不上.
        let nile = s.keys.secp256k1(&DerivationPath::parse(NILE_PATH).unwrap()).unwrap();
        assert_eq!(
            tx::verify(&req, &response, &nile).unwrap_err(),
            bitone_core::Error::BadSignature
        );

        let mut bad = response.clone();
        let mut bytes = bad.signatures[0].to_vec();
        bytes[64] = 27;
        bad.signatures = vec![bytes.into()];
        assert_eq!(
            tx::verify(&req, &bad, &key).unwrap_err(),
            bitone_core::Error::BadSignature
        );

        let mut short = response.clone();
        short.signatures = vec![vec![0u8; 64].into()];
        assert_eq!(
            tx::verify(&req, &short, &key).unwrap_err(),
            bitone_core::Error::BadSignature
        );

        let mut two = response.clone();
        two.signatures.push(two.signatures[0].clone());
        assert_eq!(
            tx::verify(&req, &two, &key).unwrap_err(),
            bitone_core::Error::BadSignature
        );

        let mut broken = req.clone();
        broken.payload.push(0);
        assert_eq!(
            tx::verify(&broken, &response, &key).unwrap_err(),
            bitone_core::Error::InvalidParams
        );
    }

    #[test]
    fn verify_rejects_high_s() {
        let s = setup();
        let req = main_request(&s, native());
        let (response, _) = sign(&s, &req);
        let mut bytes = response.signatures[0].to_vec();
        // 用 n - s 和翻转的恢复位构造同一把公钥的高 s 签名.
        const N: [u8; 32] = hex!("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let value = N[i] as i16 - bytes[32 + i] as i16 - borrow;
            bytes[32 + i] = value as u8;
            borrow = i16::from(value < 0);
        }
        bytes[64] ^= 1;
        let mut high = response.clone();
        high.signatures = vec![bytes.into()];
        assert_eq!(
            tx::verify(&req, &high, &public_key(&s)).unwrap_err(),
            bitone_core::Error::BadSignature
        );
    }
}

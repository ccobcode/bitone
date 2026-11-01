use crate::Error;
use crate::keys::Wallet;
use bitone_core::request::SignRequest;
use bitone_core::tx::tron;
use bitone_core::{DerivationPath, Network};

impl Wallet {
    pub(crate) fn sign_tron(&self, request: &SignRequest, network: Network) -> Result<Vec<u8>, Error> {
        let [path] = request.paths.as_slice() else {
            return Err(Error::Refused);
        };
        DerivationPath::parse(path)?;
        let digest = tron::signing_hash(request).map_err(|_| Error::Refused)?;
        self.sign_recoverable(path, digest, network.mode())
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;
    use crate::keys::Wallet;
    use bitone_core::keys::PublicKeys;
    use bitone_core::manifest::Manifest;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
    use bitone_core::review::{Confirm, Forbid, Review, Risk};
    use bitone_core::tx::tron::{Call, RawData, TransferParams, build_transfer};
    use bitone_core::tx::{self, HighestChange, ReviewContext};
    use bitone_core::{DerivationPath, Network};
    use hex_literal::hex;

    const PATH: &str = "m/44'/195'/0'/0/0";
    const NILE_PATH: &str = "m/44'/1'/0'/0/0";
    const OWNER: &str = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";
    const OWNER_BYTES: [u8; 20] = hex!("c8599111f29c1e1e061265b4af93ea1f274ad78a");
    const PEER: &str = "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE";
    const PEER_BYTES: [u8; 20] = hex!("a2726afbecbd8e936000ed684cef5e2f5cf43008");
    const USDT: &str = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
    const USDT_BYTES: [u8; 20] = hex!("a614f803b6fd780986a42c78ec9c7f77e6ded13c");
    const FAKE: [u8; 20] = [0x33; 20];
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
        let review = tx::review(request, &ctx);
        assert!(review.fields.is_empty(), "这条链还没迁移");
        review
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

    fn value<'a>(review: &'a Review, label: &str) -> &'a str {
        review
            .rows
            .iter()
            .find(|r| r.label == label)
            .map_or("", |r| r.value.as_str())
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

    fn wrap(call: Call) -> RawData {
        RawData {
            ref_block_bytes: hex!("abcd"),
            ref_block_hash: hex!("0102030405060708"),
            expiration: LATER,
            call,
            timestamp: NOW,
            fee_limit: 0,
        }
    }

    fn trigger(contract: [u8; 20], call_value: u64, data: Vec<u8>) -> RawData {
        wrap(Call::Trigger {
            owner: OWNER_BYTES,
            contract,
            call_value,
            data,
        })
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
        assert_eq!(value(&review, "网络"), "TRON");
        assert_eq!(value(&review, "付款账户"), OWNER);
        assert_eq!(value(&review, "派生路径"), PATH);
        assert_eq!(value(&review, "过期时间"), "2027-01-15 08:30:00 UTC");
        assert_eq!(value(&review, "创建时间"), "2027-01-15 08:00:00 UTC");
        assert_eq!(value(&review, "交易类型"), "TransferContract");
        assert_eq!(value(&review, "fee_limit"), "0 TRX");
        assert!(review.alerts.is_empty());
    }

    #[test]
    fn plain_token_transfer() {
        let s = setup();
        let review = review(&s, &main_request(&s, token()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.25", Some("USDT")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(value(&review, "交易类型"), "TriggerSmartContract");
        assert_eq!(value(&review, "fee_limit"), "100 TRX");
        assert!(action_has(&review, "调用方法", "transfer"));
        assert!(action_has(&review, "合约", USDT));
        assert!(action_has(&review, "合约清单", "在内置清单"));
        assert_eq!(review.raw[0].note.as_deref(), Some("方法选择器"));
        assert_eq!(review.raw.len(), 3);
    }

    #[test]
    fn token_outside_the_manifest_is_not_a_plain_transfer() {
        let s = setup();
        let data = abi(TRANSFER, &[word(&PEER_BYTES), word(&[0x12, 0xd6, 0x90])]);
        let review = review(&s, &main_request(&s, trigger(FAKE, 0, data).encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1234576", None));
    }

    #[test]
    fn trc20_approve_is_an_approval() {
        let s = setup();
        let data = abi(APPROVE, &[word(&PEER_BYTES), vec![0xff; 32]]);
        let review = review(&s, &main_request(&s, trigger(USDT_BYTES, 0, data).encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
        assert!(review.amount.unwrap().unlimited);
        assert_eq!(review.counterparty_label.as_deref(), Some("被授权方"));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
    }

    #[test]
    fn call_value_makes_a_transfer_unknown_and_is_shown() {
        let s = setup();
        let data = abi(TRANSFER, &[word(&PEER_BYTES), word(&[1])]);
        let review = review(&s, &main_request(&s, trigger(USDT_BYTES, 2_000_000, data).encode()));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(value(&review, "附带金额"), "2 TRX");
        assert!(review.alerts.iter().any(|a| a.contains("2 TRX")));
    }

    #[test]
    fn unknown_method_shows_the_data() {
        let s = setup();
        let unknown = review(
            &s,
            &main_request(&s, trigger(FAKE, 0, hex!("deadbeef").to_vec()).encode()),
        );
        assert_eq!(unknown.risk, Risk::Unknown);
        assert_eq!(unknown.raw[0].text, "deadbeef");
        let empty = review(&s, &main_request(&s, trigger(USDT_BYTES, 1, Vec::new()).encode()));
        assert_eq!(empty.risk, Risk::Unknown);
    }

    #[test]
    fn replaced_owner_address_is_refused() {
        let s = setup();
        let stranger = PEER_BYTES;
        let transfer = wrap(Call::Transfer {
            owner: stranger,
            to: OWNER_BYTES,
            amount: 5,
        });
        assert_eq!(
            forbidden(review(&s, &main_request(&s, transfer.encode()))),
            Some(Forbid::AddressMismatch)
        );
        let trigger = wrap(Call::Trigger {
            owner: stranger,
            contract: USDT_BYTES,
            call_value: 0,
            data: vec![1],
        });
        assert_eq!(
            forbidden(review(&s, &main_request(&s, trigger.encode()))),
            Some(Forbid::AddressMismatch)
        );
        let mut value = vec![0x0a, 7];
        value.extend_from_slice(b"1002000");
        value.extend_from_slice(&[0x12, 21, 0x41]);
        value.extend_from_slice(&stranger);
        let asset = wrap(Call::Other {
            kind: 2,
            owner: stranger,
            value,
        });
        assert_eq!(
            forbidden(review(&s, &main_request(&s, asset.encode()))),
            Some(Forbid::AddressMismatch)
        );
        let result = s.wallet.sign(
            &main_request(&s, transfer.encode()),
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
        req.paths = vec!["m/44'/195'/0'/0/1".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
        req.paths = vec!["m/44'/60'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![NILE_PATH.into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
        let nile = request(&s, "tron-nile", PATH, native());
        assert_eq!(forbidden(review(&s, &nile)), Some(Forbid::PathNetworkMismatch));
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
        assert_eq!(value(&reviewed, "网络"), "Nile");
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
        assert_eq!(value(&review, "交易类型"), "AccountPermissionUpdateContract");
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

    #[test]
    fn other_system_contracts_are_unknown_and_show_raw_fields() {
        let s = setup();
        // tronweb 的 txJsonToPb 编码.
        let cases = [
            (
                "FreezeBalanceV2Contract",
                "0a02abcd2208010203040506070840c08edfc3b1345a59083612550a34747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e467265657a6542616c616e63655632436f6e7472616374121d0a1541c8599111f29c1e1e061265b4af93ea1f274ad78a10c0843d18017080a0f1c2b134",
            ),
            (
                "TransferAssetContract",
                "0a02abcd2208010203040506070840c08edfc3b1345a730802126f0a32747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e736665724173736574436f6e747261637412390a0731303032303030121541c8599111f29c1e1e061265b4af93ea1f274ad78a1a1541a2726afbecbd8e936000ed684cef5e2f5cf4300820057080a0f1c2b134",
            ),
            (
                "AccountUpdateContract",
                "0a02abcd2208010203040506070840c08edfc3b1345a59080a12550a32747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e4163636f756e74557064617465436f6e7472616374121f0a066269746f6e65121541c8599111f29c1e1e061265b4af93ea1f274ad78a7080a0f1c2b134",
            ),
            (
                "AccountCreateContract",
                "0a02abcd2208010203040506070840c08edfc3b1345a6612640a32747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e4163636f756e74437265617465436f6e7472616374122e0a1541c8599111f29c1e1e061265b4af93ea1f274ad78a121541a2726afbecbd8e936000ed684cef5e2f5cf430087080a0f1c2b134",
            ),
        ];
        for (name, raw) in cases {
            let review = review(&s, &main_request(&s, unhex(raw)));
            assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold), "{name}");
            assert_eq!(review.actions[0].title, name);
            assert_eq!(value(&review, "交易类型"), name);
            assert!(review.actions[0].rows.iter().any(|r| r.label == "字段 1"));
            // 原始数据是 raw_data,不是 ABI,不能标方法选择器.
            assert_eq!(review.raw[0].note, None);
            assert!(raw.starts_with(&review.raw[0].text));
        }
    }

    #[test]
    fn message_signing_is_refused() {
        let s = setup();
        let mut req = main_request(&s, native());
        req.kind = KIND_TYPED_DATA;
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::MessageSigning));
        assert_eq!(
            s.wallet
                .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
                .unwrap_err(),
            Error::Refused
        );
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
        let seeds = [
            native(),
            token(),
            unhex(TRONWEB_PERMISSION),
            unhex(
                "0a02abcd2208010203040506070840c08edfc3b1345a59083612550a34747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e467265657a6542616c616e63655632436f6e7472616374121d0a1541c8599111f29c1e1e061265b4af93ea1f274ad78a10c0843d18017080a0f1c2b134",
            ),
        ];
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
            if review(&s, &main_request(&s, payload.clone())).is_signable() {
                signable += 1;
                assert_eq!(RawData::decode(&payload).unwrap().encode(), payload);
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

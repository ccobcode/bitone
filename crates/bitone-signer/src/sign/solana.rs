use crate::Error;
use crate::keys::Wallet;
use bitone_core::request::SignRequest;
use bitone_core::tx::solana::Message;
use bitone_core::{DerivationPath, Network};
use ed25519_dalek::{Signer, SigningKey};

impl Wallet {
    pub(crate) fn sign_solana(&self, request: &SignRequest, _network: Network) -> Result<Vec<u8>, Error> {
        let [path] = request.paths.as_slice() else {
            return Err(Error::Refused);
        };
        let path = DerivationPath::parse(path)?;
        Message::decode(&request.payload).map_err(|_| Error::Refused)?;
        let key = SigningKey::from_bytes(&*self.ed25519_key(&path)?);
        Ok(key.sign(&request.payload).to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;
    use crate::keys::Wallet;
    use bitone_core::address::parse_solana_address;
    use bitone_core::keys::PublicKeys;
    use bitone_core::manifest::Manifest;
    use bitone_core::protocol::VERSION;
    use bitone_core::request::{KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
    use bitone_core::review::{Confirm, Forbid, Review, Risk};
    use bitone_core::tx::solana::{
        ASSOCIATED_TOKEN_PROGRAM, COMPUTE_BUDGET_PROGRAM, Instruction, Lookup, Message, RECENT_BLOCKHASHES_SYSVAR,
        RENT_SYSVAR, SYSTEM_PROGRAM, TOKEN_2022_PROGRAM, TOKEN_PROGRAM, TransferParams, Version,
        associated_token_address, build_advance_nonce, build_create_nonce, build_transfer, nonce_account_address,
    };
    use bitone_core::tx::{self, HighestChange, ReviewContext};
    use bitone_core::{DerivationPath, Network};

    const PATH: &str = "m/44'/501'/0'/0'";
    const DEVNET_PATH: &str = "m/44'/1'/0'/0'";
    const PAYER: &str = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";
    const NONCE: &str = "DR8vVnEzE8JD2JXdqmNXZxo3WneEEdxdxoAEfycoq7XT";
    const PEER: &str = "7S3P4HxJpyyigGzodYwHtCxZyUQe9JiBMHyRWXArAaKv";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const SOURCE_ATA: &str = "5N3f1tj9v1vc5TUZ8S7mCAnVmjVKrfnzXWhxLaxyZAgt";
    const PEER_ATA: &str = "8qivCif5vUJSnr2PyYqeBM8hWDUZn1RUSv6mKBBSvJ92";
    const STRANGER: [u8; 32] = [0x44; 32];
    const VICTIM_PROGRAM: [u8; 32] = [0x55; 32];
    const BLOCKHASH: [u8; 32] = [7; 32];
    const NONCE_VALUE: [u8; 32] = [9; 32];

    struct Setup {
        wallet: Wallet,
        keys: PublicKeys,
    }

    fn setup() -> Setup {
        let wallet = Wallet::new(&[0u8; 16], "").unwrap();
        let keys = PublicKeys::decode(&wallet.public_keys(false).unwrap()).unwrap();
        Setup { wallet, keys }
    }

    fn key(text: &str) -> [u8; 32] {
        parse_solana_address(text).unwrap()
    }

    fn hex_text(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sign(s: &Setup, req: &SignRequest) -> (SignResponse, Review) {
        s.wallet
            .sign(req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap()
    }

    fn request(s: &Setup, payload: Vec<u8>) -> SignRequest {
        SignRequest {
            version: VERSION,
            id: [9; 16],
            fingerprint: s.keys.fingerprint,
            network: "solana".into(),
            kind: KIND_TRANSACTION,
            paths: vec![PATH.into()],
            payload,
            hint: None,
            chain: None,
        }
    }

    fn hinted(s: &Setup, payload: Vec<u8>, hint: &str) -> SignRequest {
        SignRequest {
            hint: Some(hint.into()),
            ..request(s, payload)
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

    fn check(s: &Setup, message: &Message) -> Review {
        review(s, &request(s, message.encode()))
    }

    fn forbidden(review: Review) -> Option<Forbid> {
        assert_eq!(review.confirm, Confirm::Refuse);
        review.forbidden
    }

    fn row<'a>(review: &'a Review, label: &str) -> &'a str {
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

    fn native_params() -> TransferParams<'static> {
        TransferParams {
            network: Network::Solana,
            from: PAYER,
            to: PEER,
            amount: 1_500_000_000,
            mint: None,
            decimals: 0,
            nonce_value: NONCE_VALUE,
            priority_micro_lamports: 1000,
            compute_units: 200_000,
            create_recipient_ata: false,
        }
    }

    fn token_params(create_recipient_ata: bool) -> TransferParams<'static> {
        TransferParams {
            amount: 1_250_000,
            mint: Some(USDC),
            decimals: 6,
            priority_micro_lamports: 5000,
            compute_units: 60_000,
            create_recipient_ata,
            ..native_params()
        }
    }

    fn native() -> Vec<u8> {
        build_transfer(&native_params()).unwrap()
    }

    fn token(create_recipient_ata: bool) -> Vec<u8> {
        build_transfer(&token_params(create_recipient_ata)).unwrap()
    }

    /// 测试用的指令,账户直接写地址.
    struct Ix {
        program: [u8; 32],
        accounts: Vec<[u8; 32]>,
        data: Vec<u8>,
    }

    fn ix(program: [u8; 32], accounts: &[[u8; 32]], data: &[u8]) -> Ix {
        Ix {
            program,
            accounts: accounts.to_vec(),
            data: data.to_vec(),
        }
    }

    /// 账户表:付款账户,其他签名者,指令里的账户,程序.
    fn compose(payer: [u8; 32], signers: &[[u8; 32]], instructions: &[Ix]) -> Message {
        let mut keys = vec![payer];
        keys.extend_from_slice(signers);
        let mut add = |key: &[u8; 32]| {
            if !keys.contains(key) {
                keys.push(*key);
            }
        };
        instructions.iter().flat_map(|i| &i.accounts).for_each(&mut add);
        instructions.iter().map(|i| &i.program).for_each(&mut add);
        let index = |key: &[u8; 32]| keys.iter().position(|k| k == key).unwrap() as u8;
        Message {
            version: Version::Legacy,
            required_signatures: 1 + signers.len() as u8,
            readonly_signed: 0,
            readonly_unsigned: 0,
            instructions: instructions
                .iter()
                .map(|i| Instruction {
                    program: index(&i.program),
                    accounts: i.accounts.iter().map(index).collect(),
                    data: i.data.clone(),
                })
                .collect(),
            keys: keys.clone(),
            blockhash: NONCE_VALUE,
            lookups: Vec::new(),
        }
    }

    fn payer() -> [u8; 32] {
        key(PAYER)
    }

    fn nonce() -> [u8; 32] {
        key(NONCE)
    }

    fn advance() -> Ix {
        ix(
            SYSTEM_PROGRAM,
            &[nonce(), RECENT_BLOCKHASHES_SYSVAR, payer()],
            &[4, 0, 0, 0],
        )
    }

    fn transfer(lamports: u64) -> Ix {
        let mut data = vec![2, 0, 0, 0];
        data.extend_from_slice(&lamports.to_le_bytes());
        ix(SYSTEM_PROGRAM, &[payer(), key(PEER)], &data)
    }

    fn token_data(tag: u8, amount: u64, decimals: Option<u8>) -> Vec<u8> {
        let mut data = vec![tag];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend(decimals);
        data
    }

    fn approve(amount: u64) -> Ix {
        ix(
            TOKEN_PROGRAM,
            &[key(SOURCE_ATA), STRANGER, payer()],
            &token_data(4, amount, None),
        )
    }

    fn transfer_checked(program: [u8; 32], mint: [u8; 32], decimals: u8) -> Ix {
        let destination = associated_token_address(&key(PEER), &mint, &program).unwrap();
        ix(
            program,
            &[key(SOURCE_ATA), mint, destination, payer()],
            &token_data(12, 1_250_000, Some(decimals)),
        )
    }

    fn compute_limit(units: u32) -> Ix {
        let mut data = vec![2];
        data.extend_from_slice(&units.to_le_bytes());
        ix(COMPUTE_BUDGET_PROGRAM, &[], &data)
    }

    fn compute_price(micro_lamports: u64) -> Ix {
        let mut data = vec![3];
        data.extend_from_slice(&micro_lamports.to_le_bytes());
        ix(COMPUTE_BUDGET_PROGRAM, &[], &data)
    }

    // 下面的期望值由 @solana/web3.js 1.99.0,@solana/spl-token 0.4.15 用同一助记词算出:
    // TransactionMessage.compileToLegacyMessage 生成消息,tweetnacl 签名,VersionedTransaction 序列化.
    // 私钥用 ed25519-hd-key 派生,都是独立实现.
    const WEB3_CREATE: [&str; 4] = [
        "01000305f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea940000006a7d517192c5c51218cc94c3d4af17f58daee089ba1fd44e3dbd98a00000000070707070707070707070707070707070707070707070707070707070707070703020200010c020000000017160000000000020201006009000000f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f70c000000000000006269746f6e652d6e6f6e63655000000000000000000000000000000000000000000000000000000000000000000000000000000002030103042406000000f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7",
        "4ba3234bc25eee10b28d79809c73e56f90fd7dc70ba6a588cc703c7a02e621edc326d098ba64d0bb8317b87f04a79b8eb990c0e0a1e431dcea5250ceb5613505",
        "014ba3234bc25eee10b28d79809c73e56f90fd7dc70ba6a588cc703c7a02e621edc326d098ba64d0bb8317b87f04a79b8eb990c0e0a1e431dcea5250ceb561350501000305f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea940000006a7d517192c5c51218cc94c3d4af17f58daee089ba1fd44e3dbd98a00000000070707070707070707070707070707070707070707070707070707070707070703020200010c020000000017160000000000020201006009000000f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f70c000000000000006269746f6e652d6e6f6e63655000000000000000000000000000000000000000000000000000000000000000000000000000000002030103042406000000f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7",
        "2Wi9RUnJa5fqWhBSC3WR6qmESKyPhRGcqYmWe9avAAJNEG55yZVcHpz3uCvKsGeuRYJjvM4zkpwFE1uMxU4Nmpqz",
    ];
    const WEB3_NATIVE: [&str; 4] = [
        "01000306f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe5f90554bb3d8c2fc82b6ee59c49aaa143e77f7d49a83e956ce1dbef17a43f805000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea94000000306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a400000000909090909090909090909090909090909090909090909090909090909090909040303010400040400000005000502400d030005000903e803000000000000030200020c02000000002f685900000000",
        "b6fc588c725bac67af82473f871a71d314f244fdd7b5ab46fae2cbed07cf3d755f6646c100feb934383f28811d77dc27d3134baf62cc37b3336730e9119ea40d",
        "01b6fc588c725bac67af82473f871a71d314f244fdd7b5ab46fae2cbed07cf3d755f6646c100feb934383f28811d77dc27d3134baf62cc37b3336730e9119ea40d01000306f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe5f90554bb3d8c2fc82b6ee59c49aaa143e77f7d49a83e956ce1dbef17a43f805000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea94000000306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a400000000909090909090909090909090909090909090909090909090909090909090909040303010400040400000005000502400d030005000903e803000000000000030200020c02000000002f685900000000",
        "4fC6YjaXW5ixgdANUgzeHy88Eoyn3cdHAuXFhWiLK4sSyQrxL5hMYjNtuAGf4WvzRorxBRBBaEbk2jfGUHKoXbun",
    ];
    const WEB3_TOKEN: [&str; 4] = [
        "0100070bf036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe747d15557d94b018733d98e2d28e8cca1aabe5566d8ccb18c3a65e03a4df3fcd40d2f27c461f29e9feba1ab8acd39496a9c65b0ae62a713f01f941a993bf55ed000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea94000000306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a400000008c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8dbe9f8595f90554bb3d8c2fc82b6ee59c49aaa143e77f7d49a83e956ce1dbef17a43f805c6fa7af3bedbad3a3d65f36aabc97431b1bbe4c2d2f6e0e47ca60203452f5d6106ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9090909090909090909090909090909090909090909090909090909090909090905040301050004040000000600050260ea0000060009038813000000000000070600020809040a01010a04030902000a0cd01213000000000006",
        "5530569b51cc72a0ca2f2c8cb36528565db5dc78fec9b6098c11a3d350dcf5281047ac4fb94c07e02e5153b651ccd0dd5d2f7042ad3ee8cda1a1952e011a330b",
        "015530569b51cc72a0ca2f2c8cb36528565db5dc78fec9b6098c11a3d350dcf5281047ac4fb94c07e02e5153b651ccd0dd5d2f7042ad3ee8cda1a1952e011a330b0100070bf036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe747d15557d94b018733d98e2d28e8cca1aabe5566d8ccb18c3a65e03a4df3fcd40d2f27c461f29e9feba1ab8acd39496a9c65b0ae62a713f01f941a993bf55ed000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea94000000306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a400000008c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8dbe9f8595f90554bb3d8c2fc82b6ee59c49aaa143e77f7d49a83e956ce1dbef17a43f805c6fa7af3bedbad3a3d65f36aabc97431b1bbe4c2d2f6e0e47ca60203452f5d6106ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9090909090909090909090909090909090909090909090909090909090909090905040301050004040000000600050260ea0000060009038813000000000000070600020809040a01010a04030902000a0cd01213000000000006",
        "2hnZFYZPR3opERgES5TLUK7ngvfHR9konW1MHYMgKXxY9ScdtNnrn1ckboKBLNJ81KUM4jjEsLJBKaFvLqekkCGJ",
    ];
    const WEB3_TOKEN_NO_ATA: [&str; 4] = [
        "01000408f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe40d2f27c461f29e9feba1ab8acd39496a9c65b0ae62a713f01f941a993bf55ed747d15557d94b018733d98e2d28e8cca1aabe5566d8ccb18c3a65e03a4df3fcd000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9c6fa7af3bedbad3a3d65f36aabc97431b1bbe4c2d2f6e0e47ca60203452f5d61090909090909090909090909090909090909090909090909090909090909090902040301050004040000000604020703000a0cd01213000000000006",
        "354ee892c55ce069afa6ddffa7c2371380f431e0d043705bfa86ae1747b5f51f52a219a6baf30aa7d87f875e3843d53ddf2ed7d13b83e2480746ff653783e301",
        "01354ee892c55ce069afa6ddffa7c2371380f431e0d043705bfa86ae1747b5f51f52a219a6baf30aa7d87f875e3843d53ddf2ed7d13b83e2480746ff653783e30101000408f036276246a75b9de3349ed42b15e232f6518fc20f5fcd4f1d64e81f9bd258f7b87b23eec251cafcbf375b02a80ce9e968597baebaed58bba1fcf5b8591ffefe40d2f27c461f29e9feba1ab8acd39496a9c65b0ae62a713f01f941a993bf55ed747d15557d94b018733d98e2d28e8cca1aabe5566d8ccb18c3a65e03a4df3fcd000000000000000000000000000000000000000000000000000000000000000006a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062ea940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9c6fa7af3bedbad3a3d65f36aabc97431b1bbe4c2d2f6e0e47ca60203452f5d61090909090909090909090909090909090909090909090909090909090909090902040301050004040000000604020703000a0cd01213000000000006",
        "24pMqTK7ep4cXActhFAnd6oJaVo6PSSxzTCFPbjCgDMSm2JEeNQjrN18SSe7CTe8pNXaAt9aiouDoSBMYGPB7fPW",
    ];

    fn check_against_web3js(s: &Setup, payload: Vec<u8>, vector: [&str; 4]) {
        let [message, signature, raw, hash] = vector;
        assert_eq!(hex_text(&payload), message);
        let req = request(s, payload);
        let (response, _) = sign(s, &req);
        assert_eq!(hex_text(&response.signatures[0]), signature);
        let verified = tx::verify(&req, &response, &payer()).unwrap();
        assert_eq!(hex_text(&verified.raw), raw);
        assert_eq!(verified.hash, hash);
    }

    #[test]
    fn create_nonce_matches_web3js() {
        let s = setup();
        check_against_web3js(
            &s,
            build_create_nonce(PAYER, BLOCKHASH, 1_447_680).unwrap(),
            WEB3_CREATE,
        );
    }

    #[test]
    fn native_transfer_matches_web3js() {
        let s = setup();
        check_against_web3js(&s, native(), WEB3_NATIVE);
    }

    #[test]
    fn token_transfer_matches_web3js() {
        let s = setup();
        check_against_web3js(&s, token(true), WEB3_TOKEN);
        let mut no_ata = token_params(false);
        no_ata.priority_micro_lamports = 0;
        no_ata.compute_units = 0;
        check_against_web3js(&s, build_transfer(&no_ata).unwrap(), WEB3_TOKEN_NO_ATA);
    }

    #[test]
    fn addresses_match_web3js() {
        assert_eq!(nonce_account_address(&payer()).unwrap(), nonce());
        let owner = payer();
        let mint = key(USDC);
        assert_eq!(
            associated_token_address(&owner, &mint, &TOKEN_PROGRAM).unwrap(),
            key(SOURCE_ATA)
        );
        assert_eq!(
            associated_token_address(&key(PEER), &mint, &TOKEN_PROGRAM).unwrap(),
            key(PEER_ATA)
        );
    }

    /// web3.js 的 findProgramAddressSync 算出的例子.bump 都小于 253,前几次哈希落在曲线上.
    #[test]
    fn program_addresses_skip_points_on_the_curve() {
        let cases = [
            (
                "HHjc7SV8L1DfEywQm2kCWByEhy6SzFMThdaienMEUaFf",
                "5WkEHzkWSKnQ3nSobYX4xMFd4KUX7J2Ko73Ez6W9jpB2",
                false,
                "9BULDFwcjLHNPkMzdfArfX7r78aFWsng2US3vAPhtUf1",
            ),
            (
                "BaaF7JeSrfqd6TgpQpt3K5VcRYFagxBMMfZ117ikf9mr",
                "GxjBaiCBSMs1bYMQimXR3R1gSoum4MyhVX652tE3PVFM",
                false,
                "36pbTH3Uf2Q5rvznf2c1iKr1vnxN5SpMTnAfMd77nswJ",
            ),
            (
                "BaaF7JeSrfqd6TgpQpt3K5VcRYFagxBMMfZ117ikf9mr",
                "GxjBaiCBSMs1bYMQimXR3R1gSoum4MyhVX652tE3PVFM",
                true,
                "C2MqSeo5vNuHYCuP4T5CHGZzVCDjAQup2Wb7outv6SoV",
            ),
            (
                "7JS9W9C8oCeRBBahV6Ve2qpR4p4ZLDsVUsJmiibyBqau",
                "7jdfSkEYejeVEJaGiCc29WDR3tq4hbL1ixpcWyuuWHz8",
                false,
                "HiTZ8qgrDyA3y4zcY1QKs9YySBMqQB6maTvjKntkLnkd",
            ),
            (
                "37gpeMzwyneRhydfmLJL4BznoHcJSjQZ2eQQbviQTdLt",
                "GLf5LH19xWEjyN6Bmn93qu7gzbRhKfNSmFC4jPw2rAQ3",
                true,
                "H5bkUDE5axF3cR5APTw5uEVCtBYQ4CFZY2nHwxqZMc4x",
            ),
            (
                "81SDXivKSpBjKMCP2whmXbgMYHP9CG72cCGeuJttNQee",
                "GKpgR4DK4J7VwyC9mdsAZZD4gcXK9oqkGVM8u64ZCaQm",
                false,
                "CyLypgzwbZ1BHVPEHpjXPtg4yVZrgkF2P4FQcYvY1MTe",
            ),
        ];
        for (owner, mint, token_2022, address) in cases {
            let program = if token_2022 { TOKEN_2022_PROGRAM } else { TOKEN_PROGRAM };
            assert_eq!(
                associated_token_address(&key(owner), &key(mint), &program).unwrap(),
                key(address)
            );
        }
    }

    #[test]
    fn plain_native_transfer() {
        let s = setup();
        let review = review(&s, &request(&s, native()));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.5", Some("SOL")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(row(&review, "网络"), "Solana");
        assert_eq!(row(&review, "手续费支付者"), PAYER);
        assert_eq!(row(&review, "派生路径"), PATH);
        assert_eq!(row(&review, "全部签名者"), PAYER);
        assert_eq!(row(&review, "nonce 账户"), NONCE);
        assert_eq!(row(&review, "消息版本"), "legacy");
        assert_eq!(row(&review, "指令数"), "4");
        assert_eq!(row(&review, "基础费"), "0.000005 SOL");
        assert_eq!(row(&review, "优先费"), "0.0000002 SOL");
        // Advance 和 ComputeBudget 不算动作.
        assert_eq!(review.actions.len(), 1);
        assert!(review.alerts.is_empty());
    }

    #[test]
    fn plain_token_transfer_with_a_confirmed_owner() {
        let s = setup();
        let review = review(&s, &hinted(&s, token(false), PEER));
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("1.25", Some("USDC")));
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert_eq!(row(&review, "收款地址"), "");
        assert!(action_has(&review, "合约清单", "在内置清单"));
    }

    #[test]
    fn creating_the_recipient_account_is_recognized() {
        let s = setup();
        let review = review(&s, &hinted(&s, token(true), PEER));
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Tap));
        // 顶部仍然显示转账.
        assert_eq!(review.amount.as_ref().unwrap().text, "1.25");
        assert_eq!(review.counterparty.as_deref(), Some(PEER));
        assert!(action_has(&review, "所有者", PEER));
        assert!(action_has(&review, "代币", USDC));
    }

    #[test]
    fn owner_is_shown_only_when_it_derives_the_destination() {
        let s = setup();
        for req in [
            request(&s, token(false)),
            hinted(&s, token(false), PAYER),
            hinted(&s, token(false), "不是地址"),
        ] {
            let review = review(&s, &req);
            assert_eq!(review.counterparty.as_deref(), Some(PEER_ATA));
            assert_eq!(row(&review, "收款地址"), "无法确认所有者");
            assert!(action_has(&review, "收款地址", "无法确认所有者"));
            // 收款方是谁都没确认,不能点一下就签.
            assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        }
    }

    /// 给 PEER 建账户,转账却转给别人的代币账户,声明的所有者也推不出目标账户.
    #[test]
    fn creating_one_account_and_paying_another_is_caught() {
        let s = setup();
        let accounts = ata_accounts(key(PEER_ATA), TOKEN_PROGRAM);
        let mut stolen = transfer_checked(TOKEN_PROGRAM, key(USDC), 6);
        stolen.accounts[2] = STRANGER;
        let message = compose(
            payer(),
            &[],
            &[advance(), ix(ASSOCIATED_TOKEN_PROGRAM, &accounts, &[1]), stolen],
        );
        let review = review(&s, &hinted(&s, message.encode(), PEER));
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&STRANGER)).as_deref()
        );
    }

    #[test]
    fn creating_the_nonce_account_is_recognized() {
        let s = setup();
        let review = review(
            &s,
            &request(&s, build_create_nonce(PAYER, BLOCKHASH, 1_447_680).unwrap()),
        );
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Tap));
        assert_eq!(review.title, "创建 nonce 账户");
        assert_eq!(review.actions.len(), 1);
        assert_eq!(review.actions[0].title, "创建 nonce 账户");
        assert_eq!(review.counterparty.as_deref(), Some(NONCE));
        assert_eq!(review.amount.unwrap().text, "0.00144768");
    }

    fn create_data(authority_base: [u8; 32], seed: &str, lamports: u64, space: u64, owner: [u8; 32]) -> Vec<u8> {
        let mut data = vec![3, 0, 0, 0];
        data.extend_from_slice(&authority_base);
        data.extend_from_slice(&(seed.len() as u64).to_le_bytes());
        data.extend_from_slice(seed.as_bytes());
        data.extend_from_slice(&lamports.to_le_bytes());
        data.extend_from_slice(&space.to_le_bytes());
        data.extend_from_slice(&owner);
        data
    }

    fn initialize(authority: [u8; 32], account: [u8; 32]) -> Ix {
        let mut data = vec![6, 0, 0, 0];
        data.extend_from_slice(&authority);
        ix(
            SYSTEM_PROGRAM,
            &[account, RECENT_BLOCKHASHES_SYSVAR, RENT_SYSVAR],
            &data,
        )
    }

    fn allocate() -> Ix {
        let mut data = vec![9, 0, 0, 0];
        data.extend_from_slice(&payer());
        data.extend_from_slice(&12u64.to_le_bytes());
        data.extend_from_slice(b"bitone-nonce");
        data.extend_from_slice(&80u64.to_le_bytes());
        data.extend_from_slice(&SYSTEM_PROGRAM);
        ix(SYSTEM_PROGRAM, &[nonce(), payer()], &data)
    }

    fn fund() -> Ix {
        let mut data = vec![2, 0, 0, 0];
        data.extend_from_slice(&1_447_680u64.to_le_bytes());
        ix(SYSTEM_PROGRAM, &[payer(), nonce()], &data)
    }

    fn own_creation() -> [Ix; 3] {
        [fund(), allocate(), initialize(payer(), nonce())]
    }

    #[test]
    fn hand_built_nonce_creation_is_recognized() {
        let s = setup();
        assert_eq!(
            check(&s, &compose(payer(), &[], &own_creation())).risk,
            Risk::Recognized
        );
    }

    /// CreateAccountWithSeed 会被预存款挡住,所以这种写法不再算本钱包的 nonce 创建.
    #[test]
    fn old_create_account_flow_is_not_recognized() {
        let s = setup();
        let data = create_data(payer(), "bitone-nonce", 1_447_680, 80, SYSTEM_PROGRAM);
        let create = ix(SYSTEM_PROGRAM, &[payer(), nonce()], &data);
        let review = check(&s, &compose(payer(), &[], &[create, initialize(payer(), nonce())]));
        assert_eq!(review.risk, Risk::Unknown);
    }

    /// "创建 nonce 账户"是点按确认,不能让转账藏在这个标题后面.
    #[test]
    fn nonce_creation_cannot_carry_other_actions() {
        let s = setup();
        let [a, b, c] = own_creation();
        let review = check(&s, &compose(payer(), &[], &[a, b, c, transfer(9_000_000_000)]));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_ne!(review.title, "创建 nonce 账户");
        assert!(action_has(&review, "金额", "9 SOL"));

        let [a, b, c] = own_creation();
        let review = check(&s, &compose(payer(), &[], &[transfer(9_000_000_000), a, b, c]));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));

        // 优先费不算动作.
        let [a, b, c] = own_creation();
        let review = check(
            &s,
            &compose(payer(), &[], &[compute_limit(10_000), compute_price(1), a, b, c]),
        );
        assert_eq!(
            (review.risk, review.title.as_str()),
            (Risk::Recognized, "创建 nonce 账户")
        );
    }

    #[test]
    fn advancing_the_nonce_is_a_labelled_cancel() {
        let s = setup();
        let payload = build_advance_nonce(PAYER, NONCE_VALUE).unwrap();
        let review = review(&s, &request(&s, payload));
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Hold));
        assert_eq!(review.title, "作废已签名的交易");
    }

    #[test]
    fn advancing_someone_elses_nonce_is_not_a_cancel() {
        let s = setup();
        let stranger = [0x66; 32];
        let message = compose(
            payer(),
            &[],
            &[ix(
                SYSTEM_PROGRAM,
                &[stranger, RECENT_BLOCKHASHES_SYSVAR, payer()],
                &[4, 0, 0, 0],
            )],
        );
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
    }

    #[test]
    fn large_nonce_deposit_needs_a_long_press() {
        let s = setup();
        let at_ceiling = build_create_nonce(PAYER, BLOCKHASH, 10_000_000).unwrap();
        assert_eq!(review(&s, &request(&s, at_ceiling)).confirm, Confirm::Tap);
        let over = build_create_nonce(PAYER, BLOCKHASH, 10_000_001).unwrap();
        let review = review(&s, &request(&s, over));
        assert_eq!((review.risk, review.confirm), (Risk::Recognized, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
    }

    #[test]
    fn nonce_creation_for_someone_else_is_unknown() {
        let s = setup();
        let lamports = 1_447_680;
        let attacker_nonce = [0x66; 32];
        let cases = [
            // authority 是别人:对方可以提走 nonce 账户里的钱.
            vec![fund(), allocate(), initialize(STRANGER, nonce())],
            // 地址不是本钱包的 nonce 账户.
            vec![
                ix(
                    SYSTEM_PROGRAM,
                    &[payer(), attacker_nonce],
                    &create_data(payer(), "bitone-nonce", lamports, 80, SYSTEM_PROGRAM),
                ),
                initialize(payer(), attacker_nonce),
            ],
            // 种子,大小,所有者不对.
            vec![
                ix(
                    SYSTEM_PROGRAM,
                    &[payer(), nonce()],
                    &create_data(payer(), "other", lamports, 80, SYSTEM_PROGRAM),
                ),
                initialize(payer(), nonce()),
            ],
            vec![
                ix(
                    SYSTEM_PROGRAM,
                    &[payer(), nonce()],
                    &create_data(payer(), "bitone-nonce", lamports, 81, SYSTEM_PROGRAM),
                ),
                initialize(payer(), nonce()),
            ],
            vec![
                ix(
                    SYSTEM_PROGRAM,
                    &[payer(), nonce()],
                    &create_data(payer(), "bitone-nonce", lamports, 80, VICTIM_PROGRAM),
                ),
                initialize(payer(), nonce()),
            ],
            // 只有一半.
            vec![fund(), allocate()],
            vec![allocate(), initialize(payer(), nonce())],
            vec![initialize(payer(), nonce())],
            // 中间插了指令.
            vec![fund(), allocate(), transfer(1), initialize(payer(), nonce())],
        ];
        for (i, instructions) in cases.iter().enumerate() {
            let review = check(&s, &compose(payer(), &[], instructions));
            assert!(review.risk >= Risk::Unknown, "第 {i} 例:{:?}", review.risk);
            assert_eq!(review.confirm, Confirm::Hold, "第 {i} 例");
        }
    }

    #[test]
    fn approval_hidden_behind_a_transfer_decides_the_category() {
        let s = setup();
        let message = compose(payer(), &[], &[advance(), transfer(1), approve(u64::MAX)]);
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Approval, Confirm::Hold));
        assert_eq!(review.counterparty_label.as_deref(), Some("被授权方"));
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&STRANGER)).as_deref()
        );
        assert!(review.amount.as_ref().unwrap().unlimited);
        assert_eq!(review.actions.len(), 2);
        assert_eq!(review.actions[0].risk, Risk::Transfer);
        assert_eq!(review.actions[1].risk, Risk::Approval);
    }

    #[test]
    fn order_does_not_matter_for_the_category() {
        let s = setup();
        let message = compose(payer(), &[], &[advance(), approve(1), transfer(1)]);
        assert_eq!(check(&s, &message).risk, Risk::Approval);
        let mut assign = vec![1, 0, 0, 0];
        assign.extend_from_slice(&STRANGER);
        let message = compose(
            payer(),
            &[],
            &[
                advance(),
                approve(1),
                ix(SYSTEM_PROGRAM, &[payer()], &assign),
                transfer(1),
            ],
        );
        assert_eq!(check(&s, &message).risk, Risk::PermissionChange);
    }

    #[test]
    fn approve_checked_shows_the_token_when_listed() {
        let s = setup();
        let mint = key(USDC);
        let approve = |mint| {
            ix(
                TOKEN_PROGRAM,
                &[key(SOURCE_ATA), mint, STRANGER, payer()],
                &token_data(13, 2_000_000, Some(6)),
            )
        };
        let review = check(&s, &compose(payer(), &[], &[advance(), approve(mint)]));
        assert_eq!(review.risk, Risk::Approval);
        let amount = review.amount.as_ref().unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol.as_deref()), ("2", Some("USDC")));
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&STRANGER)).as_deref()
        );

        let review = check(&s, &compose(payer(), &[], &[advance(), approve([0x77; 32])]));
        assert_eq!(review.risk, Risk::Approval);
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("2000000", None));
    }

    /// 链上程序解码时忽略尾部字节,这几条指令带上尾部字节一样执行,不能因此降级.
    #[test]
    fn trailing_bytes_do_not_downgrade_dangerous_instructions() {
        let s = setup();
        let mut data = token_data(4, 5, None);
        data.push(0xaa);
        let tailed = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), STRANGER, payer()], &data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), tailed])).risk,
            Risk::Approval
        );

        let mut data = token_data(13, 5, Some(6));
        data.push(0xaa);
        let tailed = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), key(USDC), STRANGER, payer()], &data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), tailed])).risk,
            Risk::Approval
        );

        let mut data = vec![6, 2, 1];
        data.extend_from_slice(&STRANGER);
        data.push(0xaa);
        let tailed = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), payer()], &data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), tailed])).risk,
            Risk::PermissionChange
        );

        let mut data = vec![1, 0, 0, 0];
        data.extend_from_slice(&STRANGER);
        data.push(0xaa);
        let tailed = ix(SYSTEM_PROGRAM, &[payer()], &data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), tailed])).risk,
            Risk::PermissionChange
        );

        let mut data = vec![7, 0, 0, 0];
        data.extend_from_slice(&STRANGER);
        data.push(0xaa);
        let tailed = ix(SYSTEM_PROGRAM, &[nonce(), payer()], &data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), tailed])).risk,
            Risk::PermissionChange
        );
    }

    #[test]
    fn revoke_and_close_are_unknown() {
        let s = setup();
        let revoke = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), payer()], &[5]);
        let review = check(&s, &compose(payer(), &[], &[advance(), revoke]));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let close = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), STRANGER, payer()], &[9]);
        let review = check(&s, &compose(payer(), &[], &[advance(), close]));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
    }

    #[test]
    fn set_authority_shows_both_controllers() {
        let s = setup();
        let mut data = vec![6, 2, 1];
        data.extend_from_slice(&STRANGER);
        for program in [TOKEN_PROGRAM, TOKEN_2022_PROGRAM] {
            let set = ix(program, &[key(SOURCE_ATA), payer()], &data);
            let review = check(&s, &compose(payer(), &[], &[advance(), set]));
            assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
            assert_eq!(review.counterparty_label.as_deref(), Some("新控制方"));
            assert_eq!(
                review.counterparty.as_deref(),
                Some(bitcoin::base58::encode(&STRANGER)).as_deref()
            );
            assert!(action_has(&review, "当前控制方", PAYER));
            assert!(action_has(&review, "权限类型", "AccountOwner"));
        }
        // 新权限为空:权限被永久移除,同样是权限变更.
        let none = ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), payer()], &[6, 3, 0]);
        let review = check(&s, &compose(payer(), &[], &[advance(), none]));
        assert_eq!(review.risk, Risk::PermissionChange);
    }

    #[test]
    fn assign_and_authorize_nonce_are_permission_changes() {
        let s = setup();
        let mut assign = vec![1, 0, 0, 0];
        assign.extend_from_slice(&VICTIM_PROGRAM);
        let review = check(
            &s,
            &compose(payer(), &[], &[advance(), ix(SYSTEM_PROGRAM, &[payer()], &assign)]),
        );
        assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
        assert!(action_has(&review, "当前控制方", PAYER));
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&VICTIM_PROGRAM)).as_deref()
        );

        let mut authorize = vec![7, 0, 0, 0];
        authorize.extend_from_slice(&STRANGER);
        let review = check(
            &s,
            &compose(
                payer(),
                &[],
                &[advance(), ix(SYSTEM_PROGRAM, &[nonce(), payer()], &authorize)],
            ),
        );
        assert_eq!(review.risk, Risk::PermissionChange);
        assert!(action_has(&review, "nonce 账户", NONCE));
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&STRANGER)).as_deref()
        );
    }

    #[test]
    fn assign_with_seed_is_a_permission_change() {
        let s = setup();
        let mut assign = vec![10, 0, 0, 0];
        assign.extend_from_slice(&STRANGER);
        assign.extend_from_slice(&5u64.to_le_bytes());
        assign.extend_from_slice(b"vault");
        assign.extend_from_slice(&VICTIM_PROGRAM);
        let accounts = [key(PEER), payer()];
        let reviewed = |data: &[u8]| {
            let message = compose(payer(), &[], &[advance(), ix(SYSTEM_PROGRAM, &accounts, data)]);
            check(&s, &message)
        };
        let review = reviewed(&assign);
        assert_eq!((review.risk, review.confirm), (Risk::PermissionChange, Confirm::Hold));
        assert!(action_has(&review, "当前控制方", PEER));
        // 新所有者在 seed 后面,开头的 32 字节是 base.
        assert_eq!(
            review.counterparty.as_deref(),
            Some(bitcoin::base58::encode(&VICTIM_PROGRAM)).as_deref()
        );
        let mut trailing = assign.clone();
        trailing.push(0);
        assert_eq!(reviewed(&trailing).risk, Risk::PermissionChange);

        // 布局读不完的指令节点会拒绝,不能把 base 当成新所有者.
        assert_eq!(reviewed(&assign[..assign.len() - 1]).risk, Risk::Unknown);
        let mut huge = assign.clone();
        huge[36..44].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(reviewed(&huge).risk, Risk::Unknown);
    }

    #[test]
    fn advance_must_be_first() {
        let s = setup();
        let message = compose(payer(), &[], &[transfer(1), advance()]);
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
        assert_eq!(review.actions[1].title, "AdvanceNonceAccount");
        assert_eq!(row(&review, "nonce 账户"), "无");
    }

    #[test]
    fn second_advance_is_not_free() {
        let s = setup();
        let review = check(&s, &compose(payer(), &[], &[advance(), advance(), transfer(1)]));
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(review.alerts.len(), 1);
    }

    #[test]
    fn advance_of_a_foreign_nonce_is_unknown() {
        let s = setup();
        let cases = [
            // nonce 账户是别人的:对方能控制这笔交易什么时候生效.
            ix(
                SYSTEM_PROGRAM,
                &[[0x66; 32], RECENT_BLOCKHASHES_SYSVAR, payer()],
                &[4, 0, 0, 0],
            ),
            // authority 不是付款账户.
            ix(
                SYSTEM_PROGRAM,
                &[nonce(), RECENT_BLOCKHASHES_SYSVAR, STRANGER],
                &[4, 0, 0, 0],
            ),
            // 系统变量不对,账户数不对,数据有尾部字节.
            ix(SYSTEM_PROGRAM, &[nonce(), RENT_SYSVAR, payer()], &[4, 0, 0, 0]),
            ix(
                SYSTEM_PROGRAM,
                &[nonce(), RECENT_BLOCKHASHES_SYSVAR, payer(), STRANGER],
                &[4, 0, 0, 0],
            ),
            ix(
                SYSTEM_PROGRAM,
                &[nonce(), RECENT_BLOCKHASHES_SYSVAR, payer()],
                &[4, 0, 0, 0, 1],
            ),
        ];
        for (i, bad) in cases.into_iter().enumerate() {
            let review = check(&s, &compose(payer(), &[], &[bad, transfer(1)]));
            assert_eq!(
                (review.risk, review.confirm),
                (Risk::Unknown, Confirm::Hold),
                "第 {i} 例"
            );
            assert_eq!(review.alerts.len(), 1, "第 {i} 例");
        }
    }

    #[test]
    fn valid_advance_does_not_change_the_category() {
        let s = setup();
        let with = check(&s, &compose(payer(), &[], &[advance(), transfer(1)]));
        let without = check(&s, &compose(payer(), &[], &[transfer(1)]));
        assert_eq!(with.risk, Risk::Transfer);
        assert_eq!(without.risk, Risk::Transfer);
        assert_eq!(with.actions.len(), 1);
        assert_eq!(row(&with, "nonce 账户"), NONCE);
        assert_eq!(row(&without, "nonce 账户"), "无");
    }

    #[test]
    fn only_a_nonce_advance_is_never_safe() {
        let s = setup();
        assert_eq!(check(&s, &compose(payer(), &[], &[advance()])).confirm, Confirm::Hold);
        assert_eq!(check(&s, &compose(payer(), &[], &[])).risk, Risk::Unknown);
    }

    #[test]
    fn fee_payer_must_be_this_wallet() {
        let s = setup();
        let message = compose(STRANGER, &[], &[transfer(1)]);
        assert_eq!(forbidden(check(&s, &message)), Some(Forbid::AddressMismatch));
        let result = s.wallet.sign(
            &request(&s, message.encode()),
            &s.keys,
            Manifest::builtin(),
            HighestChange::default(),
        );
        assert_eq!(result.unwrap_err(), Error::Refused);
    }

    #[test]
    fn being_a_second_signer_is_not_enough() {
        let s = setup();
        let message = compose(STRANGER, &[payer()], &[transfer(1)]);
        assert_eq!(forbidden(check(&s, &message)), Some(Forbid::AddressMismatch));
    }

    #[test]
    fn other_required_signers_raise_an_alert() {
        let s = setup();
        let message = compose(payer(), &[STRANGER], &[advance(), transfer(1)]);
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
        assert_eq!(
            row(&review, "全部签名者"),
            format!("{PAYER}\n{}", bitcoin::base58::encode(&STRANGER))
        );
    }

    #[test]
    fn token_outside_the_manifest_is_unknown_with_a_raw_amount() {
        let s = setup();
        let fake = [0x77; 32];
        let message = compose(payer(), &[], &[advance(), transfer_checked(TOKEN_PROGRAM, fake, 6)]);
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1250000", None));
    }

    #[test]
    fn wrong_decimals_are_not_a_plain_transfer() {
        let s = setup();
        let message = compose(
            payer(),
            &[],
            &[advance(), transfer_checked(TOKEN_PROGRAM, key(USDC), 9)],
        );
        let review = check(&s, &message);
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(review.amount.unwrap().symbol, None);
    }

    #[test]
    fn mainnet_mint_is_not_listed_on_devnet() {
        let s = setup();
        let mut params = token_params(false);
        params.network = Network::SolanaDevnet;
        params.from = "9BbGG7UMnNRJGMk1m52F4Q4gCSUnSVBosiT9kJhmgWvQ";
        let req = SignRequest {
            network: "solana-devnet".into(),
            paths: vec![DEVNET_PATH.into()],
            ..request(&s, build_transfer(&params).unwrap())
        };
        assert_eq!(review(&s, &req).risk, Risk::Unknown);
    }

    #[test]
    fn token_2022_transfer_checked_is_never_plain() {
        let s = setup();
        let mint = key(USDC);
        let message = compose(
            payer(),
            &[],
            &[advance(), transfer_checked(TOKEN_2022_PROGRAM, mint, 6)],
        );
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1250000", None));
    }

    #[test]
    fn multisig_transfer_checked_is_not_plain() {
        let s = setup();
        let mut bad = transfer_checked(TOKEN_PROGRAM, key(USDC), 6);
        bad.accounts.push(STRANGER);
        assert_eq!(check(&s, &compose(payer(), &[], &[advance(), bad])).risk, Risk::Unknown);
    }

    #[test]
    fn legacy_token_transfer_is_unknown() {
        let s = setup();
        let legacy = ix(
            TOKEN_PROGRAM,
            &[key(SOURCE_ATA), key(PEER_ATA), payer()],
            &token_data(3, 1_250_000, None),
        );
        let review = check(&s, &compose(payer(), &[], &[advance(), legacy]));
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1250000", None));
        assert_eq!(review.counterparty.as_deref(), Some(PEER_ATA));
    }

    #[test]
    fn malformed_transfers_are_not_plain() {
        let s = setup();
        let mut tailed = transfer(1);
        tailed.data.push(0);
        let mut extra = transfer(1);
        extra.accounts.push(STRANGER);
        let short = ix(SYSTEM_PROGRAM, &[payer(), key(PEER)], &[2, 0, 0, 0, 1]);
        let mut checked_tail = transfer_checked(TOKEN_PROGRAM, key(USDC), 6);
        checked_tail.data.push(0);
        for (i, bad) in [tailed, extra, short, checked_tail].into_iter().enumerate() {
            assert_eq!(
                check(&s, &compose(payer(), &[], &[advance(), bad])).risk,
                Risk::Unknown,
                "第 {i} 例"
            );
        }
    }

    #[test]
    fn several_transfers_need_a_long_press() {
        let s = setup();
        let review = check(
            &s,
            &compose(payer(), &[], &[advance(), transfer(1), transfer(2_000_000_000)]),
        );
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
        assert_eq!(review.actions.len(), 2);
        assert!(action_has(&review, "金额", "2 SOL"));
        // 两笔都不能单独当标题.
        assert_eq!((review.amount, review.counterparty), (None, None));
    }

    /// 同类动作并列最高时,第一个不能盖住后面的.
    #[test]
    fn equal_actions_do_not_hide_behind_the_first() {
        let s = setup();
        let review = check(&s, &compose(payer(), &[], &[advance(), approve(1), approve(u64::MAX)]));
        assert_eq!(review.risk, Risk::Approval);
        assert_eq!(review.alerts.len(), 1);
        assert_eq!((review.amount, review.counterparty), (None, None));

        let set = |kind: u8, new: [u8; 32]| {
            let mut data = vec![6, kind, 1];
            data.extend_from_slice(&new);
            ix(TOKEN_PROGRAM, &[key(SOURCE_ATA), payer()], &data)
        };
        let review = check(
            &s,
            &compose(payer(), &[], &[advance(), set(3, payer()), set(2, STRANGER)]),
        );
        assert_eq!(review.risk, Risk::PermissionChange);
        assert_eq!(review.alerts.len(), 1);
        assert_eq!(review.counterparty, None);
    }

    /// 账户多一个或数据多一个字节,节点照样转账,已在 solana-test-validator 上确认.
    #[test]
    fn irregular_transfers_still_show_the_amount() {
        let s = setup();
        let mut extra = transfer(1_000_000_000_000);
        extra.accounts.push(STRANGER);
        let review = check(&s, &compose(payer(), &[], &[advance(), transfer(1), extra]));
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(review.alerts.len(), 1);
        assert!(action_has(&review, "金额", "1000 SOL"));
        // 风险最高的是这一笔,顶部显示它.
        assert_eq!(review.amount.unwrap().text, "1000");

        let mut tailed = transfer_checked(TOKEN_PROGRAM, key(USDC), 6);
        tailed.data.push(0);
        let review = check(&s, &compose(payer(), &[], &[advance(), tailed]));
        assert_eq!(review.risk, Risk::Unknown);
        let amount = review.amount.unwrap();
        assert_eq!((amount.text.as_str(), amount.symbol), ("1250000", None));
        assert_eq!(review.counterparty.as_deref(), Some(PEER_ATA));

        let mut multisig = transfer_checked(TOKEN_PROGRAM, key(USDC), 6);
        multisig.accounts.push(STRANGER);
        let review = check(&s, &compose(payer(), &[], &[advance(), multisig]));
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(review.amount.unwrap().symbol, None);
    }

    /// 无法识别的指令可能动用更多资金,小额转账不能占住顶部.
    #[test]
    fn small_transfer_does_not_headline_over_an_unknown_instruction() {
        let s = setup();
        let unknown = ix(VICTIM_PROGRAM, &[payer(), STRANGER], &[1, 2, 3]);
        let review = check(&s, &compose(payer(), &[], &[advance(), transfer(1), unknown]));
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!((review.amount, review.counterparty), (None, None));
    }

    #[test]
    fn several_token_accounts_need_a_long_press() {
        let s = setup();
        let accounts = ata_accounts(key(PEER_ATA), TOKEN_PROGRAM);
        let create = || ix(ASSOCIATED_TOKEN_PROGRAM, &accounts, &[1]);
        let one = check(&s, &compose(payer(), &[], &[advance(), create()]));
        assert_eq!((one.risk, one.confirm), (Risk::Recognized, Confirm::Tap));
        let two = check(&s, &compose(payer(), &[], &[advance(), create(), create()]));
        assert_eq!((two.risk, two.confirm), (Risk::Recognized, Confirm::Hold));
        assert_eq!(two.alerts.len(), 1);
    }

    #[test]
    fn unknown_program_shows_everything() {
        let s = setup();
        let mut message = compose(
            payer(),
            &[STRANGER],
            &[
                advance(),
                ix(
                    VICTIM_PROGRAM,
                    &[STRANGER, key(PEER), payer()],
                    &[0xde, 0xad, 0xbe, 0xef],
                ),
            ],
        );
        message.readonly_signed = 1;
        message.readonly_unsigned = 2;
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        let rows = &review.actions[0].rows;
        let find = |label: &str| rows.iter().find(|r| r.label == label).map(|r| r.value.as_str());
        assert_eq!(
            find("程序 ID"),
            Some(bitcoin::base58::encode(&VICTIM_PROGRAM)).as_deref()
        );
        assert_eq!(
            find("账户 1(签名者,只读)"),
            Some(bitcoin::base58::encode(&STRANGER)).as_deref()
        );
        assert_eq!(find("账户 2(可写)"), Some(PEER));
        assert_eq!(find("账户 3(签名者,可写)"), Some(PAYER));
        assert_eq!(find("数据长度"), Some("4 字节"));
        assert_eq!(find("数据"), Some("deadbeef"));
        assert_eq!(review.raw[0].note, None);
    }

    fn with_lookups(mut message: Message, count: u8) -> Message {
        message.version = Version::V0;
        message.lookups = vec![Lookup {
            table: [0x88; 32],
            writable: (0..count).collect(),
            readonly: Vec::new(),
        }];
        message
    }

    #[test]
    fn lookup_accounts_make_the_instruction_unknown() {
        let s = setup();
        let mut message = with_lookups(compose(payer(), &[], &[advance(), transfer(1)]), 2);
        let statics = message.keys.len() as u8;
        message.instructions[1].accounts[1] = statics;
        let review = check(&s, &message);
        assert_eq!((review.risk, review.confirm), (Risk::Unknown, Confirm::Hold));
        assert_eq!(row(&review, "地址查找表"), "2 个地址无法离线解析");
        assert_eq!(row(&review, "消息版本"), "v0");
        assert!(action_has(&review, "账户 2(可写)", "查找表地址"));
    }

    #[test]
    fn lookups_cannot_hide_a_token_recipient() {
        let s = setup();
        let mut message = with_lookups(
            compose(
                payer(),
                &[],
                &[advance(), transfer_checked(TOKEN_PROGRAM, key(USDC), 6)],
            ),
            1,
        );
        let statics = message.keys.len() as u8;
        message.instructions[1].accounts[2] = statics;
        assert_eq!(check(&s, &message).risk, Risk::Unknown);
    }

    #[test]
    fn lookups_cannot_downgrade_an_approval() {
        let s = setup();
        let mut message = with_lookups(compose(payer(), &[], &[advance(), approve(1)]), 1);
        let statics = message.keys.len() as u8;
        message.instructions[1].accounts[1] = statics;
        let review = check(&s, &message);
        assert_eq!(review.risk, Risk::Approval);
        assert_eq!(review.counterparty.as_deref(), Some("查找表地址"));
    }

    #[test]
    fn unreferenced_lookup_table_is_still_unknown() {
        let s = setup();
        let message = with_lookups(compose(payer(), &[], &[advance(), transfer(1)]), 3);
        let review = check(&s, &message);
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(row(&review, "地址查找表"), "3 个地址无法离线解析");
    }

    #[test]
    fn v0_without_lookups_is_treated_like_legacy() {
        let s = setup();
        let mut message = compose(payer(), &[], &[advance(), transfer(1)]);
        message.version = Version::V0;
        let review = check(&s, &message);
        assert_eq!(review.risk, Risk::Transfer);
        assert_eq!(row(&review, "消息版本"), "v0");
    }

    #[test]
    fn priority_fee_is_micro_lamports_rounded_up() {
        let s = setup();
        let fee = |instructions: &[Ix]| {
            let review = check(&s, &compose(payer(), &[], instructions));
            (row(&review, "优先费").to_string(), review.alerts.len())
        };
        // 1 微 lamport 乘 1 个单元,向上取整到 1 lamport.
        assert_eq!(
            fee(&[compute_limit(1), compute_price(1), transfer(1)]).0,
            "0.000000001 SOL"
        );
        // 1000 微 lamports 乘 200000 单元是 200 lamports.
        assert_eq!(
            fee(&[compute_limit(200_000), compute_price(1000), transfer(1)]).0,
            "0.0000002 SOL"
        );
        // 没写单元数时按每条指令 200000 算.
        assert_eq!(fee(&[compute_price(1000), transfer(1)]).0, "0.0000004 SOL");
        // 单元数超过协议上限时按上限算.
        assert_eq!(
            fee(&[compute_limit(u32::MAX), compute_price(1000), transfer(1)]).0,
            "0.0000014 SOL"
        );
        // 没有单价就没有优先费.
        assert_eq!(fee(&[compute_limit(1_000_000), transfer(1)]).0, "0 SOL");
    }

    #[test]
    fn fee_over_the_ceiling_needs_a_long_press() {
        let s = setup();
        // 基础费 5000 lamports,优先费 9995000 lamports,正好 0.01 SOL.
        let at = compose(
            payer(),
            &[],
            &[compute_limit(1_000_000), compute_price(9_995_000), transfer(1)],
        );
        let review = check(&s, &at);
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Tap));
        let over = compose(
            payer(),
            &[],
            &[compute_limit(1_000_000), compute_price(9_995_001), transfer(1)],
        );
        let review = check(&s, &over);
        assert_eq!((review.risk, review.confirm), (Risk::Transfer, Confirm::Hold));
        assert_eq!(review.alerts.len(), 1);
        // 乘积超过 64 位时不能回绕成小数字,否则告警会被绕过.
        let wrapped = ((1u128 << 64) * 10 / 14 + 1) as u64;
        for price in [u64::MAX, wrapped] {
            let huge = compose(
                payer(),
                &[],
                &[compute_limit(1_400_000), compute_price(price), transfer(1)],
            );
            let review = check(&s, &huge);
            assert_eq!(review.confirm, Confirm::Hold, "{price}");
            assert_eq!(review.alerts.len(), 1, "{price}");
        }
    }

    #[test]
    fn signature_count_is_in_the_base_fee() {
        let s = setup();
        let review = check(&s, &compose(payer(), &[STRANGER], &[transfer(1)]));
        assert_eq!(row(&review, "基础费"), "0.00001 SOL");
    }

    #[test]
    fn other_compute_budget_instructions_are_unknown() {
        let s = setup();
        // 请求堆内存不是这里认识的指令.
        let heap = ix(COMPUTE_BUDGET_PROGRAM, &[], &[1, 0, 0, 1, 0]);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[heap, transfer(1)])).risk,
            Risk::Unknown
        );
    }

    #[test]
    fn irregular_compute_budget_is_unknown_but_still_counts_for_the_fee() {
        let s = setup();
        let mut tailed = compute_price(1000);
        tailed.data.push(0);
        let review = check(
            &s,
            &compose(payer(), &[], &[compute_limit(200_000), tailed, transfer(1)]),
        );
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(row(&review, "优先费"), "0.0000002 SOL");

        let review = check(
            &s,
            &compose(
                payer(),
                &[],
                &[
                    compute_limit(200_000),
                    compute_price(1),
                    compute_price(1000),
                    transfer(1),
                ],
            ),
        );
        assert_eq!(review.risk, Risk::Unknown);
        assert_eq!(row(&review, "优先费"), "0.0000002 SOL");

        let with_account = ix(COMPUTE_BUDGET_PROGRAM, &[STRANGER], &compute_price(1).data);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[with_account, transfer(1)])).risk,
            Risk::Unknown
        );
    }

    fn ata_accounts(account: [u8; 32], token_program: [u8; 32]) -> [[u8; 32]; 6] {
        [payer(), account, key(PEER), key(USDC), SYSTEM_PROGRAM, token_program]
    }

    #[test]
    fn associated_account_creation_is_recognized() {
        let s = setup();
        let accounts = ata_accounts(key(PEER_ATA), TOKEN_PROGRAM);
        for data in [&[][..], &[0], &[1]] {
            let create = ix(ASSOCIATED_TOKEN_PROGRAM, &accounts, data);
            let review = check(&s, &compose(payer(), &[], &[advance(), create]));
            assert_eq!(review.risk, Risk::Recognized, "{data:?}");
            assert!(action_has(&review, "所有者", PEER));
            assert!(action_has(&review, "代币", USDC));
        }
    }

    #[test]
    fn irregular_associated_account_instructions_are_unknown() {
        let s = setup();
        let good = ata_accounts(key(PEER_ATA), TOKEN_PROGRAM);
        let wrong_program = ata_accounts(key(PEER_ATA), VICTIM_PROGRAM);
        let wrong_account = ata_accounts(STRANGER, TOKEN_PROGRAM);
        let cases = [
            ix(ASSOCIATED_TOKEN_PROGRAM, &good, &[2]),
            ix(ASSOCIATED_TOKEN_PROGRAM, &good[..5], &[1]),
            ix(ASSOCIATED_TOKEN_PROGRAM, &wrong_program, &[1]),
            ix(ASSOCIATED_TOKEN_PROGRAM, &wrong_account, &[1]),
        ];
        for (i, bad) in cases.into_iter().enumerate() {
            assert_eq!(
                check(&s, &compose(payer(), &[], &[advance(), bad])).risk,
                Risk::Unknown,
                "第 {i} 例"
            );
        }
    }

    #[test]
    fn other_system_instructions_are_unknown() {
        let s = setup();
        let withdraw = ix(
            SYSTEM_PROGRAM,
            &[nonce(), STRANGER, payer()],
            &[5, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), withdraw])).risk,
            Risk::Unknown
        );
        let short = ix(SYSTEM_PROGRAM, &[payer()], &[1]);
        assert_eq!(
            check(&s, &compose(payer(), &[], &[advance(), short])).risk,
            Risk::Unknown
        );
    }

    #[test]
    fn malformed_messages_cannot_be_signed() {
        let s = setup();
        let mut tail = native();
        tail.push(0);
        let mut out_of_range = compose(payer(), &[], &[transfer(1)]);
        out_of_range.instructions[0].accounts[0] = 200;
        for payload in [tail, Vec::new(), vec![1, 0, 0], out_of_range.encode()] {
            let req = request(&s, payload);
            assert_eq!(forbidden(review(&s, &req)), Some(Forbid::ParseFailed));
            assert_eq!(
                s.wallet
                    .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
                    .unwrap_err(),
                Error::Refused
            );
        }
    }

    /// 随机改字节,删字节,插字节.不能 panic,能进入确认的消息必须能原样重新编码.
    #[test]
    fn mutated_messages_never_panic() {
        let s = setup();
        let seeds = [
            native(),
            token(true),
            build_create_nonce(PAYER, BLOCKHASH, 1_447_680).unwrap(),
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
            if review(&s, &request(&s, payload.clone())).is_signable() {
                signable += 1;
                assert_eq!(Message::decode(&payload).unwrap().encode(), payload);
            }
        }
        assert!(signable > 0);
    }

    #[test]
    fn message_signing_is_refused() {
        let s = setup();
        let mut req = request(&s, native());
        req.kind = KIND_TYPED_DATA;
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::MessageSigning));
    }

    #[test]
    fn path_rules() {
        let s = setup();
        let mut req = request(&s, native());
        req.paths = vec!["m/44'/501'/1'/0'".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
        req.paths = vec!["m/44'/60'/0'/0/0".into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![DEVNET_PATH.into()];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNetworkMismatch));
        req.paths = vec![];
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::PathNotAllowed));
    }

    #[test]
    fn devnet_uses_its_own_key() {
        let s = setup();
        let mut params = native_params();
        params.network = Network::SolanaDevnet;
        // 主网地址在 Devnet 路径下对不上.
        let req = SignRequest {
            network: "solana-devnet".into(),
            paths: vec![DEVNET_PATH.into()],
            ..request(&s, build_transfer(&params).unwrap())
        };
        assert_eq!(forbidden(review(&s, &req)), Some(Forbid::AddressMismatch));
        params.from = "9BbGG7UMnNRJGMk1m52F4Q4gCSUnSVBosiT9kJhmgWvQ";
        let req = SignRequest {
            payload: build_transfer(&params).unwrap(),
            ..req
        };
        assert_eq!(review(&s, &req).risk, Risk::Transfer);
        let (response, _) = sign(&s, &req);
        let signer = s.keys.ed25519(&DerivationPath::parse(DEVNET_PATH).unwrap()).unwrap();
        tx::verify(&req, &response, &signer).unwrap();
    }

    #[test]
    fn verify_rejects_tampering() {
        use bitone_core::Error as Core;
        let s = setup();
        let req = request(&s, native());
        let (response, _) = sign(&s, &req);
        let signer = payer();
        tx::verify(&req, &response, &signer).unwrap();

        let mut other = req.clone();
        other.payload = {
            let mut params = native_params();
            params.amount += 1;
            build_transfer(&params).unwrap()
        };
        assert_eq!(tx::verify(&other, &response, &signer).unwrap_err(), Core::BadSignature);

        let mut wrong_id = response.clone();
        wrong_id.id = [1; 16];
        assert_eq!(
            tx::verify(&req, &wrong_id, &signer).unwrap_err(),
            Core::ResponseMismatch
        );

        // 不是手续费支付者的公钥.
        assert_eq!(tx::verify(&req, &response, &key(PEER)).unwrap_err(), Core::BadSignature);
        assert_eq!(tx::verify(&req, &response, &[1u8; 31]).unwrap_err(), Core::InvalidKey);

        let mut short = response.clone();
        short.signatures = vec![vec![0u8; 65].into()];
        assert_eq!(tx::verify(&req, &short, &signer).unwrap_err(), Core::BadSignature);

        let mut two = response.clone();
        two.signatures.push(two.signatures[0].clone());
        assert_eq!(tx::verify(&req, &two, &signer).unwrap_err(), Core::BadSignature);

        let mut flipped = response.clone();
        let mut bytes = flipped.signatures[0].to_vec();
        bytes[10] ^= 1;
        flipped.signatures = vec![bytes.into()];
        assert_eq!(tx::verify(&req, &flipped, &signer).unwrap_err(), Core::BadSignature);

        let mut broken = req.clone();
        broken.payload.push(0);
        assert_eq!(
            tx::verify(&broken, &response, &signer).unwrap_err(),
            Core::InvalidParams
        );
    }

    #[test]
    fn verify_refuses_a_partly_signed_message() {
        let s = setup();
        let message = compose(payer(), &[STRANGER], &[advance(), transfer(1)]);
        let req = request(&s, message.encode());
        let (response, review) = sign(&s, &req);
        assert_eq!(review.confirm, Confirm::Hold);
        assert_eq!(
            tx::verify(&req, &response, &payer()).unwrap_err(),
            bitone_core::Error::InvalidParams
        );
    }

    #[test]
    fn signature_is_deterministic() {
        let s = setup();
        let req = request(&s, native());
        let first = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap()
            .0;
        let second = s
            .wallet
            .sign(&req, &s.keys, Manifest::builtin(), HighestChange::default())
            .unwrap()
            .0;
        assert_eq!(first, second);
    }
}

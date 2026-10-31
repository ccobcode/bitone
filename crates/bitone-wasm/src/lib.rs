//! 网页端的 WASM 绑定。这里不依赖 `bitone-signer`，构建产物里没有任何签名代码。

use bitone_core::keys::PublicKeys;
use bitone_core::manifest::{Manifest, VENDOR_KEY};
use bitone_core::path::account_specs;
use bitone_core::protocol::{
    self, AccountsExport, AddressCheck, DEFAULT_FRAGMENT_BYTES, ExportedAccount, FrameDecoder, FrameEncoder,
    UR_ACCOUNTS, UR_MANIFEST, UR_REQUEST,
};
use bitone_core::request::{KIND_TRANSACTION, SignRequest, SignResponse};
use bitone_core::tx::{self, HighestChange, ReviewContext, bitcoin, evm, solana, tron};
use bitone_core::{AccountKind, Chain, Mode, Network, address, qr};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

fn js_error(error: bitone_core::Error) -> JsError {
    JsError::new(&error.to_string())
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(value).map_err(|e| JsError::new(&e.to_string()))
}

fn network(id: &str) -> Result<Network, JsError> {
    Network::from_id(id).ok_or_else(|| JsError::new("未知网络"))
}

fn chain_name(chain: Chain) -> &'static str {
    match chain {
        Chain::Bitcoin => "bitcoin",
        Chain::Evm => "evm",
        Chain::Tron => "tron",
        Chain::Solana => "solana",
    }
}

fn kind_name(kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::SegWit => "segwit",
        AccountKind::Taproot => "taproot",
        AccountKind::NestedSegWit => "nested-segwit",
        AccountKind::Legacy => "legacy",
        AccountKind::Single => "single",
    }
}

fn kind_from_name(name: &str) -> Result<AccountKind, JsError> {
    match name {
        "segwit" => Ok(AccountKind::SegWit),
        "taproot" => Ok(AccountKind::Taproot),
        "nested-segwit" => Ok(AccountKind::NestedSegWit),
        "legacy" => Ok(AccountKind::Legacy),
        _ => Err(JsError::new("未知账户类型")),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkInfo {
    id: &'static str,
    chain: &'static str,
    testnet: bool,
    chain_id: Option<u64>,
}

/// 网络表内置在网页端，不从网关获取，见 docs/04-security.md 第 5 节。
#[wasm_bindgen]
pub fn networks() -> Result<JsValue, JsError> {
    let list: Vec<NetworkInfo> = Network::ALL
        .into_iter()
        .map(|n| NetworkInfo {
            id: n.id(),
            chain: chain_name(n.chain()),
            testnet: n.mode() == Mode::Testnet,
            chain_id: n.evm_chain_id(),
        })
        .collect();
    to_js(&list)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    chain: &'static str,
    kind: &'static str,
    path: String,
    /// 这个账户用在哪：`mainnet`、`testnet` 或两者都用的 `both`。网页端按它分开主网和测试网。
    scope: &'static str,
    #[serde(with = "serde_bytes_array")]
    key: Vec<u8>,
}

mod serde_bytes_array {
    use serde::Serializer;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Pairing {
    fingerprint: String,
    accounts: Vec<Account>,
}

fn scope_of(path: &str) -> &'static str {
    if path.starts_with("m/44'/60'") {
        "both"
    } else if path.contains("/1'/") {
        "testnet"
    } else {
        "mainnet"
    }
}

fn pairing(export: AccountsExport) -> Result<Pairing, bitone_core::Error> {
    let accounts = export
        .accounts
        .into_iter()
        .map(|a| {
            Ok(Account {
                chain: chain_name(protocol::chain_from_code(a.chain)?),
                kind: kind_name(protocol::kind_from_code(a.kind)?),
                scope: scope_of(&a.path),
                path: a.path,
                key: a.key,
            })
        })
        .collect::<Result<Vec<_>, bitone_core::Error>>()?;
    Ok(Pairing {
        fingerprint: export.fingerprint.iter().map(|b| format!("{b:02X}")).collect(),
        accounts,
    })
}

/// 扫码接收器。每扫到一帧调用一次 `receive`，收齐后读取结果。
#[wasm_bindgen]
#[derive(Default)]
pub struct FrameReceiver {
    decoder: FrameDecoder,
}

#[wasm_bindgen]
impl FrameReceiver {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回进度，0 到 1。
    pub fn receive(&mut self, part: &str) -> Result<f32, JsError> {
        self.decoder.receive(part).map_err(js_error)?;
        Ok(self.decoder.progress())
    }

    pub fn complete(&self) -> bool {
        self.decoder.complete()
    }

    #[wasm_bindgen(js_name = urType)]
    pub fn ur_type(&self) -> Option<String> {
        self.decoder.ur_type().map(String::from)
    }

    /// 收齐后的原始消息。
    pub fn message(&self) -> Result<Vec<u8>, JsError> {
        self.decoder.message().map_err(js_error)
    }

    /// 解析配对数据。不是观察钱包二维码时报错。
    pub fn accounts(&self) -> Result<JsValue, JsError> {
        if self.decoder.ur_type() != Some(UR_ACCOUNTS) {
            return Err(js_error(bitone_core::Error::UnknownUrType));
        }
        let message = self.decoder.message().map_err(js_error)?;
        let export = AccountsExport::decode(&message).map_err(js_error)?;
        to_js(&pairing(export).map_err(js_error)?)
    }
}

/// 从账户扩展公钥派生 Bitcoin 地址。
#[wasm_bindgen(js_name = bitcoinAddress)]
pub fn bitcoin_address(xpub: &[u8], kind: &str, change: bool, index: u32, network_id: &str) -> Result<String, JsError> {
    address::bitcoin_address(xpub, kind_from_name(kind)?, change, index, network(network_id)?).map_err(js_error)
}

/// EVM、TRON、Solana 账户公钥对应的地址。
#[wasm_bindgen(js_name = accountAddress)]
pub fn account_address(chain: &str, key: &[u8]) -> Result<String, JsError> {
    match chain {
        "evm" => address::evm_address(key),
        "tron" => address::tron_address(key),
        "solana" => address::solana_address(key),
        _ => return Err(JsError::new("未知链")),
    }
    .map_err(js_error)
}

#[derive(Serialize)]
struct Spec {
    chain: &'static str,
    kind: &'static str,
    path: &'static str,
}

/// 这个模式下的账户，顺序就是显示顺序。
#[wasm_bindgen(js_name = accountSpecs)]
pub fn specs(testnet: bool) -> Result<JsValue, JsError> {
    let mode = if testnet { Mode::Testnet } else { Mode::Mainnet };
    let list: Vec<Spec> = account_specs(mode, true)
        .into_iter()
        .map(|s| Spec {
            chain: chain_name(s.chain),
            kind: kind_name(s.kind),
            path: s.path,
        })
        .collect();
    to_js(&list)
}

/// 地址核对二维码。路径与网络冲突时报错。`fingerprint` 是 8 位十六进制。
#[wasm_bindgen(js_name = addressFrame)]
pub fn address_frame(fingerprint: &str, network_id: &str, path: &str, address: &str) -> Result<String, JsError> {
    let digits = (fingerprint.len() == 8)
        .then(|| u32::from_str_radix(fingerprint, 16).ok())
        .flatten();
    let fingerprint = digits.ok_or_else(|| JsError::new("指纹格式错误"))?.to_be_bytes();
    AddressCheck::new(fingerprint, network(network_id)?, path, address)
        .and_then(|check| check.frame())
        .map_err(js_error)
}

#[derive(Serialize)]
struct Matrix {
    size: u32,
    modules: Vec<bool>,
}

fn matrix_to_js(matrix: qr::QrMatrix) -> Result<JsValue, JsError> {
    to_js(&Matrix {
        size: matrix.size,
        modules: matrix.modules,
    })
}

/// UR 帧的二维码模块矩阵，由页面自己画出来。
#[wasm_bindgen(js_name = qrMatrix)]
pub fn qr_matrix(part: &str) -> Result<JsValue, JsError> {
    matrix_to_js(qr::matrix(part).map_err(js_error)?)
}

/// 普通文本的二维码，比如给付款方扫的收款地址。
#[wasm_bindgen(js_name = qrText)]
pub fn qr_text(text: &str) -> Result<JsValue, JsError> {
    matrix_to_js(qr::text_matrix(text).map_err(js_error)?)
}

/// 内置清单。网页端和冷钱包用同一份，这里是唯一来源。
#[wasm_bindgen]
pub fn assets() -> Result<JsValue, JsError> {
    to_js(&Manifest::builtin().assets)
}

/// 校验收款地址属于这个网络，返回规范写法。
#[wasm_bindgen(js_name = validateAddress)]
pub fn validate_address(network_id: &str, text: &str) -> Result<String, JsError> {
    address::validate(network(network_id)?, text).map_err(js_error)
}

/// 分帧发送。`ur_type` 是 `request` 或 `manifest`。
#[wasm_bindgen]
pub struct FrameSender {
    encoder: FrameEncoder,
}

#[wasm_bindgen]
impl FrameSender {
    #[wasm_bindgen(constructor)]
    pub fn new(message: &[u8], ur_type: &str) -> Result<FrameSender, JsError> {
        let ur_type = match ur_type {
            "request" => UR_REQUEST,
            "manifest" => UR_MANIFEST,
            _ => return Err(js_error(bitone_core::Error::UnknownUrType)),
        };
        let encoder = FrameEncoder::new(message, ur_type, DEFAULT_FRAGMENT_BYTES).map_err(js_error)?;
        Ok(Self { encoder })
    }

    #[wasm_bindgen(js_name = next)]
    pub fn next_part(&mut self) -> Result<String, JsError> {
        self.encoder.next_part().map_err(js_error)
    }

    #[wasm_bindgen(getter)]
    pub fn count(&self) -> usize {
        self.encoder.fragment_count()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestHeader {
    /// 由页面用 `crypto.getRandomValues` 生成。WASM 里不放随机数。
    #[serde(with = "serde_bytes")]
    id: Vec<u8>,
    fingerprint: String,
    network: String,
}

impl RequestHeader {
    fn request(&self, paths: Vec<String>, payload: Vec<u8>, hint: Option<String>) -> Result<Vec<u8>, JsError> {
        let id = self
            .id
            .as_slice()
            .try_into()
            .map_err(|_| JsError::new("请求 ID 必须是 16 字节"))?;
        Ok(SignRequest {
            version: protocol::VERSION,
            id,
            fingerprint: parse_fingerprint(&self.fingerprint)?,
            network: self.network.clone(),
            kind: KIND_TRANSACTION,
            paths,
            payload,
            hint,
        }
        .encode())
    }
}

fn parse_fingerprint(text: &str) -> Result<[u8; 4], JsError> {
    let digits = (text.len() == 8).then(|| u32::from_str_radix(text, 16).ok()).flatten();
    Ok(digits.ok_or_else(|| JsError::new("指纹格式错误"))?.to_be_bytes())
}

fn parse<T: for<'de> Deserialize<'de>>(args: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(args).map_err(|e| JsError::new(&e.to_string()))
}

fn number(text: &str) -> Result<u128, JsError> {
    text.parse().map_err(|_| JsError::new("数值格式错误"))
}

fn hex32(text: &str) -> Result<[u8; 32], JsError> {
    address::parse_solana_address(text).map_err(js_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvmArgs {
    #[serde(flatten)]
    header: RequestHeader,
    path: String,
    to: String,
    amount: String,
    contract: Option<String>,
    nonce: u64,
    gas_limit: u64,
    max_fee: String,
    max_priority: String,
}

#[wasm_bindgen(js_name = buildEvmTransfer)]
pub fn build_evm_transfer(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: EvmArgs = parse(args)?;
    let tx = evm::build_transfer(&evm::TransferParams {
        network: network(&args.header.network)?,
        to: &args.to,
        amount: &args.amount,
        contract: args.contract.as_deref(),
        nonce: args.nonce,
        gas_limit: args.gas_limit,
        max_fee: number(&args.max_fee)?,
        max_priority: number(&args.max_priority)?,
    })
    .map_err(js_error)?;
    args.header.request(vec![args.path], tx.encode(), None)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TronArgs {
    #[serde(flatten)]
    header: RequestHeader,
    path: String,
    from: String,
    to: String,
    amount: String,
    contract: Option<String>,
    ref_block_bytes: String,
    ref_block_hash: String,
    expiration_ms: u64,
    timestamp_ms: u64,
    fee_limit: u64,
}

fn unhex<const N: usize>(text: &str) -> Result<[u8; N], JsError> {
    let bytes = (0..text.len() / 2)
        .map(|i| {
            text.get(i * 2..i * 2 + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(|| JsError::new("十六进制格式错误"))?;
    bytes.try_into().map_err(|_| JsError::new("十六进制长度错误"))
}

#[wasm_bindgen(js_name = buildTronTransfer)]
pub fn build_tron_transfer(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: TronArgs = parse(args)?;
    let payload = tron::build_transfer(&tron::TransferParams {
        network: network(&args.header.network)?,
        from: &args.from,
        to: &args.to,
        amount: &args.amount,
        contract: args.contract.as_deref(),
        ref_block_bytes: unhex(&args.ref_block_bytes)?,
        ref_block_hash: unhex(&args.ref_block_hash)?,
        expiration_ms: args.expiration_ms,
        timestamp_ms: args.timestamp_ms,
        fee_limit: args.fee_limit,
    })
    .map_err(js_error)?;
    args.header.request(vec![args.path], payload, None)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolanaArgs {
    #[serde(flatten)]
    header: RequestHeader,
    path: String,
    from: String,
    to: String,
    amount: u64,
    mint: Option<String>,
    decimals: u8,
    nonce_value: String,
    priority_micro_lamports: u64,
    compute_units: u32,
    create_recipient_ata: bool,
}

#[wasm_bindgen(js_name = buildSolanaTransfer)]
pub fn build_solana_transfer(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: SolanaArgs = parse(args)?;
    let payload = solana::build_transfer(&solana::TransferParams {
        network: network(&args.header.network)?,
        from: &args.from,
        to: &args.to,
        amount: args.amount,
        mint: args.mint.as_deref(),
        decimals: args.decimals,
        nonce_value: hex32(&args.nonce_value)?,
        priority_micro_lamports: args.priority_micro_lamports,
        compute_units: args.compute_units,
        create_recipient_ata: args.create_recipient_ata,
    })
    .map_err(js_error)?;
    let hint = args.mint.is_some().then(|| args.to.clone());
    args.header.request(vec![args.path], payload, hint)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolanaNonceArgs {
    #[serde(flatten)]
    header: RequestHeader,
    path: String,
    from: String,
    blockhash: String,
    lamports: u64,
}

#[wasm_bindgen(js_name = buildSolanaCreateNonce)]
pub fn build_solana_create_nonce(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: SolanaNonceArgs = parse(args)?;
    let payload = solana::build_create_nonce(&args.from, hex32(&args.blockhash)?, args.lamports).map_err(js_error)?;
    args.header.request(vec![args.path], payload, None)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolanaAdvanceArgs {
    #[serde(flatten)]
    header: RequestHeader,
    path: String,
    from: String,
    nonce_value: String,
}

/// 只推进 nonce，用同一个 nonce 值签过名的交易随之作废。
#[wasm_bindgen(js_name = buildSolanaAdvanceNonce)]
pub fn build_solana_advance_nonce(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: SolanaAdvanceArgs = parse(args)?;
    let payload = solana::build_advance_nonce(&args.from, hex32(&args.nonce_value)?).map_err(js_error)?;
    args.header.request(vec![args.path], payload, None)
}

/// 本钱包的 nonce 账户地址。
#[wasm_bindgen(js_name = solanaNonceAddress)]
pub fn solana_nonce_address(payer: &str) -> Result<String, JsError> {
    let key = hex32(payer)?;
    let nonce = solana::nonce_account_address(&key).map_err(js_error)?;
    address::solana_address(&nonce).map_err(js_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UtxoArgs {
    txid: String,
    vout: u32,
    amount: u64,
    path: String,
    #[serde(default, with = "serde_bytes")]
    prev_tx: Option<Vec<u8>>,
}

impl From<UtxoArgs> for bitcoin::Utxo {
    fn from(utxo: UtxoArgs) -> Self {
        Self {
            txid: utxo.txid,
            vout: utxo.vout,
            amount: utxo.amount,
            path: utxo.path,
            prev_tx: utxo.prev_tx,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectedUtxo {
    txid: String,
    vout: u32,
    amount: u64,
    path: String,
}

#[derive(Serialize)]
struct Selected {
    inputs: Vec<SelectedUtxo>,
    fee: u64,
    change: u64,
}

/// 选币。选出后页面要用 `raw-txs` 取回前序交易，再调用 `buildBitcoinTransfer`。
#[wasm_bindgen(js_name = selectBitcoinCoins)]
pub fn select_bitcoin_coins(utxos: JsValue, amount: u64, fee_rate: u64, change_kind: &str) -> Result<JsValue, JsError> {
    let list: Vec<UtxoArgs> = parse(utxos)?;
    let list: Vec<bitcoin::Utxo> = list.into_iter().map(Into::into).collect();
    let selection = bitcoin::select_coins(&list, amount, fee_rate, kind_from_name(change_kind)?).map_err(js_error)?;
    to_js(&Selected {
        inputs: selection
            .inputs
            .into_iter()
            .map(|u| SelectedUtxo {
                txid: u.txid,
                vout: u.vout,
                amount: u.amount,
                path: u.path,
            })
            .collect(),
        fee: selection.fee,
        change: selection.change,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeeInfo {
    fee: u64,
    vsize: u64,
}

/// PSBT 的手续费和估算的虚拟大小。
#[wasm_bindgen(js_name = bitcoinFeeInfo)]
pub fn bitcoin_fee_info(request: &[u8]) -> Result<JsValue, JsError> {
    let request = SignRequest::decode(request).map_err(js_error)?;
    let (fee, vsize) = bitcoin::fee_info(network(&request.network)?, &request).map_err(js_error)?;
    to_js(&FeeInfo { fee, vsize })
}

#[derive(Serialize)]
struct Replaced {
    fee: u64,
    change: u64,
}

/// RBF 用同一组输入提高手续费。`amount` 为 0 表示取消。
#[wasm_bindgen(js_name = bitcoinReplacement)]
pub fn bitcoin_replacement(
    total: u64,
    amount: u64,
    old_fee: u64,
    vsize: u64,
    fee_rate: u64,
    change_kind: &str,
) -> Result<JsValue, JsError> {
    let result = bitcoin::replacement(total, amount, old_fee, vsize, fee_rate, kind_from_name(change_kind)?)
        .map_err(js_error)?;
    to_js(&Replaced {
        fee: result.fee,
        change: result.change,
    })
}

/// ERC-20 的 transfer 调用数据，十六进制，用于估算 Gas。
#[wasm_bindgen(js_name = evmTransferData)]
pub fn evm_transfer_data(to: &str, amount: &str) -> Result<String, JsError> {
    let recipient = address::parse_evm_address(to).map_err(js_error)?;
    let amount = evm::be_from_decimal(amount).map_err(js_error)?;
    let data = evm::transfer_data(&recipient, &amount);
    Ok(format!(
        "0x{}",
        data.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

/// 校验清单包，返回版本号。冷钱包导入时还会再校验一次。
#[wasm_bindgen(js_name = verifyManifestPackage)]
pub fn verify_manifest_package(package: &[u8], current_version: u32) -> Result<u32, JsError> {
    Ok(Manifest::from_package(package, &VENDOR_KEY, current_version)
        .map_err(js_error)?
        .version)
}

/// 界面上显示的网络名。
#[wasm_bindgen(js_name = networkLabel)]
pub fn network_label(id: &str) -> Result<String, JsError> {
    Ok(network(id)?.label().to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyArgs {
    chain: String,
    kind: String,
    path: String,
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
}

fn chain_from_name(name: &str) -> Result<Chain, JsError> {
    match name {
        "bitcoin" => Ok(Chain::Bitcoin),
        "evm" => Ok(Chain::Evm),
        "tron" => Ok(Chain::Tron),
        "solana" => Ok(Chain::Solana),
        _ => Err(JsError::new("未知链")),
    }
}

fn kind_from_any(name: &str) -> Result<AccountKind, JsError> {
    if name == "single" {
        Ok(AccountKind::Single)
    } else {
        kind_from_name(name)
    }
}

fn public_keys(fingerprint: &str, keys: Vec<KeyArgs>) -> Result<PublicKeys, JsError> {
    let accounts = keys
        .into_iter()
        .map(|k| {
            Ok(ExportedAccount {
                chain: protocol::chain_code(chain_from_name(&k.chain)?),
                kind: protocol::kind_code(kind_from_any(&k.kind)?),
                path: k.path,
                key: k.key,
            })
        })
        .collect::<Result<Vec<_>, JsError>>()?;
    let export = AccountsExport {
        version: protocol::VERSION,
        fingerprint: parse_fingerprint(fingerprint)?,
        accounts,
    };
    PublicKeys::new(&[export]).map_err(js_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitcoinArgs {
    #[serde(flatten)]
    header: RequestHeader,
    keys: Vec<KeyArgs>,
    inputs: Vec<UtxoArgs>,
    /// 留空表示取消，全部回到找零。
    to: Option<String>,
    #[serde(default)]
    amount: u64,
    change_path: Option<String>,
    #[serde(default)]
    change: u64,
}

#[wasm_bindgen(js_name = buildBitcoinTransfer)]
pub fn build_bitcoin_transfer(args: JsValue) -> Result<Vec<u8>, JsError> {
    let args: BitcoinArgs = parse(args)?;
    let net = network(&args.header.network)?;
    let keys = public_keys(&args.header.fingerprint, args.keys)?;
    let inputs: Vec<bitcoin::Utxo> = args.inputs.into_iter().map(Into::into).collect();
    let amount = args.amount;
    let payments: Vec<bitcoin::Payment> = args
        .to
        .into_iter()
        .map(|address| bitcoin::Payment { address, amount })
        .collect();
    let change = args.change_path.as_deref().map(|path| (path, args.change));
    let payload = bitcoin::build_psbt(net, &keys, &inputs, &payments, change).map_err(js_error)?;
    args.header.request(Vec::new(), payload, None)
}

/// 用和冷钱包相同的代码预览核对结果。这里的结果只用于提示，判断以冷钱包屏幕为准。
#[wasm_bindgen(js_name = previewRequest)]
pub fn preview_request(request: &[u8], keys: JsValue) -> Result<JsValue, JsError> {
    let request = SignRequest::decode(request).map_err(js_error)?;
    let fingerprint: String = request.fingerprint.iter().map(|b| format!("{b:02X}")).collect();
    let keys = public_keys(&fingerprint, parse(keys)?)?;
    let wide = u32::MAX / 2;
    let ctx = ReviewContext {
        keys: &keys,
        manifest: Manifest::builtin(),
        highest_change: HighestChange {
            mainnet: wide,
            testnet: wide,
        },
    };
    to_js(&tx::review(&request, &ctx))
}

#[derive(Serialize)]
struct Checked {
    raw: String,
    hash: String,
}

/// 广播前核验。用公钥验证签名，并装回构建记录，见 docs/03-system.md 第 8 节。
#[wasm_bindgen(js_name = verifyResponse)]
pub fn verify_response(request: &[u8], response: &[u8], signer: &[u8]) -> Result<JsValue, JsError> {
    let request = SignRequest::decode(request).map_err(js_error)?;
    let response = SignResponse::decode(response).map_err(js_error)?;
    let verified = tx::verify(&request, &response, signer).map_err(js_error)?;
    to_js(&Checked {
        raw: verified.raw.iter().map(|b| format!("{b:02x}")).collect(),
        hash: verified.hash,
    })
}

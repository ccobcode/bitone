//! EVM:核对,签名原文,构建和核验.解码在 `decode`,这里算出策略要的事实.
//!
//! 第 1.1 节:本机认识的网络和 Chain ID 矛盾时拒绝,其余内容都可以签,解码和规则只负责让用户看清楚.

use crate::address::{eth_address_bytes, evm_text, parse_evm_address};
use crate::decode::{
    self, Coin, Decoded, Decoder, EvmPayload, EvmWalker, Node, PersonalWalker, RawHashWalker, Reject, Tx, TypedFlavor,
    TypedMessage, TypedWalker, parse_evm, parse_typed,
};
use crate::path::{DerivationPath, check_foreign_path, is_usual_evm_path};
use crate::policy::{self, CallFact, EvmFacts};
use crate::request::{KIND_PERSONAL_SIGN, KIND_RAW_HASH, KIND_TRANSACTION, KIND_TYPED_DATA, SignRequest, SignResponse};
use crate::review::{ActionView, AmountView, Forbid, RawLine, Review, Risk, Row};
use crate::tx::abi::{self, Style};
use crate::tx::{ReviewContext, Verified, check_signature, native_alert, raw_lines};
use crate::units::format_units;
use crate::{Chain, Error, Mode, Network};
use alloy_consensus::{SignableTransaction, Transaction};
use alloy_eips::Encodable2718;
use alloy_eips::eip7702::SignedAuthorization;
use alloy_primitives::{Address, Bytes, Signature, TxKind, U256, eip191_hash_message};
use bitcoin::hex::DisplayHex;
use sha3::{Digest, Keccak256};

pub(crate) mod typed;

/// 官方类型.构建和测试用.
pub mod types {
    pub use alloy_consensus::{TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};
    pub use alloy_eips::eip2930::{AccessList, AccessListItem};
    pub use alloy_eips::eip7702::{Authorization, SignedAuthorization};
    pub use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
    pub use alloy_rlp::encode as rlp_encode;
}

/// 派生路径的层数上限,BIP-32 的深度只有一个字节.
pub(super) const MAX_PATH_DEPTH: usize = 255;

/// 未签名的交易.只能由官方类型得到,所以解码,展示和签名看到的是同一份内容.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsignedTx(Tx);

impl UnsignedTx {
    pub fn new(tx: impl Into<Tx>) -> Self {
        Self(tx.into())
    }

    /// 只认交易.解码器保证重新编码回完全相同的字节.
    pub fn decode(payload: &[u8]) -> Result<Self, Forbid> {
        match parse_evm(payload).map_err(|_| Forbid::ParseFailed)? {
            EvmPayload::Transaction(tx) => Ok(Self(*tx)),
            EvmPayload::Authorization(_) | EvmPayload::Message | EvmPayload::Unknown => Err(Forbid::ParseFailed),
        }
    }

    /// 未签名编码,也就是签名哈希的原文.
    pub fn encode(&self) -> Vec<u8> {
        self.0.encoded_for_signing()
    }

    /// 装上签名,得到可以广播的原始交易.类型 0x03 是共识形式,边车由广播方附上.
    pub fn assemble(&self, signature: &[u8; 65]) -> Result<Vec<u8>, Error> {
        let signature = Signature::from_raw_array(signature).map_err(|_| Error::BadSignature)?;
        Ok(self.0.clone().into_envelope(signature).encoded_2718())
    }

    /// 最高手续费,单位是最小单位.类型 0x03 含 blob 费.
    pub fn fee_cap(&self) -> U256 {
        decode::fee_cap(&self.0)
    }

    pub fn chain_id(&self) -> Option<u64> {
        self.0.chain_id()
    }
}

pub fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// `eth_signTypedData_v4` 的哈希.
pub fn typed_data_hash(payload: &[u8]) -> Result<[u8; 32], Forbid> {
    let parsed = parse_typed(payload, TypedFlavor::Eip712).map_err(|_| Forbid::ParseFailed)?;
    Ok(parsed.digest.0)
}

/// 路径不设白名单.只有一条路径才知道用哪把私钥.
fn request_path(request: &SignRequest) -> Result<DerivationPath, Forbid> {
    let [path] = request.paths.as_slice() else {
        return Err(Forbid::PathNotAllowed);
    };
    let path = DerivationPath::parse(path).map_err(|_| Forbid::PathNotAllowed)?;
    if path.0.len() > MAX_PATH_DEPTH {
        return Err(Forbid::PathNotAllowed);
    }
    check_foreign_path(&path, Chain::Evm).map_err(|_| Forbid::PathNetworkMismatch)?;
    Ok(path)
}

/// 付款地址.没有导出的路径保存的公钥算不出来,页面显示路径并告警.
fn signer_address(ctx: &ReviewContext, path: &DerivationPath) -> Result<Option<String>, Forbid> {
    match ctx.keys.secp256k1(path) {
        Ok(key) => Ok(Some(evm_text(
            &eth_address_bytes(&key).map_err(|_| Forbid::AddressMismatch)?,
        ))),
        Err(Forbid::PathNotAllowed) => Ok(None),
        Err(reason) => Err(reason),
    }
}

/// 这笔签名属于哪个网络.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target<'a> {
    Known(Network),
    /// 谁也不认识的网络,这是请求里的名字.
    Foreign(&'a str),
}

impl Target<'_> {
    fn id(&self) -> &str {
        match self {
            Self::Known(network) => network.id(),
            Self::Foreign(name) => name,
        }
    }

    fn network(&self) -> Option<Network> {
        match self {
            Self::Known(network) => Some(*network),
            Self::Foreign(_) => None,
        }
    }
}

/// 载荷对网络的约束.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Binding {
    /// 交易的 Chain ID,和本机认识的网络不符是矛盾.
    required: Option<u64>,
    /// 签名绑定的链.没有时这个签名不限于一条链.
    bound: Option<u64>,
}

fn binding_of(payload: &EvmPayload) -> Binding {
    match payload {
        EvmPayload::Transaction(tx) => Binding {
            required: tx.chain_id(),
            bound: tx.chain_id(),
        },
        // 第 1.1 节:授权属于别的链只显示不拒绝.0 表示每条链.
        EvmPayload::Authorization(authorization) => Binding {
            required: None,
            bound: u64::try_from(authorization.chain_id).ok().filter(|id| *id != 0),
        },
        EvmPayload::Message | EvmPayload::Unknown => Binding::default(),
    }
}

/// 域的 `chainId` 和本机认识的网络不符时不算绑定,所以主网提示一直在.
fn typed_binding(parsed: &TypedMessage, declared: Option<Network>) -> Binding {
    let chain_id = parsed.chain_id().and_then(|id| u64::try_from(id).ok());
    Binding {
        required: None,
        bound: chain_id.filter(|id| declared.is_none_or(|network| network.evm_chain_id() == Some(*id))),
    }
}

/// 本机认识的网络和 Chain ID 不符时拒绝.请求的名字本机不认识时以 Chain ID 为准,编出来的名字不能改变显示.
fn resolve<'a>(declared: Option<Network>, name: &'a str, binding: Binding) -> Result<Target<'a>, Forbid> {
    match declared {
        Some(network) if binding.required.is_some_and(|id| network.evm_chain_id() != Some(id)) => {
            Err(Forbid::ChainIdMismatch)
        }
        Some(network) => Ok(Target::Known(network)),
        None => Ok(binding
            .bound
            .and_then(Network::from_evm_chain_id)
            .map_or(Target::Foreign(name), Target::Known)),
    }
}

/// EVM 的路径不分主网和测试网,这里看 Chain ID.没有 Chain ID 的签名不能确认是测试网.
fn mode_of(binding: Binding) -> Mode {
    match binding.bound.and_then(Network::from_evm_chain_id) {
        Some(network) => network.mode(),
        None => Mode::Mainnet,
    }
}

/// 页面顶部的摘要和旧字段,第 9 节删除.类别和告警都来自策略.
#[derive(Default)]
struct Summary {
    amount: Option<AmountView>,
    counterparty_label: Option<String>,
    counterparty: Option<String>,
    actions: Vec<ActionView>,
    rows: Vec<Row>,
    raw: Vec<RawLine>,
    /// 描述文件解出的字段.
    call: Vec<Node>,
    facts: EvmFacts,
}

pub(super) fn call_note(actions: &[ActionView]) -> Option<String> {
    let lines: Vec<String> = actions
        .iter()
        .map(|action| {
            let rows: Vec<String> = action
                .rows
                .iter()
                .map(|row| [row.label.as_str(), row.value.as_str()].join(" "))
                .collect();
            [action.title.as_str(), rows.join(",").as_str()].join(":")
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn native_amount(target: &Target<'_>, ctx: &ReviewContext, value: U256) -> AmountView {
    let raw = value.to_string();
    match target.network() {
        Some(network) => {
            let native = ctx.native(network);
            AmountView {
                text: format_units(&raw, native.decimals),
                symbol: Some(native.symbol.clone()),
                asset: Some(native.key()),
                unlimited: false,
            }
        }
        None => AmountView {
            text: raw,
            symbol: None,
            asset: None,
            unlimited: false,
        },
    }
}

fn delegation(authorization: &SignedAuthorization, signer: Option<&str>) -> ActionView {
    let current = decode::authority(authorization)
        .map(|address| address.to_string())
        .or(signer.map(str::to_string));
    let mut rows: Vec<Row> = current.iter().map(|text| Row::mono("当前控制方", text)).collect();
    rows.push(Row::mono("新控制方", authorization.address.to_string()));
    ActionView {
        risk: Risk::PermissionChange,
        title: "EIP-7702 账户委托".to_string(),
        rows,
    }
}

fn summarize(tx: &Tx, target: &Target<'_>, ctx: &ReviewContext, signer: Option<&str>) -> Summary {
    let value = tx.value();
    let data: &Bytes = tx.input();
    let style = Style {
        chain: Chain::Evm,
        network: target.id(),
        manifest: ctx.manifest,
        format: &evm_text,
    };
    let mut facts = EvmFacts::default();
    let classified = match (tx.kind(), data.is_empty()) {
        (TxKind::Call(to), true) => abi::Classified {
            risk: Risk::Transfer,
            title: Risk::Transfer.label().to_string(),
            amount: Some(native_amount(target, ctx, value)),
            counterparty_label: Some("收款地址".into()),
            counterparty: Some(to.to_string()),
            fact: CallFact::default(),
            fields: Vec::new(),
            inner: Vec::new(),
            rows: Vec::new(),
        },
        (TxKind::Call(to), false) => {
            let classified = abi::classify(&style, &to.into_array(), data, !value.is_zero());
            facts.calls = classified.facts();
            facts.call_note = call_note(&classified.actions());
            classified
        }
        (TxKind::Create, _) => abi::Classified {
            risk: Risk::Unknown,
            title: "创建合约".to_string(),
            amount: (!value.is_zero()).then(|| native_amount(target, ctx, value)),
            counterparty_label: None,
            counterparty: None,
            fact: CallFact::default(),
            fields: Vec::new(),
            inner: Vec::new(),
            rows: Vec::new(),
        },
    };
    let authorizations = tx.authorization_list().unwrap_or_default();
    let mut actions = classified.actions();
    // 有账户委托时页面顶部是新控制方,这笔交易本身的金额和收款方要放进列表.
    if !authorizations.is_empty() {
        actions[0] = classified.detailed();
    }
    actions.extend(
        authorizations
            .iter()
            .map(|authorization| delegation(authorization, signer)),
    );
    let new_controller = authorizations.last().map(|authorization| authorization.address);
    facts.fee_high = target
        .network()
        .is_some_and(|network| decode::fee_cap(tx) > U256::from(policy::fee_ceiling(network)));
    Summary {
        amount: classified.amount,
        counterparty_label: new_controller.map_or(classified.counterparty_label, |_| Some("新控制方".to_string())),
        counterparty: new_controller.map_or(classified.counterparty, |address: Address| Some(address.to_string())),
        actions,
        raw: raw_lines(data),
        call: classified.fields,
        facts,
        ..Summary::default()
    }
}

fn coin_of(target: &Target<'_>) -> Option<Coin> {
    target
        .network()
        .map(Network::native_coin)
        .map(|(unit, decimals)| Coin { unit, decimals })
}

/// 这笔请求的公共部分.
struct Reviewing<'a> {
    request: &'a SignRequest,
    ctx: &'a ReviewContext<'a>,
    path: DerivationPath,
    signer: Option<String>,
}

impl Reviewing<'_> {
    fn decoded(&self, decoder: &impl Decoder, target: &Target<'_>, mode: Mode) -> Result<Decoded, decode::Failure> {
        decode::decode(
            decoder,
            &decode::Input {
                chain: Chain::Evm,
                mode: Some(mode),
                network: target.id(),
                kind: self.request.kind,
                paths: &self.request.paths,
                payload: &self.request.payload,
                signer: self.signer.as_deref(),
            },
        )
    }

    fn rows(&self, extra: Vec<Row>) -> Vec<Row> {
        let signer = self.signer.iter().map(|signer| Row::mono("付款账户", signer));
        signer.chain(extra).collect()
    }

    fn finish(
        &self,
        decoded: Result<Decoded, decode::Failure>,
        target: &Target<'_>,
        binding: Binding,
        declared: Option<Network>,
        mut summary: Summary,
    ) -> Review {
        let decoded = match decoded {
            Ok(decoded) => decoded,
            Err(failure) => return Review::undecodable(&self.request.network, &failure),
        };
        let name = decode::escape_text(&self.request.network);
        summary.facts.signer.clone_from(&self.signer);
        summary.facts.unusual_path = (!is_usual_evm_path(&self.path)).then(|| self.path.to_string());
        summary.facts.renamed = match (declared, target.network()) {
            (None, Some(network)) => Some((name.clone(), network.label().to_string())),
            (Some(_) | None, Some(_) | None) => None,
        };
        summary.facts.unknown_chain = match (target, binding.required.or(binding.bound)) {
            (Target::Foreign(_), Some(chain_id)) => Some((chain_id.to_string(), name)),
            (Target::Foreign(_) | Target::Known(_), Some(_) | None) => None,
        };
        let assessment = policy::assess_evm(&decoded, &summary.facts);
        let review = Review {
            amount: summary.amount,
            counterparty_label: summary.counterparty_label,
            counterparty: summary.counterparty,
            actions: summary.actions,
            rows: self.rows(summary.rows),
            raw: summary.raw,
            mainnet: mode_of(binding) == Mode::Mainnet,
            ..Review::assessed(&decoded, assessment)
        };
        // 请求没有说明网络时清单还没查过.
        match (declared, target.network()) {
            (None, Some(network)) => native_alert(review, network, self.ctx),
            (Some(_) | None, Some(_) | None) => review,
        }
    }

    fn payload(&self, declared: Option<Network>) -> Result<Review, Forbid> {
        let parsed = parse_evm(&self.request.payload);
        let binding = parsed.as_ref().map_or_else(|_| Binding::default(), binding_of);
        let target = resolve(declared, &self.request.network, binding)?;
        let summary = match &parsed {
            Ok(EvmPayload::Transaction(tx)) => summarize(tx, &target, self.ctx, self.signer.as_deref()),
            Ok(EvmPayload::Authorization(_) | EvmPayload::Message | EvmPayload::Unknown) | Err(_) => Summary::default(),
        };
        let walker = EvmWalker {
            coin: coin_of(&target),
            chain_id: target.network().and_then(Network::evm_chain_id),
            call: &summary.call,
        };
        let decoded = self.decoded(&walker, &target, mode_of(binding));
        Ok(self.finish(decoded, &target, binding, declared, summary))
    }

    /// 第 5.1 节:任何消息都能解码和签名,域没有绑定的内容由规则告警.
    fn typed(&self, declared: Option<Network>) -> Result<Review, Forbid> {
        let parsed = match parse_typed(&self.request.payload, TypedFlavor::Eip712) {
            Ok(parsed) => parsed,
            Err(reason) => return Ok(self.refused(reason)),
        };
        let binding = typed_binding(&parsed, declared);
        let target = resolve(declared, &self.request.network, binding)?;
        let style = Style {
            chain: Chain::Evm,
            network: target.id(),
            manifest: self.ctx.manifest,
            format: &evm_text,
        };
        let view = typed::view(&parsed, &style);
        let labels = typed::labels(&parsed, &style);
        let walker = TypedWalker {
            parsed: &parsed,
            chain_id: target.network().and_then(Network::evm_chain_id),
            format: &evm_text,
            labels: labels.as_deref(),
        };
        let decoded = self.decoded(&walker, &target, mode_of(binding));
        let summary = Summary {
            amount: view.amount,
            counterparty_label: view.counterparty_label,
            counterparty: view.counterparty,
            actions: view.actions,
            rows: view.rows,
            facts: EvmFacts {
                permit: view.permit,
                ..EvmFacts::default()
            },
            ..Summary::default()
        };
        Ok(self.finish(decoded, &target, binding, declared, summary))
    }

    fn refused(&self, reason: Reject) -> Review {
        let failure = decode::Failure {
            reason,
            digest: decode::payload_digest(&self.request.payload),
        };
        Review::undecodable(&self.request.network, &failure)
    }

    /// 裸哈希和消息不绑定链,网络名只是声明.
    fn unbound(&self, decoder: &impl Decoder, declared: Option<Network>) -> Review {
        let target = declared.map_or(Target::Foreign(&self.request.network), Target::Known);
        let binding = Binding::default();
        let decoded = self.decoded(decoder, &target, mode_of(binding));
        self.finish(decoded, &target, binding, declared, Summary::default())
    }
}

pub fn review(declared: Option<Network>, request: &SignRequest, ctx: &ReviewContext) -> Result<Review, Forbid> {
    let path = request_path(request)?;
    let signer = signer_address(ctx, &path)?;
    let reviewing = Reviewing {
        request,
        ctx,
        path,
        signer,
    };
    match request.kind {
        KIND_TRANSACTION => reviewing.payload(declared),
        KIND_RAW_HASH => Ok(reviewing.unbound(&RawHashWalker, declared)),
        KIND_PERSONAL_SIGN => Ok(reviewing.unbound(&PersonalWalker, declared)),
        KIND_TYPED_DATA => reviewing.typed(declared),
        _ => Err(Forbid::Unsupported),
    }
}

/// 签名原文的哈希.`request` 已经过核对.未知的交易类型按 `keccak256(payload)` 计算.
pub fn signing_hash(request: &SignRequest) -> Result<[u8; 32], Forbid> {
    match request.kind {
        KIND_TRANSACTION => Ok(keccak(&request.payload)),
        KIND_TYPED_DATA => typed_data_hash(&request.payload),
        KIND_RAW_HASH => request.payload.as_slice().try_into().map_err(|_| Forbid::ParseFailed),
        KIND_PERSONAL_SIGN => Ok(eip191_hash_message(&request.payload).0),
        _ => Err(Forbid::Unsupported),
    }
}

pub struct TransferParams<'a> {
    pub network: Network,
    pub to: &'a str,
    /// 最小单位的十进制字符串.
    pub amount: &'a str,
    /// 代币合约.留空是原生币转账.
    pub contract: Option<&'a str>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee: u128,
    pub max_priority: u128,
}

/// 十进制字符串转大端字节,去掉前导 0.
pub fn be_from_decimal(text: &str) -> Result<Vec<u8>, Error> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidParams);
    }
    let mut bytes: Vec<u8> = vec![];
    for digit in text.bytes().map(|b| (b - b'0') as u32) {
        let mut carry = digit;
        for byte in bytes.iter_mut().rev() {
            let value = *byte as u32 * 10 + carry;
            *byte = value as u8;
            carry = value >> 8;
        }
        while carry > 0 {
            bytes.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    if bytes.len() > 32 {
        return Err(Error::InvalidParams);
    }
    Ok(bytes)
}

fn address_amount_data(selector: [u8; 4], address: &[u8; 20], amount: &[u8]) -> Vec<u8> {
    let mut data = selector.to_vec();
    data.extend_from_slice(&[0; 12]);
    data.extend_from_slice(address);
    data.resize(data.len() + 32 - amount.len(), 0);
    data.extend_from_slice(amount);
    data
}

/// ERC-20 的 transfer 调用数据.估算 Gas 时也要用.
pub fn transfer_data(recipient: &[u8; 20], amount: &[u8]) -> Vec<u8> {
    address_amount_data(abi::TRANSFER, recipient, amount)
}

/// ERC-20 的 approve 调用数据.额度为 0 就是撤销.
pub fn approve_data(spender: &[u8; 20], amount: &[u8]) -> Vec<u8> {
    address_amount_data(abi::APPROVE, spender, amount)
}

pub struct CallParams<'a> {
    pub network: Network,
    /// 留空是部署合约.
    pub to: Option<&'a str>,
    /// 最小单位的十进制字符串.
    pub value: &'a str,
    pub data: Vec<u8>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee: u128,
    pub max_priority: u128,
}

/// 构建 EIP-1559 调用,收款方和数据都不限制.
pub fn build_call(params: &CallParams) -> Result<UnsignedTx, Error> {
    let to = params.to.map(parse_evm_address).transpose()?;
    Ok(UnsignedTx::new(types::TxEip1559 {
        chain_id: params.network.evm_chain_id().ok_or(Error::InvalidParams)?,
        nonce: params.nonce,
        gas_limit: params.gas_limit,
        max_fee_per_gas: params.max_fee,
        max_priority_fee_per_gas: params.max_priority,
        to: to.map_or(TxKind::Create, |address| TxKind::Call(Address::from(address))),
        value: U256::from_be_slice(&be_from_decimal(params.value)?),
        access_list: types::AccessList::default(),
        input: params.data.clone().into(),
    }))
}

/// 构建 EIP-1559 转账.
pub fn build_transfer(params: &TransferParams) -> Result<UnsignedTx, Error> {
    let recipient = parse_evm_address(params.to)?;
    let amount = be_from_decimal(params.amount)?;
    let (to, value, data) = match params.contract {
        None => (params.to, params.amount, Vec::new()),
        Some(contract) => (contract, "0", transfer_data(&recipient, &amount)),
    };
    build_call(&CallParams {
        network: params.network,
        to: Some(to),
        value,
        data,
        nonce: params.nonce,
        gas_limit: params.gas_limit,
        max_fee: params.max_fee,
        max_priority: params.max_priority,
    })
}

/// 用公钥验证签名.交易装回构建记录,消息和未知的交易类型没有可以广播的形式,结果只有签名.
pub fn verify(request: &SignRequest, response: &SignResponse, signer: &[u8]) -> Result<Verified, Error> {
    let [signature] = response.signatures.as_slice() else {
        return Err(Error::BadSignature);
    };
    let signature: &[u8; 65] = signature.as_slice().try_into().map_err(|_| Error::BadSignature)?;
    let digest = signing_hash(request).map_err(|_| Error::InvalidParams)?;
    check_signature(&digest, signature, signer)?;
    let hex = |bytes: &[u8]| ["0x", bytes.to_lower_hex_string().as_str()].concat();
    let payload = (request.kind == KIND_TRANSACTION)
        .then(|| parse_evm(&request.payload))
        .transpose()
        .map_err(|_| Error::InvalidParams)?;
    match payload {
        Some(EvmPayload::Transaction(tx)) => {
            let raw = UnsignedTx(*tx).assemble(signature)?;
            let hash = hex(&keccak(&raw));
            Ok(Verified {
                raw,
                hash,
                fee_unconfirmed: false,
                signature_only: false,
            })
        }
        Some(EvmPayload::Authorization(_) | EvmPayload::Message | EvmPayload::Unknown) | None => Ok(Verified {
            raw: signature.to_vec(),
            hash: hex(&digest),
            fee_unconfirmed: false,
            signature_only: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

    fn call(to: Option<&str>, data: Vec<u8>) -> UnsignedTx {
        build_call(&CallParams {
            network: Network::Ethereum,
            to,
            value: "7",
            data,
            nonce: 3,
            gas_limit: 500_000,
            max_fee: 2,
            max_priority: 1,
        })
        .unwrap()
    }

    #[test]
    fn any_call_and_a_deployment_survive_the_strict_decoder() {
        for tx in [call(Some(PEER), vec![0xde, 0xad]), call(None, vec![0x60, 0x80])] {
            assert_eq!(UnsignedTx::decode(&tx.encode()), Ok(tx));
        }
        assert_eq!(call(None, Vec::new()).0.kind(), TxKind::Create);
        assert_eq!(call(Some(PEER), Vec::new()).0.value(), U256::from(7));
    }

    #[test]
    fn a_call_rejects_a_bad_address_and_a_network_without_chain_id() {
        let params = |network, to| CallParams {
            network,
            to,
            value: "0",
            data: Vec::new(),
            nonce: 0,
            gas_limit: 1,
            max_fee: 1,
            max_priority: 1,
        };
        assert!(build_call(&params(Network::Ethereum, Some("0x12"))).is_err());
        assert!(build_call(&params(Network::Tron, Some(PEER))).is_err());
    }

    #[test]
    fn approve_data_is_the_selector_the_spender_and_the_amount() {
        let spender = parse_evm_address(PEER).unwrap();
        let data = approve_data(&spender, &[1, 0]);
        assert_eq!(&data[..4], &abi::APPROVE);
        assert_eq!(data.len(), 68);
        assert_eq!(&data[16..36], &spender);
        assert_eq!(&data[66..], &[1, 0]);
        // 撤销就是额度为 0.
        assert_eq!(approve_data(&spender, &[])[36..], [0; 32]);
    }

    #[test]
    fn a_token_transfer_still_goes_to_the_contract_with_no_value() {
        let tx = build_transfer(&TransferParams {
            network: Network::Ethereum,
            to: PEER,
            amount: "9",
            contract: Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
            nonce: 0,
            gas_limit: 60_000,
            max_fee: 2,
            max_priority: 1,
        })
        .unwrap();
        assert!(tx.0.value().is_zero());
        assert_eq!(&tx.0.input()[..4], &abi::TRANSFER);
        assert_ne!(
            tx.0.kind(),
            TxKind::Call(Address::from(parse_evm_address(PEER).unwrap()))
        );
    }
}

// 构建待签名交易。协议和编码都在核心库里，这里只负责取数据、分配序号、写入记录。
// 顺序固定：在账户锁里取链上数据，分配序号，构建，预览，全部通过后才写入占用和 txs。
// 任何一步失败都什么也不写，所以 Forbidden 的交易不会留下记录，也不会有二维码。
import {
    accountKeyList,
    bitcoinFeeInfo,
    bitcoinReplacement,
    buildBitcoinTransfer,
    buildEvmTransfer,
    buildSolanaAdvanceNonce,
    buildSolanaCreateNonce,
    buildSolanaTransfer,
    buildTronTransfer,
    evmTransferData,
    previewRequest,
    selectCoins,
    solanaNonceAddress,
    type BitcoinReplacement,
    type CoinInput,
    type CoinSelection,
    type RequestHeader,
    type Review,
} from "../core";
import { lookupAsset, type AssetEntry } from "../core/manifest";
import {
    GatewayError,
    addressKey,
    broadcastRaw,
    estimateEvmGas,
    estimateSolana,
    estimateTron,
    fetchBlockhash,
    fetchFees,
    fetchNonceAccount,
    fetchNonces,
    fetchRawTxs,
    fetchRefBlock,
    fetchResources,
    fetchUtxos,
    type EvmTierFee,
    type FeeTier,
    type Fees,
    type TronEstimate,
    type TronResources,
} from "../gateway/client";
import {
    db,
    type Account,
    type BitcoinPlan,
    type PlanInput,
    type TxKind,
    type TxPlan,
    type TxRecord,
    type TxState,
    type Wallet,
} from "./db";
import { addressPath, ensureAddress } from "./pairing";
import {
    abandonedHolder,
    beginBroadcast,
    cancelTx,
    finishBroadcast,
    insertTx,
    nextNonce,
    planOf,
    reservationsOf,
    sharedClaims,
    withAccountLock,
    type Claim,
    type NewTx,
} from "./txs";
import { t } from "../core/i18n";

/** 网页端的 30 分钟只是手续费报价的有效期，见 docs/03-system.md 第 9 节。 */
export const QUOTE_LIFETIME = 30 * 60_000;
/** TRON 默认过期时间只有 60 秒，二维码往返来不及，构建时显式设置。 */
export const TRON_LIFETIME = 30 * 60_000;
/** blockhash 约 150 个 slot 后过期，这笔只有这么长的窗口。 */
export const SOLANA_BLOCKHASH_WINDOW = 60_000;
/** nonce 账户 80 字节的免租金额，docs/09 的示例就是这个值，账户关闭后可以赎回。 */
export const NONCE_RENT = 1_447_680n;
/** 引用区块的时间和本机差太多时不用，防止网关把签名有效期拉长。 */
const REF_BLOCK_DRIFT = 60 * 60_000;
const EVM_TRANSFER_GAS = 21_000n;

/** 与 docs/03 第 10 节一致：Solana 基础费是每个签名 5000 lamports。 */
export const SOLANA_BASE_FEE = 5000n;

/** 单元数都留了余量，超出会失败，多出的部分不收费以外的代价。 */
export function solanaComputeUnits(token: boolean, createAta: boolean): number {
    if (createAta) return 200_000;
    return token ? 50_000 : 10_000;
}

export type TransferErrorCode =
    | "forbidden"
    | "revert"
    | "insufficient"
    | "no-nonce-account"
    | "nonce-busy"
    | "nonce-held"
    | "duplicate"
    | "prev-tx"
    | "too-large"
    | "unsupported"
    | "invalid";

export class TransferError extends Error {
    readonly code: TransferErrorCode;
    readonly review?: Review;
    constructor(code: TransferErrorCode, message: string, review?: Review) {
        super(message);
        this.code = code;
        this.review = review;
    }
}

export interface TransferDraft {
    wallet: Wallet;
    /** 这个钱包的全部账户，核心库预览和构建 PSBT 时要用。 */
    accounts: Account[];
    account: Account;
    from: string;
    network: string;
    asset: string;
    entry: AssetEntry;
    to: string;
    amount: bigint;
    tier: FeeTier;
    /** 高级设置里手动填写的值。 */
    gasLimit?: bigint;
    feeLimit?: bigint;
}

/**
 * 同样的转账结果不明时不让再发。那一笔可能已经上链，新交易用的是下一个序号，两笔都会生效。
 * 重建走 createReplacement，复用原来的序号。
 */
async function rejectTwin({ account, network, asset, to, amount }: TransferDraft): Promise<void> {
    const rows = await db.txs.where("state").equals("uncertain").toArray();
    const twin = rows.find(
        (tx) =>
            tx.accountId === account.id &&
            tx.network === network &&
            addressKey(tx.asset) === addressKey(asset) &&
            addressKey(tx.to) === addressKey(to) &&
            tx.amount === amount.toString(),
    );
    if (twin) throw new TransferError("duplicate", t("同样的转账 {0} 结果不明，先处理那一笔。", twin.id));
}

export function randomRequestId(): Uint8Array {
    return crypto.getRandomValues(new Uint8Array(16));
}

function hex(bytes: Uint8Array): string {
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function bytesFromHex(text: string): Uint8Array {
    const bytes = new Uint8Array(text.length / 2);
    for (let i = 0; i < bytes.length; i++) bytes[i] = Number.parseInt(text.slice(i * 2, i * 2 + 2), 16);
    return bytes;
}

function header(wallet: Wallet, network: string): RequestHeader {
    return { id: randomRequestId(), fingerprint: wallet.fingerprint, network };
}

function reviewRequest(request: Uint8Array, accounts: Account[]): Review {
    let review: Review;
    try {
        review = previewRequest(request, accountKeyList(accounts));
    } catch (error) {
        if (error instanceof Error && error.message.includes("大小上限")) {
            throw new TransferError("too-large", t("交易数据超过二维码上限，请减少输入数量。"));
        }
        throw new TransferError("invalid", t("交易无法解析，已拒绝。"));
    }
    if (review.confirm === "refuse") {
        throw new TransferError("forbidden", review.alerts[0] ?? t("这笔交易不能签名。"), review);
    }
    return review;
}

function requestFields(
    draft: Pick<TransferDraft, "wallet" | "account" | "from" | "network" | "asset" | "to">,
    id: Uint8Array,
    request: Uint8Array,
    plan: TxPlan,
    amount: string,
    kind: TxKind,
    expiresAt: number | null,
    hardExpiry: boolean,
): NewTx {
    return {
        walletId: draft.wallet.id,
        accountId: draft.account.id,
        chain: draft.account.chain,
        network: draft.network,
        kind,
        asset: draft.asset,
        amount,
        from: draft.from,
        to: draft.to,
        request,
        requestId: hex(id),
        plan,
        expiresAt,
        hardExpiry,
    };
}

/** TRC-20 的调用数据。TRON 地址的编码在核心库里，没有单独的接口，所以从预览里读回来。 */
export function tronCallData(draft: Omit<TransferDraft, "tier">): string {
    const now = Date.now();
    const request = buildTronTransfer({
        ...header(draft.wallet, draft.network),
        path: draft.account.path,
        from: draft.from,
        to: draft.to,
        amount: draft.amount.toString(),
        contract: draft.asset,
        refBlockBytes: "0000",
        refBlockHash: "0000000000000000",
        expirationMs: now + TRON_LIFETIME,
        timestampMs: now,
        feeLimit: 1_000_000,
    });
    return reviewRequest(request, draft.accounts)
        .raw.map((line) => line.text)
        .join("");
}

/** 估算 Gas。普通转账就是 21000，合约调用留 20% 余量。 */
export async function evmGasLimit(draft: Omit<TransferDraft, "tier">): Promise<bigint> {
    if (draft.gasLimit !== undefined) return draft.gasLimit;
    const native = draft.asset === "native";
    try {
        const gas = await estimateEvmGas(draft.network, {
            from: draft.from,
            to: native ? draft.to : draft.asset,
            value: native ? draft.amount.toString() : "0",
            ...(native ? {} : { data: evmTransferData(draft.to, draft.amount.toString()) }),
        });
        return gas === EVM_TRANSFER_GAS ? gas : (gas * 12n) / 10n;
    } catch (error) {
        if (error instanceof GatewayError && error.code === "invalid_request") {
            throw new TransferError("revert", error.detail || t("这笔交易预计会失败。"));
        }
        throw error;
    }
}

async function createEvm(draft: TransferDraft): Promise<TxRecord> {
    const { account, network, from, to, amount, asset } = draft;
    return withAccountLock(account.id, async () => {
        await rejectTwin(draft);
        const [fees, nonces, gas] = await Promise.all([
            fetchFees(network),
            fetchNonces(network, [from]),
            evmGasLimit(draft),
        ]);
        if (fees.kind !== "evm") throw new TransferError("invalid", t("手续费数据异常，请稍后重试。"));
        const fee = fees[draft.tier];
        const chain = nonces.get(from);
        if (!chain) throw new TransferError("invalid", t("链上数据不完整，请稍后重试。"));
        const floor = Math.max(chain.pending, chain.latest);
        const held = abandonedHolder(await reservationsOf(account.id, network), await db.txs.toArray(), floor);
        if (held) throw new TransferError("nonce-held", t("交易 {0} 已取消但签名仍然有效，先发替换交易。", held.id));
        const nonce = await nextNonce(account.id, network, floor);
        const head = header(draft.wallet, network);
        const plan: TxPlan = {
            chain: "evm",
            nonce,
            gasLimit: Number(gas),
            maxFee: fee.maxFee.toString(),
            maxPriority: fee.priority.toString(),
        };
        const request = buildEvmTransfer({
            ...head,
            path: account.path,
            to,
            amount: amount.toString(),
            ...(asset === "native" ? {} : { contract: asset }),
            nonce,
            gasLimit: plan.gasLimit,
            maxFee: plan.maxFee,
            maxPriority: plan.maxPriority,
        });
        reviewRequest(request, draft.accounts);
        const fields = requestFields(
            draft,
            head.id,
            request,
            plan,
            amount.toString(),
            "transfer",
            Date.now() + QUOTE_LIFETIME,
            false,
        );
        return insertTx(fields, [{ type: "nonce", value: String(nonce) }]);
    });
}

export interface SpendableCoin extends CoinInput {
    address: string;
}

/** 可以花的 UTXO：已上链，属于这个账户，没有被待处理的交易占用。 */
export async function spendableCoins(account: Account, network: string): Promise<SpendableCoin[]> {
    const rows = await db.addresses.where("accountId").equals(account.id).toArray();
    const byAddress = new Map(rows.map((row) => [row.address, row]));
    const utxos = await fetchUtxos(network, [...byAddress.keys()]);
    const reservations = await reservationsOf(account.id, network);
    const taken = new Set(reservations.filter((row) => row.type === "utxo").map((row) => row.value));
    const coins = new Map<string, SpendableCoin>();
    for (const utxo of utxos) {
        const key = `${utxo.txid}:${utxo.vout}`;
        const row = byAddress.get(utxo.address);
        if (!row || utxo.height === 0 || taken.has(key) || utxo.amount > BigInt(Number.MAX_SAFE_INTEGER)) continue;
        coins.set(key, {
            txid: utxo.txid,
            vout: utxo.vout,
            amount: Number(utxo.amount),
            path: addressPath(account, row),
            address: row.address,
        });
    }
    return [...coins.values()];
}

function isTaprootPath(path: string): boolean {
    return path.startsWith("m/86'");
}

/** 除全 Taproot 外都要带前序交易，混合输入里的 Taproot 也一样，见 docs/04-security.md 第 2.7 节。 */
export function needsPrevTx(inputs: readonly { path: string }[]): boolean {
    return inputs.some((input) => !isTaprootPath(input.path));
}

/** 取回前序交易。txid 由核心库构建时重算，对不上会拒绝。 */
async function withPrevTx<T extends { txid: string; path: string }>(
    network: string,
    inputs: readonly T[],
): Promise<(T & { prevTx?: Uint8Array })[]> {
    if (!needsPrevTx(inputs)) return [...inputs];
    const raw = await fetchRawTxs(network, [...new Set(inputs.map((input) => input.txid))]);
    return inputs.map((input) => {
        const text = raw.get(input.txid);
        if (!text) throw new TransferError("prev-tx", t("链上数据不完整，请稍后重试。"));
        return { ...input, prevTx: bytesFromHex(text) };
    });
}

/** 核心库的错误原样给用户看，比如金额低于粉尘线。参数错误只可能来自网关给的前序交易。 */
function buildPsbt(args: Parameters<typeof buildBitcoinTransfer>[0]): Uint8Array {
    try {
        return buildBitcoinTransfer(args);
    } catch (error) {
        const message = error instanceof Error ? error.message : "";
        if (!message || message === "交易参数无效") {
            throw new TransferError("prev-tx", t("前序交易与输入不符，已拒绝构建。"));
        }
        throw new TransferError("invalid", `${message}。`);
    }
}

async function nextChangeIndex(account: Account, network: string, mode: Wallet["mode"]): Promise<number> {
    const rows = await db.addresses.where("accountId").equals(account.id).toArray();
    const used = new Set(rows.filter((row) => row.change === 1 && row.used).map((row) => row.index));
    const reservations = await reservationsOf(account.id, network);
    for (const row of reservations) if (row.type === "change") used.add(Number(row.value));
    let index = 0;
    while (used.has(index)) index += 1;
    await ensureAddress(account, mode, index, 1);
    return index;
}

function tierRate(fees: Fees, tier: FeeTier): bigint {
    if (fees.kind !== "bitcoin") throw new TransferError("invalid", t("手续费数据异常，请稍后重试。"));
    return fees[tier];
}

function pickCoins(coins: SpendableCoin[], amount: bigint, rate: bigint, account: Account): CoinSelection {
    try {
        return selectCoins(coins, amount, rate, account.kind);
    } catch {
        throw new TransferError("insufficient", t("可用余额不足，部分余额被待处理的交易占用。"));
    }
}

async function createBitcoin(draft: TransferDraft): Promise<TxRecord> {
    const { account, network, to, amount, wallet } = draft;
    return withAccountLock(account.id, async () => {
        await rejectTwin(draft);
        const rate = tierRate(await fetchFees(network), draft.tier);
        const coins = await spendableCoins(account, network);
        const selection = pickCoins(coins, amount, rate, account);
        const changeIndex = selection.change > 0 ? await nextChangeIndex(account, network, wallet.mode) : null;
        const changePath = changeIndex === null ? null : addressPath(account, { change: 1, index: changeIndex });
        const addresses = new Map(coins.map((coin) => [`${coin.txid}:${coin.vout}`, coin.address]));
        const inputs: PlanInput[] = selection.inputs.map((input) => ({
            ...input,
            address: addresses.get(`${input.txid}:${input.vout}`) ?? "",
        }));
        const head = header(wallet, network);
        const request = buildPsbt({
            ...head,
            keys: accountKeyList(draft.accounts),
            inputs: await withPrevTx(network, selection.inputs),
            to,
            amount: Number(amount),
            ...(changePath ? { changePath } : {}),
            change: selection.change,
        });
        reviewRequest(request, draft.accounts);
        const plan: BitcoinPlan = {
            chain: "bitcoin",
            inputs,
            changePath,
            changeIndex,
            change: selection.change,
            feeRate: Number(rate),
            ...bitcoinFeeInfo(request),
        };
        const claims: Claim[] = selection.inputs.map((input) => ({
            type: "utxo",
            value: `${input.txid}:${input.vout}`,
        }));
        if (changeIndex !== null) claims.push({ type: "change", value: String(changeIndex) });
        const fields = requestFields(
            draft,
            head.id,
            request,
            plan,
            amount.toString(),
            "transfer",
            Date.now() + QUOTE_LIFETIME,
            false,
        );
        return insertTx(fields, claims);
    });
}

export interface TronCost {
    /** 能量不足时燃烧的 TRX，单位 sun。 */
    energyBurn: bigint;
    /** 带宽不足时整笔按字节燃烧。 */
    bandwidthBurn: bigint;
    activationFee: bigint;
    total: bigint;
}

export function tronCost(
    estimate: TronEstimate,
    resources: TronResources,
    energyPrice: bigint,
    bandwidthPrice: bigint,
): TronCost {
    const missing = estimate.energy > resources.energy ? estimate.energy - resources.energy : 0n;
    const energyBurn = missing * energyPrice;
    const bandwidthBurn = estimate.bandwidth > resources.bandwidth ? estimate.bandwidth * bandwidthPrice : 0n;
    return {
        energyBurn,
        bandwidthBurn,
        activationFee: estimate.activationFee,
        total: energyBurn + bandwidthBurn + estimate.activationFee,
    };
}

/** 默认的 fee_limit：全部能量都靠燃烧时的花费，再留 20% 余量。 */
export function tronFeeLimit(energy: bigint, energyPrice: bigint): bigint {
    return (energy * energyPrice * 12n) / 10n;
}

function createTron(draft: TransferDraft): Promise<TxRecord> {
    return withAccountLock(draft.account.id, () => buildTron(draft));
}

async function buildTron(draft: TransferDraft): Promise<TxRecord> {
    const { account, network, from, to, amount, asset } = draft;
    await rejectTwin(draft);
    const fees = await fetchFees(network);
    if (fees.kind !== "tron") throw new TransferError("invalid", t("手续费数据异常，请稍后重试。"));
    const token = asset !== "native";
    const call = token ? { from, to, contract: asset, data: tronCallData(draft) } : { from, to };
    const [estimate, resources, block] = await Promise.all([
        estimateTron(network, call),
        fetchResources(network, from),
        fetchRefBlock(network),
    ]);
    if (Math.abs(block.timestamp - Date.now()) > REF_BLOCK_DRIFT) {
        throw new TransferError("invalid", t("引用区块的时间异常，已停止构建。"));
    }
    const feeLimit = token ? (draft.feeLimit ?? tronFeeLimit(estimate.energy, fees.energyPrice)) : 0n;
    // 过期时间从区块时间算，不依赖本机时钟。
    const expirationMs = block.timestamp + TRON_LIFETIME;
    const cost = tronCost(estimate, resources, fees.energyPrice, fees.bandwidthPrice);
    const head = header(draft.wallet, network);
    const request = buildTronTransfer({
        ...head,
        path: account.path,
        from,
        to,
        amount: amount.toString(),
        ...(token ? { contract: asset } : {}),
        refBlockBytes: block.refBlockBytes,
        refBlockHash: block.refBlockHash,
        expirationMs,
        timestampMs: block.timestamp,
        feeLimit: Number(feeLimit),
    });
    reviewRequest(request, draft.accounts);
    const plan: TxPlan = {
        chain: "tron",
        expirationMs,
        feeLimit: Number(feeLimit),
        energy: estimate.energy.toString(),
        bandwidth: estimate.bandwidth.toString(),
        activationFee: estimate.activationFee.toString(),
        burn: cost.total.toString(),
    };
    // TRON 没有序号可以占用，防重放靠过期时间和交易哈希。
    return insertTx(
        requestFields(draft, head.id, request, plan, amount.toString(), "transfer", expirationMs, true),
        [],
    );
}

const SOLANA_NONCE_CLAIM = "account";

/** nonce 值是一次性的，上一笔没有结束时再构建，两笔只有一笔能上链。 */
async function ensureNonceFree(account: Account, network: string): Promise<void> {
    const taken = (await reservationsOf(account.id, network)).some((row) => row.type === "solana-nonce");
    if (taken) throw new TransferError("nonce-busy", t("上一笔 Solana 交易还没有结束，完成后才能转账。"));
}

async function createSolana(draft: TransferDraft): Promise<TxRecord> {
    const { account, network, from, to, amount, asset, entry } = draft;
    return withAccountLock(account.id, async () => {
        await rejectTwin(draft);
        await ensureNonceFree(account, network);
        const nonceAccount = solanaNonceAddress(from);
        const token = asset !== "native";
        const [fees, info, estimate] = await Promise.all([
            fetchFees(network),
            fetchNonceAccount(network, nonceAccount),
            token ? estimateSolana(network, to, asset) : Promise.resolve({ recipientTokenAccount: true, rent: 0n }),
        ]);
        if (!info.exists) throw new TransferError("no-nonce-account", t("首次 Solana 转账前要先创建 nonce 账户。"));
        if (fees.kind !== "solana") throw new TransferError("invalid", t("手续费数据异常，请稍后重试。"));
        const createAta = token && !estimate.recipientTokenAccount;
        const computeUnits = solanaComputeUnits(token, createAta);
        const head = header(draft.wallet, network);
        const priority = Number(fees[draft.tier]);
        const request = buildSolanaTransfer({
            ...head,
            path: account.path,
            from,
            to,
            amount,
            ...(token ? { mint: asset } : {}),
            decimals: entry.decimals,
            nonceValue: info.nonce,
            priorityMicroLamports: priority,
            computeUnits,
            createRecipientAta: createAta,
        });
        reviewRequest(request, draft.accounts);
        const plan: TxPlan = {
            chain: "solana",
            nonceValue: info.nonce,
            nonceAccount,
            priority,
            computeUnits,
            createAta,
            rent: estimate.rent.toString(),
        };
        // 不过期，直到 nonce 推进。
        const fields = requestFields(draft, head.id, request, plan, amount.toString(), "transfer", null, false);
        return insertTx(fields, [{ type: "solana-nonce", value: SOLANA_NONCE_CLAIM }]);
    });
}

export type NonceDraft = Pick<TransferDraft, "wallet" | "accounts" | "account" | "from" | "network">;

/** 创建 nonce 账户。这一笔只能用 blockhash，超时要重新构建。 */
export function createSolanaNonce(draft: NonceDraft): Promise<TxRecord> {
    const { account, network, from } = draft;
    return withAccountLock(account.id, async () => {
        await ensureNonceFree(account, network);
        const nonceAccount = solanaNonceAddress(from);
        const info = await fetchNonceAccount(network, nonceAccount);
        if (info.exists) throw new TransferError("invalid", t("nonce 账户已存在。"));
        const blockhash = await fetchBlockhash(network);
        const head = header(draft.wallet, network);
        const request = buildSolanaCreateNonce({ ...head, path: account.path, from, blockhash, lamports: NONCE_RENT });
        reviewRequest(request, draft.accounts);
        const plan: TxPlan = {
            chain: "solana",
            nonceValue: null,
            nonceAccount,
            priority: 0,
            computeUnits: 0,
            createAta: false,
            rent: NONCE_RENT.toString(),
        };
        const fields = requestFields(
            { ...draft, asset: "native", to: nonceAccount },
            head.id,
            request,
            plan,
            NONCE_RENT.toString(),
            "create-nonce",
            Date.now() + SOLANA_BLOCKHASH_WINDOW,
            true,
        );
        return insertTx(fields, [{ type: "solana-nonce", value: SOLANA_NONCE_CLAIM }]);
    });
}

export function createTransfer(draft: TransferDraft): Promise<TxRecord> {
    switch (draft.account.chain) {
        case "evm":
            return createEvm(draft);
        case "bitcoin":
            return createBitcoin(draft);
        case "tron":
            return createTron(draft);
        case "solana":
            return createSolana(draft);
    }
}

export type ReplaceMode = "speedup" | "cancel" | "rebuild";

/** 替换交易的单价至少比原来高 12.5%，节点要求高 10% 才接受。 */
function bump(value: bigint): bigint {
    return (value * 9n) / 8n + 1n;
}

export function bumpedEvmFees(plan: { maxFee: string; maxPriority: string }, fresh: EvmTierFee): EvmTierFee {
    const raise = (old: bigint, next: bigint) => (next > bump(old) ? next : bump(old));
    return {
        maxFee: raise(BigInt(plan.maxFee), fresh.maxFee),
        priority: raise(BigInt(plan.maxPriority), fresh.priority),
    };
}

export type ReplaceDraft = Pick<TransferDraft, "wallet" | "accounts" | "account">;

/** 已上链的交易不能再替换。已放弃但已签名的交易仍然有效，可以发替换交易作废。 */
const REPLACEABLE: ReadonlySet<TxState> = new Set<TxState>([
    "signing",
    "signed",
    "broadcast",
    "uncertain",
    "abandoned",
]);

async function replaceEvm(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft, claims: Claim[]): Promise<TxRecord> {
    const plan = planOf(tx, "evm");
    const fees = await fetchFees(tx.network);
    if (fees.kind !== "evm") throw new TransferError("invalid", t("手续费数据异常，请稍后重试。"));
    const fee = bumpedEvmFees(plan, fees.fast);
    const cancel = mode === "cancel";
    const to = cancel ? tx.from : tx.to;
    const amount = cancel ? "0" : tx.amount;
    const contract = cancel || tx.asset === "native" ? undefined : tx.asset;
    const gasLimit = cancel ? Number(EVM_TRANSFER_GAS) : plan.gasLimit;
    const head = header(draft.wallet, tx.network);
    const next: TxPlan = {
        chain: "evm",
        nonce: plan.nonce,
        gasLimit,
        maxFee: fee.maxFee.toString(),
        maxPriority: fee.priority.toString(),
    };
    const request = buildEvmTransfer({
        ...head,
        path: draft.account.path,
        to,
        amount,
        ...(contract ? { contract } : {}),
        nonce: plan.nonce,
        gasLimit,
        maxFee: next.maxFee,
        maxPriority: next.maxPriority,
    });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        { ...draft, from: tx.from, network: tx.network, asset: cancel ? "native" : tx.asset, to },
        head.id,
        request,
        next,
        amount,
        mode,
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return insertTx({ ...fields, replaces: tx.id }, [], claims);
}

function bitcoinFees(plan: BitcoinPlan, amount: bigint, fast: bigint, account: Account): BitcoinReplacement {
    const total = plan.inputs.reduce((sum, input) => sum + BigInt(input.amount), 0n);
    try {
        return bitcoinReplacement(total, amount, BigInt(plan.fee), BigInt(plan.vsize), fast, account.kind);
    } catch {
        throw new TransferError("insufficient", t("输入金额不够支付更高的手续费。"));
    }
}

async function replaceBitcoin(
    tx: TxRecord,
    mode: ReplaceMode,
    draft: ReplaceDraft,
    claims: Claim[],
): Promise<TxRecord> {
    const plan = planOf(tx, "bitcoin");
    const { account, wallet } = draft;
    const fast = tierRate(await fetchFees(tx.network), "fast");
    // 取消交易再加速还是取消。
    const cancel = mode === "cancel" || tx.kind === "cancel";
    const amount = cancel ? 0n : BigInt(tx.amount);
    const { change } = bitcoinFees(plan, amount, fast, account);

    let changePath = change > 0 ? plan.changePath : null;
    let changeIndex = changePath ? plan.changeIndex : null;
    const fresh: Claim[] = [];
    if (change > 0 && changePath === null) {
        // 原交易没有找零，取消时要给资金分配一个回来的地址。
        changeIndex = await nextChangeIndex(account, tx.network, wallet.mode);
        changePath = addressPath(account, { change: 1, index: changeIndex });
        fresh.push({ type: "change", value: String(changeIndex) });
    }
    const home = changeIndex === null ? null : await ensureAddress(account, wallet.mode, changeIndex, 1);
    const to = cancel && home ? home.address : tx.to;

    const inputs = plan.inputs.map(({ txid, vout, amount: value, path }) => ({ txid, vout, amount: value, path }));
    const head = header(wallet, tx.network);
    const request = buildPsbt({
        ...head,
        keys: accountKeyList(draft.accounts),
        inputs: await withPrevTx(tx.network, inputs),
        ...(cancel ? {} : { to, amount: Number(amount) }),
        ...(changePath ? { changePath } : {}),
        change,
    });
    reviewRequest(request, draft.accounts);
    const { fee, vsize } = bitcoinFeeInfo(request);
    const next: BitcoinPlan = { ...plan, changePath, changeIndex, change, fee, feeRate: Math.ceil(fee / vsize), vsize };
    const fields = requestFields(
        { ...draft, from: tx.from, network: tx.network, asset: tx.asset, to },
        head.id,
        request,
        next,
        amount.toString(),
        mode,
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return insertTx({ ...fields, replaces: tx.id }, fresh, claims);
}

/** Solana 没有手续费替换。取消只推进 nonce，用同一个 nonce 值签过名的交易随之作废。 */
async function replaceSolana(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft, claims: Claim[]): Promise<TxRecord> {
    const plan = planOf(tx, "solana");
    const { nonceValue } = plan;
    if (nonceValue === null || mode === "speedup") {
        throw new TransferError("unsupported", t("Solana 不提供加速。"));
    }
    const place = { ...draft, from: tx.from, network: tx.network };
    const head = header(draft.wallet, tx.network);
    const path = draft.account.path;
    if (mode === "cancel" || tx.kind === "cancel") {
        const request = buildSolanaAdvanceNonce({ ...head, path, from: tx.from, nonceValue });
        reviewRequest(request, draft.accounts);
        const next: TxPlan = { ...plan, priority: 0, computeUnits: 0, createAta: false, rent: "0" };
        const target = { ...place, asset: "native", to: tx.from };
        const fields = requestFields(target, head.id, request, next, "0", "cancel", null, false);
        return insertTx({ ...fields, replaces: tx.id }, [], claims);
    }
    const token = tx.asset !== "native";
    const decimals = lookupAsset(tx.network, tx.asset)?.decimals;
    if (decimals === undefined) throw new TransferError("invalid", t("代币不在内置清单。"));
    const request = buildSolanaTransfer({
        ...head,
        path,
        from: tx.from,
        to: tx.to,
        amount: BigInt(tx.amount),
        ...(token ? { mint: tx.asset } : {}),
        decimals,
        nonceValue,
        priorityMicroLamports: plan.priority,
        computeUnits: plan.computeUnits,
        createRecipientAta: plan.createAta,
    });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        { ...place, asset: tx.asset, to: tx.to },
        head.id,
        request,
        plan,
        tx.amount,
        mode,
        null,
        false,
    );
    return insertTx({ ...fields, replaces: tx.id }, [], claims);
}

/**
 * 生成替换交易或重建交易。复用原交易的 nonce 或同一组 UTXO，保证最多一笔上链。
 * TRON 没有序号，重建就是另一笔独立的交易，可能重复付款，所以不提供。
 */
export function createReplacement(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft): Promise<TxRecord> {
    if (tx.chain === "tron")
        return Promise.reject(new TransferError("unsupported", t("TRON 不提供加速和重建，超过过期时间后自动失效。")));
    return withAccountLock(tx.accountId, async () => {
        const current = await db.txs.get(tx.id);
        if (!current || !REPLACEABLE.has(current.state))
            throw new TransferError("invalid", t("这笔交易已经上链或结束，不能替换。"));
        const claims = await sharedClaims(current);
        if (claims.length === 0) throw new TransferError("invalid", t("这笔交易没有可复用的序号。"));
        switch (current.chain) {
            case "evm":
                return replaceEvm(current, mode, draft, claims);
            case "bitcoin":
                return replaceBitcoin(current, mode, draft, claims);
            default:
                return replaceSolana(current, mode, draft, claims);
        }
    });
}

/** W4 取消后发替换交易：先写入新请求占住序号，再取消原交易，中间不会释放。 */
export async function replaceAndCancel(tx: TxRecord, draft: ReplaceDraft): Promise<TxRecord> {
    const next = await createReplacement(tx, "cancel", draft);
    await cancelTx(tx.id, t("用户取消，已生成替换交易"));
    return next;
}

/**
 * 再发一次同一笔，不改变状态。Solana 的节点经常丢弃交易，durable nonce 没有推进时原交易一直有效，
 * 重发是幂等的。结果由跟踪逻辑从链上判断。
 */
export async function resendTx(tx: TxRecord): Promise<void> {
    if (tx.raw !== undefined) await broadcastRaw(tx.network, tx.raw);
}

const BROADCAST_FAILURES: Record<string, string> = {
    unreachable: t("连接失败"),
    upstream_unavailable: t("网络超时"),
    rate_limited: t("请求过于频繁"),
};

/** 广播同一笔。被拒绝和超时都不是定论，只有节点接收才算已广播。 */
export async function broadcastTx(id: string): Promise<TxRecord> {
    const tx = await beginBroadcast(id);
    if (tx.raw === undefined) throw new Error(t("交易还没有签名"));
    try {
        const hash = await broadcastRaw(tx.network, tx.raw);
        const same = tx.chain === "solana" ? hash === tx.hash : hash.toLowerCase() === tx.hash?.toLowerCase();
        return await finishBroadcast(id, same ? null : t("返回的交易哈希与本地不一致"));
    } catch (error) {
        const detail =
            error instanceof GatewayError
                ? error.detail || (BROADCAST_FAILURES[error.code] ?? error.code)
                : t("连接失败");
        return finishBroadcast(id, detail);
    }
}

// 核心库的类型和初始化。网络表只从这里来，不从网关取。
import init, {
    FrameReceiver,
    FrameSender,
    accountAddress,
    accountSpecs,
    addressFrame,
    bitcoinAddress,
    bitcoinFeeInfo as wasmBitcoinFeeInfo,
    bitcoinReplacement as wasmBitcoinReplacement,
    buildBitcoinTransfer as wasmBuildBitcoin,
    buildEvmTransfer as wasmBuildEvm,
    buildSolanaAdvanceNonce as wasmBuildSolanaAdvance,
    buildSolanaCreateNonce as wasmBuildSolanaNonce,
    buildSolanaTransfer as wasmBuildSolana,
    buildTronTransfer as wasmBuildTron,
    evmTransferData,
    initSync,
    networkLabel,
    networks,
    previewRequest as wasmPreview,
    qrMatrix as wasmQrMatrix,
    qrText as wasmQrText,
    selectBitcoinCoins as wasmSelectCoins,
    solanaNonceAddress,
    validateAddress as wasmValidateAddress,
    verifyManifestPackage,
    verifyResponse as wasmVerifyResponse,
} from "./pkg/bitone_wasm";
import wasmUrl from "./pkg/bitone_wasm_bg.wasm?url";
import { t, translate } from "./i18n";

export type Chain = "bitcoin" | "evm" | "tron" | "solana";
export type AccountKind = "segwit" | "taproot" | "nested-segwit" | "legacy" | "single";
export type Mode = "mainnet" | "testnet";

export interface NetworkInfo {
    id: string;
    chain: Chain;
    testnet: boolean;
    chainId?: number;
}

/** 账户属于哪个网络模式。EVM 主网和测试网共用一个账户，是 both。 */
export type AccountScope = Mode | "both";

export interface PairingAccount {
    chain: Chain;
    kind: AccountKind;
    path: string;
    key: Uint8Array;
    scope: AccountScope;
}

export interface Pairing {
    fingerprint: string;
    accounts: PairingAccount[];
}

export interface QrMatrix {
    size: number;
    modules: boolean[];
}

let ready: Promise<unknown> | undefined;

/** 页面启动时调用一次。 */
export function initCore(): Promise<unknown> {
    ready ??= init({ module_or_path: wasmUrl });
    return ready;
}

/** 测试环境直接传入 wasm 字节。 */
export function initCoreSync(bytes: BufferSource): void {
    initSync({ module: bytes });
}

let cachedNetworks: NetworkInfo[] | undefined;

export function networkList(): NetworkInfo[] {
    cachedNetworks ??= networks() as NetworkInfo[];
    return cachedNetworks;
}

export function networksFor(chain: Chain, mode: Mode): NetworkInfo[] {
    return networkList().filter((n) => n.chain === chain && n.testnet === (mode === "testnet"));
}

/** 地址和二维码都用每条链在该模式下的第一个网络。 */
export function primaryNetwork(chain: Chain, mode: Mode): string {
    const first = networksFor(chain, mode)[0];
    if (!first) throw new Error(t("未知网络"));
    return first.id;
}

/** 界面上显示的网络名，名称表在核心库里。 */
export function networkName(id: string): string {
    try {
        return networkLabel(id);
    } catch {
        return id;
    }
}

export type AccountSpec = Pick<PairingAccount, "chain" | "kind" | "path">;

/** 这个模式下白名单里的账户，含旧路径账户。 */
export function accountSpecList(mode: Mode): AccountSpec[] {
    return accountSpecs(mode === "testnet") as AccountSpec[];
}

const cachedRanks: Partial<Record<Mode, string[]>> = {};

/** 账户的显示顺序，与核心库导出的顺序一致。 */
export function accountRank(path: string, mode: Mode): number {
    cachedRanks[mode] ??= accountSpecList(mode).map((s) => s.path);
    return cachedRanks[mode].indexOf(path);
}

/** 这两类账户只用于转出旧资产，有余额时才显示。 */
export function isLegacyKind(kind: AccountKind): boolean {
    return kind === "nested-segwit" || kind === "legacy";
}

export function deriveAddress(
    account: Pick<PairingAccount, "chain" | "kind" | "key">,
    mode: Mode,
    change: 0 | 1,
    index: number,
): string {
    if (account.chain === "bitcoin") {
        const network = primaryNetwork("bitcoin", mode);
        return bitcoinAddress(account.key, account.kind, change === 1, index, network);
    }
    return accountAddress(account.chain, account.key);
}

export function qrMatrix(part: string): QrMatrix {
    return wasmQrMatrix(part) as QrMatrix;
}

/** 普通文本的二维码，内容不会被转成大写。 */
export function qrText(text: string): QrMatrix {
    return wasmQrText(text) as QrMatrix;
}

export type FeedResult = "accepted" | "repeated" | "foreign" | "invalid";

/**
 * 包一层 FrameReceiver。它记住收到的第一种 UR 类型，之后拒收其他类型，
 * 所以别的二维码要在这里挡掉，否则误扫一次就再也收不到后面的帧。
 */
export class UrReceiver {
    protected readonly receiver = new FrameReceiver();
    readonly #prefix: string;
    #last = "";
    progress = 0;
    frames = 0;

    constructor(type: string) {
        this.#prefix = `ur:${type}/`;
    }

    feed(text: string): FeedResult {
        const part = text.trim().toLowerCase();
        if (part === this.#last) return "repeated";
        if (!part.startsWith(this.#prefix)) return "foreign";
        this.#last = part;
        try {
            this.progress = this.receiver.receive(part);
        } catch {
            return "invalid";
        }
        const total = /^[^/]+\/\d+-(\d+)\//.exec(part)?.[1];
        this.frames = Number(total ?? 1);
        return "accepted";
    }

    get complete(): boolean {
        return this.receiver.complete();
    }

    /** 收齐后的原始消息。 */
    message(): Uint8Array {
        return this.receiver.message();
    }

    free(): void {
        this.receiver.free();
    }
}

export class PairingReceiver extends UrReceiver {
    constructor() {
        super("bitone-accounts");
    }

    /** 收齐后调用。核心库解码时校验路径，不在白名单时抛错。 */
    pairing(): Pairing {
        return this.receiver.accounts() as Pairing;
    }
}

export { FrameSender, addressFrame, evmTransferData, solanaNonceAddress };

/** 校验收款地址属于这个网络，返回规范写法。格式错误时抛错。 */
export function validateAddress(network: string, text: string): string {
    return wasmValidateAddress(network, text.trim());
}

export type KeyEntry = Pick<PairingAccount, "chain" | "kind" | "path" | "key">;

/** 核心库需要的账户公钥列表。 */
export function accountKeyList(accounts: readonly KeyEntry[]): KeyEntry[] {
    return accounts.map(({ chain, kind, path, key }) => ({ chain, kind, path, key }));
}

/** 请求头。`id` 是 16 字节随机数，WASM 里不放随机数。 */
export interface RequestHeader {
    id: Uint8Array;
    fingerprint: string;
    network: string;
}

export interface EvmTransfer extends RequestHeader {
    path: string;
    to: string;
    amount: string;
    contract?: string;
    nonce: number;
    gasLimit: number;
    maxFee: string;
    maxPriority: string;
}

export interface TronTransfer extends RequestHeader {
    path: string;
    from: string;
    to: string;
    amount: string;
    contract?: string;
    refBlockBytes: string;
    refBlockHash: string;
    expirationMs: number;
    timestampMs: number;
    feeLimit: number;
}

export interface SolanaTransfer extends RequestHeader {
    path: string;
    from: string;
    to: string;
    amount: bigint;
    mint?: string;
    decimals: number;
    nonceValue: string;
    priorityMicroLamports: number;
    computeUnits: number;
    createRecipientAta: boolean;
}

export interface SolanaCreateNonce extends RequestHeader {
    path: string;
    from: string;
    blockhash: string;
    lamports: bigint;
}

export interface CoinInput {
    txid: string;
    vout: number;
    amount: number;
    path: string;
    prevTx?: Uint8Array;
}

export interface BitcoinTransfer extends RequestHeader {
    keys: KeyEntry[];
    inputs: CoinInput[];
    /** 留空表示取消，全部回到 `changePath`，冷钱包显示转给自己。 */
    to?: string;
    amount?: number;
    changePath?: string;
    change: number;
}

export interface SolanaAdvanceNonce extends RequestHeader {
    path: string;
    from: string;
    nonceValue: string;
}

export const buildEvmTransfer = (args: EvmTransfer): Uint8Array => wasmBuildEvm(args);
export const buildTronTransfer = (args: TronTransfer): Uint8Array => wasmBuildTron(args);
export const buildSolanaTransfer = (args: SolanaTransfer): Uint8Array => wasmBuildSolana(args);
export const buildSolanaCreateNonce = (args: SolanaCreateNonce): Uint8Array => wasmBuildSolanaNonce(args);
export const buildSolanaAdvanceNonce = (args: SolanaAdvanceNonce): Uint8Array => wasmBuildSolanaAdvance(args);
export const buildBitcoinTransfer = (args: BitcoinTransfer): Uint8Array => wasmBuildBitcoin(args);

export interface CoinSelection {
    inputs: Omit<CoinInput, "prevTx">[];
    fee: number;
    change: number;
}

/** 选币。金额不够时抛错。选出后要取回前序交易才能构建。 */
export function selectCoins(utxos: CoinInput[], amount: bigint, feeRate: bigint, kind: AccountKind): CoinSelection {
    return wasmSelectCoins(utxos, amount, feeRate, kind) as CoinSelection;
}

export interface BitcoinFeeInfo {
    fee: number;
    vsize: number;
}

/** PSBT 的手续费和估算的虚拟大小。 */
export function bitcoinFeeInfo(request: Uint8Array): BitcoinFeeInfo {
    return wasmBitcoinFeeInfo(request) as BitcoinFeeInfo;
}

export interface BitcoinReplacement {
    fee: number;
    change: number;
}

/** RBF 用同一组输入提高手续费。`amount` 为 0 表示取消。输入不够付时抛错。 */
export function bitcoinReplacement(
    total: bigint,
    amount: bigint,
    oldFee: bigint,
    vsize: bigint,
    feeRate: bigint,
    changeKind: AccountKind,
): BitcoinReplacement {
    return wasmBitcoinReplacement(total, amount, oldFee, vsize, feeRate, changeKind) as BitcoinReplacement;
}

/** 校验清单包，返回版本号。网页端不知道冷钱包现在的版本，版本回退由冷钱包判断。 */
export function manifestPackageVersion(bytes: Uint8Array): number {
    return verifyManifestPackage(bytes, 0);
}

export type Risk = "transfer" | "recognized" | "unknown" | "approval" | "permission-change" | "forbidden";

export interface Review {
    risk: Risk;
    confirm: "tap" | "hold" | "refuse";
    title: string;
    amount?: { text: string; symbol?: string; asset?: string; unlimited: boolean } | null;
    counterparty?: string | null;
    alerts: string[];
    actions: { title: string }[];
    raw: { text: string }[];
}

/** 用和冷钱包相同的代码预览。结果只用于提示，判断以冷钱包屏幕为准。 */
export function previewRequest(request: Uint8Array, keys: KeyEntry[]): Review {
    const review = wasmPreview(request, keys) as Review;
    return { ...review, title: translate(review.title), alerts: review.alerts.map(translate) };
}

export interface Verified {
    raw: string;
    hash: string;
}

/** 广播前核验。`signer` 是付款账户公钥，Bitcoin 的公钥在 PSBT 里，传空数组。 */
export function verifyResponse(request: Uint8Array, response: Uint8Array, signer: Uint8Array): Verified {
    return wasmVerifyResponse(request, response, signer) as Verified;
}

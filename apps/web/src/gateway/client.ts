// 网关 HTTP 接口，格式见 docs/09-gateway-api.md。网关的数据都不可信，进来先校验。
import { cleanText } from "../core/text";
import { parseAmount } from "../core/amount";
import type { ChainObservation, ChainStatus } from "../data/machine";
import { t } from "../core/i18n";

/** 单次请求的地址上限。 */
export const MAX_ADDRESSES = 100;
/** 一次快照的条目上限。 */
export const MAX_SNAPSHOT_ITEMS = 1000;

export interface Balance {
    address: string;
    /** `native` 或合约地址。 */
    asset: string;
    amount: bigint;
}

/** 一次请求的时限。转账构建在账户锁里等待网关，不能一直挂着。 */
const REQUEST_TIMEOUT = 20_000;
/** 网关可能依次尝试几个上游，广播要比普通查询等得久。 */
const BROADCAST_TIMEOUT = 45_000;
/** 快照要等最慢的网络，单个上游超时后还要换下一个。 */
const SNAPSHOT_TIMEOUT = 60_000;

export class GatewayError extends Error {
    readonly code: string;
    /** 网关转述的节点原文，只当纯文本显示。 */
    readonly detail: string;
    constructor(code: string, detail = "") {
        super(t("网关不可用"));
        this.code = code;
        this.detail = detail;
    }
}

async function open(path: string, init?: RequestInit, timeout = REQUEST_TIMEOUT): Promise<Response> {
    let response: Response;
    try {
        response = await fetch(path, { ...init, signal: AbortSignal.timeout(timeout) });
    } catch {
        throw new GatewayError("unreachable");
    }
    if (response.ok) return response;
    const body: unknown = await response.json().catch(() => null);
    const error = (body as { error?: { code?: unknown; message?: unknown } } | null)?.error;
    const detail = typeof error?.message === "string" ? cleanText(error.message, 200) : "";
    throw new GatewayError(typeof error?.code === "string" ? cleanText(error.code) : "unreachable", detail);
}

async function request(path: string, init?: RequestInit, timeout?: number): Promise<unknown> {
    const response = await open(path, init, timeout);
    return response.json().catch(() => null);
}

function post(network: string, path: string, body: unknown, timeout?: number): Promise<unknown> {
    return request(
        `/v1/${encodeURIComponent(network)}/${path}`,
        { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) },
        timeout,
    );
}

function get(network: string, path: string): Promise<unknown> {
    return request(`/v1/${encodeURIComponent(network)}/${path}`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null;
}

/** 响应不完整时算查询失败，不能当成 0 或空。 */
function malformed(): GatewayError {
    return new GatewayError("unreachable");
}

function requireAmount(value: unknown): bigint {
    const amount = parseAmount(value);
    if (amount === null) throw malformed();
    return amount;
}

function requireCount(value: unknown): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw malformed();
    return value;
}

function requireHex(value: unknown, bytes: number): string {
    if (typeof value !== "string" || value.length !== bytes * 2 || !/^[0-9a-f]+$/i.test(value)) throw malformed();
    return value;
}

function requireText(value: unknown, max: number): string {
    if (typeof value !== "string") throw malformed();
    const text = cleanText(value, max);
    if (!text) throw malformed();
    return text;
}

function chunks<T>(list: readonly T[], size: number): T[][] {
    const result: T[][] = [];
    for (let i = 0; i < list.length; i += size) result.push(list.slice(i, i + size));
    return result;
}

export interface GatewayVersion {
    version: string;
    networks: string[];
    /** 没有配置索引服务的网络，查不到链上历史。 */
    noHistory: string[];
}

export async function fetchVersion(): Promise<GatewayVersion> {
    const body = (await request("/v1/version")) as Partial<GatewayVersion> | null;
    if (!body || !Array.isArray(body.networks)) throw new GatewayError("unreachable");
    return {
        version: typeof body.version === "string" ? cleanText(body.version, 32) : "",
        networks: body.networks.filter((n): n is string => typeof n === "string"),
        noHistory: (body.noHistory ?? []).filter((n): n is string => typeof n === "string"),
    };
}

/** EVM 地址可能以不同大小写返回，其余地址区分大小写。 */
export function addressKey(address: string): string {
    return address.startsWith("0x") ? address.toLowerCase() : address;
}

/**
 * 只留下请求过的地址和合法金额，同一地址同一资产以最后一条为准。
 * 地址统一换回本地的写法，后面按本地地址聚合。
 */
export function parseBalances(list: unknown, addresses: readonly string[]): Balance[] {
    if (!Array.isArray(list)) return [];
    const known = new Map(addresses.map((address) => [addressKey(address), address]));
    const merged = new Map<string, Balance>();
    for (const item of list as Record<string, unknown>[]) {
        const address = typeof item?.address === "string" ? known.get(addressKey(item.address)) : undefined;
        const amount = parseAmount(item?.amount);
        if (!address || amount === null || typeof item.asset !== "string") continue;
        const asset = cleanText(item.asset, 128);
        if (!asset) continue;
        merged.set(`${address}\n${addressKey(asset)}`, { address, asset, amount });
    }
    return [...merged.values()];
}

export interface Utxo {
    address: string;
    txid: string;
    vout: number;
    amount: bigint;
    /** 0 表示还在内存池。 */
    height: number;
}

/** 只留下请求过的地址。网关给的 UTXO 不可信，金额由前序交易的 txid 校验。 */
export async function fetchUtxos(network: string, addresses: readonly string[]): Promise<Utxo[]> {
    const pages = await Promise.all(
        chunks(addresses, MAX_ADDRESSES).map(async (chunk) => {
            const body = await post(network, "utxos", { addresses: chunk });
            const list = isRecord(body) ? body.utxos : null;
            if (!Array.isArray(list)) throw malformed();
            const known = new Set(chunk);
            return list.flatMap((item: unknown): Utxo[] => {
                if (!isRecord(item) || typeof item.address !== "string" || !known.has(item.address)) return [];
                return [
                    {
                        address: item.address,
                        // 占用按字符串比较，大小写不同不能算成另一个 UTXO。
                        txid: requireHex(item.txid, 32).toLowerCase(),
                        vout: requireCount(item.vout),
                        amount: requireAmount(item.amount),
                        height: requireCount(item.height),
                    },
                ];
            });
        }),
    );
    return pages.flat();
}

/** 只返回请求过的 txid。内容对不对由核心库重算 txid 判断。 */
export async function fetchRawTxs(network: string, txids: readonly string[]): Promise<Map<string, string>> {
    const result = new Map<string, string>();
    for (const chunk of chunks(txids, MAX_ADDRESSES)) {
        const body = await post(network, "raw-txs", { txids: chunk });
        const map = isRecord(body) ? body.txs : null;
        if (!isRecord(map)) throw malformed();
        for (const txid of chunk) {
            const hex = map[txid];
            if (typeof hex === "string" && hex.length % 2 === 0 && /^[0-9a-f]*$/i.test(hex)) result.set(txid, hex);
        }
    }
    return result;
}

export interface NonceInfo {
    latest: number;
    pending: number;
}

export async function fetchNonces(network: string, addresses: readonly string[]): Promise<Map<string, NonceInfo>> {
    const body = await post(network, "nonce", { addresses });
    const map = isRecord(body) ? body.nonces : null;
    if (!isRecord(map)) throw malformed();
    const lower = new Map(Object.entries(map).map(([address, value]) => [addressKey(address), value]));
    const result = new Map<string, NonceInfo>();
    for (const address of addresses) {
        const row = lower.get(addressKey(address));
        if (!isRecord(row)) throw malformed();
        result.set(address, { latest: requireCount(row.latest), pending: requireCount(row.pending) });
    }
    return result;
}

export interface TronResources {
    bandwidth: bigint;
    energy: bigint;
    activated: boolean;
}

export async function fetchResources(network: string, address: string): Promise<TronResources> {
    const body = await post(network, "resources", { addresses: [address] });
    const map = isRecord(body) ? body.resources : null;
    const row = isRecord(map) ? map[address] : null;
    if (!isRecord(row)) throw malformed();
    return {
        bandwidth: BigInt(requireCount(row.bandwidth)),
        energy: BigInt(requireCount(row.energy)),
        activated: row.activated === true,
    };
}

export interface RefBlock {
    refBlockBytes: string;
    refBlockHash: string;
    timestamp: number;
}

export async function fetchRefBlock(network: string): Promise<RefBlock> {
    const body = await get(network, "ref-block");
    if (!isRecord(body)) throw malformed();
    return {
        refBlockBytes: requireHex(body.refBlockBytes, 2),
        refBlockHash: requireHex(body.refBlockHash, 8),
        timestamp: requireCount(body.timestamp),
    };
}

export interface NonceAccount {
    exists: boolean;
    nonce: string;
}

export async function fetchNonceAccount(network: string, address: string): Promise<NonceAccount> {
    const body = await post(network, "nonce-account", { address });
    if (!isRecord(body) || typeof body.exists !== "boolean") throw malformed();
    return body.exists ? { exists: true, nonce: requireText(body.nonce, 64) } : { exists: false, nonce: "" };
}

export async function fetchBlockhash(network: string): Promise<string> {
    const body = await get(network, "blockhash");
    return requireText(isRecord(body) ? body.blockhash : null, 64);
}

export type FeeTier = "slow" | "medium" | "fast";

export interface EvmTierFee {
    maxFee: bigint;
    priority: bigint;
}

export type Fees =
    | { kind: "bitcoin"; slow: bigint; medium: bigint; fast: bigint }
    | { kind: "evm"; baseFee: bigint; slow: EvmTierFee; medium: EvmTierFee; fast: EvmTierFee }
    | { kind: "tron"; energyPrice: bigint; bandwidthPrice: bigint }
    | { kind: "solana"; slow: bigint; medium: bigint; fast: bigint };

function evmTier(value: unknown): EvmTierFee {
    if (!isRecord(value)) throw malformed();
    return { maxFee: requireAmount(value.maxFee), priority: requireAmount(value.priority) };
}

export async function fetchFees(network: string): Promise<Fees> {
    const body = await get(network, "fees");
    if (!isRecord(body)) throw malformed();
    switch (body.kind) {
        case "bitcoin":
        case "solana":
            return {
                kind: body.kind,
                slow: BigInt(requireCount(body.slow)),
                medium: BigInt(requireCount(body.medium)),
                fast: BigInt(requireCount(body.fast)),
            };
        case "evm":
            return {
                kind: "evm",
                baseFee: requireAmount(body.baseFee),
                slow: evmTier(body.slow),
                medium: evmTier(body.medium),
                fast: evmTier(body.fast),
            };
        case "tron":
            return {
                kind: "tron",
                energyPrice: BigInt(requireCount(body.energyPrice)),
                bandwidthPrice: BigInt(requireCount(body.bandwidthPrice)),
            };
        default:
            throw malformed();
    }
}

/** 合约回滚时网关返回 400 `invalid_request` 加节点原文。 */
export async function estimateEvmGas(
    network: string,
    call: { from: string; to: string; value: string; data?: string },
): Promise<bigint> {
    const body = await post(network, "estimate", call);
    return requireAmount(isRecord(body) ? body.gas : null);
}

export interface TronEstimate {
    energy: bigint;
    bandwidth: bigint;
    activationFee: bigint;
}

export async function estimateTron(
    network: string,
    call: { from: string; to: string; contract?: string; data?: string },
): Promise<TronEstimate> {
    const body = await post(network, "estimate", call);
    if (!isRecord(body)) throw malformed();
    return {
        energy: requireAmount(body.energy),
        bandwidth: requireAmount(body.bandwidth),
        activationFee: requireAmount(body.activationFee),
    };
}

export interface SolanaEstimate {
    recipientTokenAccount: boolean;
    rent: bigint;
}

export async function estimateSolana(network: string, recipient: string, mint?: string): Promise<SolanaEstimate> {
    const body = await post(network, "estimate", { recipient, mint });
    if (!isRecord(body) || typeof body.recipientTokenAccount !== "boolean") throw malformed();
    return { recipientTokenAccount: body.recipientTokenAccount, rent: requireAmount(body.rent) };
}

/** 节点接收时返回哈希。其余情况都抛错，由调用方按结果不明处理。 */
export async function broadcastRaw(network: string, raw: string): Promise<string> {
    const body = await post(network, "broadcast", { raw }, BROADCAST_TIMEOUT);
    if (!isRecord(body) || body.status !== "accepted") throw malformed();
    return requireText(body.hash, 128);
}

const STATUSES: readonly ChainStatus[] = ["unknown", "pending", "included", "finalized", "failed"];

function isStatus(value: unknown): value is ChainStatus {
    return STATUSES.includes(value as ChainStatus);
}

/** `/tx` 和 `tx` 频道共用的格式。不合法时返回 null。 */
export function parseObservation(value: unknown): ChainObservation | null {
    if (!isRecord(value) || typeof value.hash !== "string" || !isStatus(value.status)) return null;
    const block = typeof value.block === "number" && Number.isSafeInteger(value.block) ? value.block : null;
    const confirmations =
        typeof value.confirmations === "number" && Number.isSafeInteger(value.confirmations) ? value.confirmations : 0;
    const fee = parseAmount(value.fee);
    return {
        hash: value.hash,
        status: value.status,
        stage: typeof value.stage === "string" ? cleanText(value.stage, 16) : null,
        confirmations: Math.max(confirmations, 0),
        block,
        fee: fee === null ? null : fee.toString(),
    };
}

export async function fetchTxs(network: string, hashes: readonly string[]): Promise<ChainObservation[]> {
    const pages = await Promise.all(
        chunks(hashes, MAX_ADDRESSES).map(async (chunk) => {
            const body = await post(network, "tx", { hashes: chunk });
            const list = isRecord(body) ? body.txs : null;
            if (!Array.isArray(list)) throw malformed();
            const known = new Set(chunk.map(addressKey));
            return list.flatMap((item: unknown) => {
                const seen = parseObservation(item);
                return seen && known.has(addressKey(seen.hash)) ? [seen] : [];
            });
        }),
    );
    return pages.flat();
}

export interface HistoryItem {
    hash: string;
    time: number | null;
    from: string | null;
    to: string | null;
    asset: string;
    amount: bigint;
    status: ChainStatus;
}

export interface HistoryPage {
    items: HistoryItem[];
    /** 空表示没有更多。空页也可能带游标，不能遇到空页就停。 */
    cursor: string | null;
    /** 下次增量查询的水位，原样传回，不解析。 */
    checkpoint: string;
    /** 上次之后没有变化，这一页是空的。 */
    unchanged: boolean;
}

function parseItems(list: readonly unknown[]): HistoryItem[] {
    return list.flatMap((item: unknown): HistoryItem[] => {
        if (!isRecord(item) || typeof item.hash !== "string" || !isStatus(item.status)) return [];
        const amount = parseAmount(item.amount);
        const asset = typeof item.asset === "string" ? cleanText(item.asset, 128) : "";
        const hash = cleanText(item.hash, 128);
        if (amount === null || !asset || !hash) return [];
        const text = (value: unknown) => (typeof value === "string" && cleanText(value, 128)) || null;
        return [
            {
                hash,
                time: typeof item.time === "number" ? item.time : null,
                from: text(item.from),
                to: text(item.to),
                asset,
                amount,
                status: item.status,
            },
        ];
    });
}

function parsePage(body: unknown): HistoryPage {
    const list = isRecord(body) ? body.items : null;
    if (!isRecord(body) || !Array.isArray(list)) throw malformed();
    return {
        items: parseItems(list),
        cursor: typeof body.cursor === "string" && body.cursor ? body.cursor : null,
        checkpoint: typeof body.checkpoint === "string" ? cleanText(body.checkpoint, 32) : "",
        unchanged: body.unchanged === true,
    };
}

export async function fetchHistory(
    network: string,
    addresses: readonly string[],
    cursor: string | null,
    since: string | null = null,
): Promise<HistoryPage> {
    return parsePage(await post(network, "history", { addresses, cursor, since }));
}

export interface SnapshotItem {
    network: string;
    address: string;
    /** 没有时是原生币。 */
    contract?: string;
    since?: string;
    /** 分批编号。翻页游标绑定一批地址，水位相同的批次也不能合并。 */
    group: number;
}

/** 一批地址的最新状态。`balances` 和 `history` 分开报告失败，空表示这部分没查到。 */
export interface SnapshotResult {
    network: string;
    addresses: string[];
    height: number | null;
    balances: Balance[] | null;
    history: HistoryPage | null;
    error: string | null;
}

function parseHeight(value: unknown): number | null {
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function parseSnapshot(value: unknown): SnapshotResult | null {
    if (!isRecord(value) || typeof value.network !== "string" || !Array.isArray(value.addresses)) return null;
    const addresses = value.addresses.filter((address): address is string => typeof address === "string");
    return {
        network: value.network,
        addresses,
        height: parseHeight(value.height),
        // 响应不完整时算查询失败，不能当成余额为 0。
        balances: Array.isArray(value.balances) ? parseBalances(value.balances, addresses) : null,
        history: isRecord(value.history) ? parsePage(value.history) : null,
        error: typeof value.error === "string" ? cleanText(value.error) : null,
    };
}

/** 响应一行一个 JSON，哪批先查完就先到，到一批处理一批。慢的网络不拖累其他网络。 */
export async function fetchSnapshot(
    items: readonly SnapshotItem[],
    onResult: (result: SnapshotResult) => Promise<void>,
): Promise<void> {
    const response = await open(
        "/v1/snapshot",
        { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ items }) },
        SNAPSHOT_TIMEOUT,
    );
    const reader = response.body?.pipeThrough(new TextDecoderStream()).getReader();
    if (!reader) throw malformed();
    let buffer = "";
    for (;;) {
        const { done, value } = await reader.read().catch(() => {
            throw new GatewayError("unreachable");
        });
        buffer += value ?? "";
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
            const result = parseSnapshot(parseLine(line));
            if (result) await onResult(result);
        }
        if (done) return;
    }
}

function parseLine(line: string): unknown {
    try {
        return JSON.parse(line);
    } catch {
        throw malformed();
    }
}

/** 跟块推送。`items` 为空表示网关这次没取到交易，要自己补取。 */
export interface ChainUpdate {
    height: number;
    /** 上一条推送的高度，接不上说明中间漏了。 */
    previous: number;
    balances: Balance[];
    items: HistoryItem[] | null;
}

export function parseUpdate(value: unknown, addresses: readonly string[]): ChainUpdate | null {
    if (!isRecord(value)) return null;
    const height = parseHeight(value.height);
    const previous = parseHeight(value.previous);
    if (height === null || previous === null) return null;
    return {
        height,
        previous,
        balances: parseBalances(value.balances, addresses),
        items: Array.isArray(value.txs) ? parseItems(value.txs) : null,
    };
}

export type PriceMap = Map<string, string>;

/** 键统一成 `网络:合约`，EVM 合约转小写。`/prices` 和 `prices` 频道共用。 */
export function parsePrices(value: unknown): PriceMap {
    const prices: PriceMap = new Map();
    if (!isRecord(value)) return prices;
    for (const [key, price] of Object.entries(value)) {
        if (typeof price !== "string") continue;
        const split = key.indexOf(":");
        if (split <= 0) continue;
        prices.set(`${key.slice(0, split)}:${addressKey(key.slice(split + 1))}`, price);
    }
    return prices;
}

export async function fetchPrices(): Promise<PriceMap> {
    const body = await request("/v1/prices");
    if (!isRecord(body) || !isRecord(body.prices)) throw malformed();
    return parsePrices(body.prices);
}

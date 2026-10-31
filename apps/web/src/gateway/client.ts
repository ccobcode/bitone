// 网关 HTTP 接口.网关的数据都不可信,进来先校验.
import { addressKey, cleanText, isRecord } from "../core/text";
import { parseAmount } from "../core/amount";
import type { ChainObservation, ChainStatus } from "../data/machine";
import { t } from "../core/i18n";

export const MAX_ADDRESSES = 100;
export const MAX_SNAPSHOT_ITEMS = 1000;

export interface Balance {
    address: string;
    /** `native` 或合约地址. */
    asset: string;
    amount: bigint;
}

/** 一次请求的时限.转账构建在账户锁里等待网关,不能一直挂着. */
const REQUEST_TIMEOUT = 20_000;
/** 网关可能依次尝试几个上游,广播要比普通查询等得久. */
const BROADCAST_TIMEOUT = 45_000;
/** 快照要等最慢的网络,单个上游超时后还要换下一个. */
const SNAPSHOT_TIMEOUT = 60_000;

export class GatewayError extends Error {
    readonly code: string;
    /** 网关转述的节点原文,只当纯文本显示. */
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

export function post(network: string, path: string, body: unknown, timeout?: number): Promise<unknown> {
    return request(
        `/v1/${encodeURIComponent(network)}/${path}`,
        { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) },
        timeout,
    );
}

export function get(network: string, path: string): Promise<unknown> {
    return request(`/v1/${encodeURIComponent(network)}/${path}`);
}

/** 响应不完整时算查询失败,不能当成 0 或空. */
export function malformed(): GatewayError {
    return new GatewayError("unreachable");
}

export function requireAmount(value: unknown): bigint {
    const amount = parseAmount(value);
    if (amount === null) throw malformed();
    return amount;
}

export function requireCount(value: unknown): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw malformed();
    return value;
}

export function requireHex(value: unknown, bytes: number): string {
    if (typeof value !== "string" || value.length !== bytes * 2 || !/^[0-9a-f]+$/i.test(value)) throw malformed();
    return value;
}

export function requireText(value: unknown, max: number): string {
    if (typeof value !== "string") throw malformed();
    const text = cleanText(value, max);
    if (!text) throw malformed();
    return text;
}

export function chunks<T>(list: readonly T[], size: number): T[][] {
    const result: T[][] = [];
    for (let i = 0; i < list.length; i += size) result.push(list.slice(i, i + size));
    return result;
}

export interface GatewayVersion {
    networks: string[];
    /** 没有配置索引服务的网络,查不到链上历史. */
    noHistory: string[];
}

export async function fetchVersion(): Promise<GatewayVersion> {
    const body = (await request("/v1/version")) as Partial<GatewayVersion> | null;
    if (!body || !Array.isArray(body.networks)) throw new GatewayError("unreachable");
    return {
        networks: body.networks.filter((n): n is string => typeof n === "string"),
        noHistory: (body.noHistory ?? []).filter((n): n is string => typeof n === "string"),
    };
}

export { addressKey };

/**
 * 只留下请求过的地址和合法金额,同一地址同一资产以最后一条为准.
 * 地址统一换回本地的写法,后面按本地地址聚合.
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

export type FeeTier = "slow" | "medium" | "fast";

/** 手续费的格式按链不同,由注册表按 `kind` 分发给各链解析. */
export async function fetchFeeBody(network: string): Promise<Record<string, unknown>> {
    const body = await get(network, "fees");
    if (!isRecord(body)) throw malformed();
    return body;
}

/** 三档都是整数的手续费. */
export function requireTiers(body: Record<string, unknown>): Record<FeeTier, bigint> {
    return {
        slow: BigInt(requireCount(body.slow)),
        medium: BigInt(requireCount(body.medium)),
        fast: BigInt(requireCount(body.fast)),
    };
}

/** 节点接收时返回哈希.其余情况都抛错,由调用方按结果不明处理. */
export async function broadcastRaw(network: string, raw: string): Promise<string> {
    const body = await post(network, "broadcast", { raw }, BROADCAST_TIMEOUT);
    if (!isRecord(body) || body.status !== "accepted") throw malformed();
    return requireText(body.hash, 128);
}

const STATUSES: readonly ChainStatus[] = ["unknown", "pending", "included", "finalized", "failed"];

function isStatus(value: unknown): value is ChainStatus {
    return STATUSES.includes(value as ChainStatus);
}

/** `/tx` 和 `tx` 频道共用的格式.不合法时返回 null. */
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

/** 一笔交易的输入输出.Bitcoin 才有,见 `walletView`. */
export interface Flows {
    inputs: string[];
    outputs: { address: string | null; amount: string }[];
}

export interface HistoryItem {
    hash: string;
    time: number | null;
    from: string | null;
    to: string | null;
    asset: string;
    amount: bigint;
    status: ChainStatus;
    flows?: Flows;
}

export interface HistoryPage {
    items: HistoryItem[];
    /** 空表示没有更多.空页也可能带游标,不能遇到空页就停. */
    cursor: string | null;
    /** 下次增量查询的水位,原样传回,不解析. */
    checkpoint: string;
    /** 上次之后没有变化,这一页是空的. */
    unchanged: boolean;
}

/** 只收形状完整的.不完整时退回网关按批归纳的结果. */
function parseFlows(value: unknown): Flows | null {
    if (!isRecord(value) || !Array.isArray(value.inputs) || !Array.isArray(value.outputs)) return null;
    const inputs = value.inputs.flatMap((input: unknown) => (typeof input === "string" ? [cleanText(input, 128)] : []));
    const outputs = value.outputs.flatMap((output: unknown) => {
        if (!isRecord(output)) return [];
        const amount = parseAmount(output.amount);
        const address = typeof output.address === "string" ? cleanText(output.address, 128) : null;
        return amount === null ? [] : [{ address, amount: amount.toString() }];
    });
    const complete = inputs.length === value.inputs.length && outputs.length === value.outputs.length;
    return complete ? { inputs, outputs } : null;
}

function parseItems(list: readonly unknown[]): HistoryItem[] {
    return list.flatMap((item: unknown): HistoryItem[] => {
        if (!isRecord(item) || typeof item.hash !== "string" || !isStatus(item.status)) return [];
        const amount = parseAmount(item.amount);
        const asset = typeof item.asset === "string" ? cleanText(item.asset, 128) : "";
        const hash = cleanText(item.hash, 128);
        if (amount === null || !asset || !hash) return [];
        const text = (value: unknown) => (typeof value === "string" && cleanText(value, 128)) || null;
        const flows = parseFlows(item.flows);
        return [
            {
                hash,
                time: typeof item.time === "number" ? item.time : null,
                from: text(item.from),
                to: text(item.to),
                asset,
                amount,
                status: item.status,
                ...(flows ? { flows } : {}),
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

/** 这批地址里有过交易的,地址发现用. */
export async function fetchUsed(network: string, addresses: readonly string[]): Promise<string[]> {
    const body = await post(network, "used", { addresses });
    const list = isRecord(body) ? body.used : null;
    if (!Array.isArray(list)) throw malformed();
    return list.filter((address): address is string => typeof address === "string");
}

export interface SnapshotItem {
    network: string;
    address: string;
    /** 没有时是原生币. */
    contract?: string;
    since?: string;
    /** 分批编号.翻页游标绑定一批地址,水位相同的批次也不能合并. */
    group: number;
}

/** 一批地址的最新状态.`balances` 和 `history` 分开报告失败,空表示这部分没查到. */
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
        // 响应不完整时算查询失败,不能当成余额为 0.
        balances: Array.isArray(value.balances) ? parseBalances(value.balances, addresses) : null,
        history: isRecord(value.history) ? parsePage(value.history) : null,
        error: typeof value.error === "string" ? cleanText(value.error) : null,
    };
}

/** 响应一行一个 JSON,哪批先查完就先到,到一批处理一批.慢的网络不拖累其他网络. */
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

/** 跟块推送.`items` 为空表示网关这次没取到交易,要自己补取. */
export interface ChainUpdate {
    height: number;
    /** 上一条推送的高度,接不上说明中间漏了. */
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

/** 键统一成 `网络:合约`,EVM 合约转小写.`/prices` 和 `prices` 频道共用. */
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

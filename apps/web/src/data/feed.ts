// 链上状态的本地缓存：余额、交易记录和同步游标，见 docs/03-system.md 第 3 节。
// 网页端是一个有状态的小客户端：先订阅，再取快照，两边的结果都按同样的规则写入本地，谁先到都一样。
// 余额带高度，不低于本地才覆盖。交易记录按标识写入，重复收到没有影响。
// 水位是已经不可回滚的位置，水位之后的记录每次都以链上结果为准，这样被重组掉的交易会消失。
import { networkList } from "../core";
import {
    GatewayError,
    MAX_SNAPSHOT_ITEMS,
    addressKey,
    fetchHistory,
    fetchSnapshot,
    fetchTxs,
    type Balance,
    type ChainUpdate,
    type Flows,
    type HistoryItem,
    type HistoryPage,
    type SnapshotItem,
    type SnapshotResult,
} from "../gateway/client";
import type { HistoryStream } from "../gateway/history";
import { db, txKey, type BalanceRow, type ChainEntry, type FeedCursor } from "./db";

/** 索引服务比推送慢。这么久之后快照里还没有，才算被重组掉或者被内存池丢弃。 */
const INDEX_LAG = 10 * 60_000;

const isBitcoin = (network: string) => networkList().some((n) => n.id === network && n.chain === "bitcoin");

/** 这些状态还可能被重组改变。`failed` 不算：它可能已经在水位之前，删掉就拉不回来了。 */
const OPEN: ReadonlySet<ChainEntry["status"]> = new Set(["unknown", "pending", "included"]);

export function streamId(walletId: string, { network, addresses }: HistoryStream): string {
    return `${walletId}:${network}:${addresses[0]}`;
}

export function entry(walletId: string, stream: string, network: string, item: HistoryItem): ChainEntry {
    const key = txKey(network, item.hash);
    const asset = addressKey(item.asset);
    // 本地构建的记录是校验和地址，网关返回的是小写，同一笔要算成一条。
    const party = (address: string | null) => address && addressKey(address);
    const content = `${asset}:${party(item.from)}:${party(item.to)}:${item.amount}`;
    // Bitcoin 的条目带输入输出，从哪批地址看都是同一笔，所以一笔只留一条。
    const id = isBitcoin(network) ? key : `${key}:${content}`;
    const seenAt = Date.now();
    return { ...item, id, walletId, stream, txKey: key, network, amount: item.amount.toString(), seenAt };
}

/** 显示用的方向和金额。 */
export type WalletView = Pick<ChainEntry, "from" | "to" | "amount">;

/**
 * 按整个钱包归纳一笔 Bitcoin 交易。网关只看到一批地址，找零可能在另一批里，直接用它的结果会把
 * 找零算成付款。规则和网关相同：输入里有自己的地址算转出，金额是付给别人的部分，否则算转入。
 * 没有输入输出的链和老记录用保存的结果。
 */
export function walletView(row: WalletView & { flows?: Flows }, own: ReadonlySet<string>): WalletView {
    if (!row.flows) return row;
    const { inputs, outputs } = row.flows;
    const isOwn = (address: string | null) => address !== null && own.has(addressKey(address));
    const mine = outputs.filter((output) => isOwn(output.address));
    const others = outputs.filter((output) => !isOwn(output.address));
    const total = (list: Flows["outputs"]) => list.reduce((sum, output) => sum + BigInt(output.amount), 0n).toString();
    // 数据输出没有地址，不算收款方。
    const first = (list: Flows["outputs"]) => list.find((output) => output.address !== null)?.address ?? null;
    const sender = inputs.find(isOwn);
    if (sender === undefined) return { from: inputs[0] ?? null, to: first(mine), amount: total(mine) };
    const recipient = first(others);
    if (recipient === null) return { from: sender, to: first(mine), amount: total(mine) };
    return { from: sender, to: recipient, amount: total(others) };
}

/** 同一时间只跑一轮，期间要同步的批次排队，合成下一轮。 */
export interface SyncRun<T> {
    running: boolean;
    queue: Map<string, T>;
}

/**
 * 跑完队列。一批出错不影响后面的批次，`failed` 负责记下来。`running` 留在 true 时，
 * 刷新页面之前同步都不会再运行。
 */
export async function drainQueue<T>(
    run: SyncRun<T>,
    gather: number,
    step: (batch: T[]) => Promise<void>,
    failed: (batch: T[]) => void,
): Promise<void> {
    run.running = true;
    try {
        while (run.queue.size > 0) {
            await new Promise((resolve) => setTimeout(resolve, gather));
            const batch = [...run.queue.values()];
            run.queue.clear();
            await step(batch).catch(() => failed(batch));
        }
    } finally {
        run.running = false;
    }
}

/** 高度不低于本地才覆盖，快照和推送谁先到都不会用旧值盖掉新值。 */
async function mergeBalances(network: string, height: number, rows: readonly Balance[]): Promise<void> {
    const fresh: BalanceRow[] = rows.map(({ address, asset, amount }) => ({
        id: `${network}|${addressKey(address)}|${addressKey(asset)}`,
        network,
        address,
        asset,
        amount: amount.toString(),
        height,
    }));
    const stored = await db.balances.bulkGet(fresh.map((row) => row.id));
    await db.balances.bulkPut(fresh.filter((_, i) => (stored[i]?.height ?? 0) <= height));
}

/** 本地还没有的记录。多个标签页收到同一条推送时，写入在事务里排队，只有一个能看到它是新的。 */
async function unseen(rows: readonly ChainEntry[]): Promise<ChainEntry[]> {
    const stored = await db.chainTxs.bulkGet(rows.map((row) => row.id));
    return rows.filter((_, i) => !stored[i]);
}

/** 第一次同步只取最新一页，更早的记录用户翻到时再取。 */
async function saveFirstPage(id: string, walletId: string, stream: HistoryStream, page: HistoryPage): Promise<void> {
    const { network, addresses } = stream;
    const cursor: FeedCursor = {
        id,
        network,
        addresses,
        checkpoint: page.checkpoint,
        older: page.cursor,
    };
    await db.transaction("rw", db.chainTxs, db.feedCursors, async () => {
        await db.chainTxs.bulkPut(page.items.map((item) => entry(walletId, id, network, item)));
        await db.feedCursors.put(cursor);
    });
}

/**
 * 取水位之后的变化，全部拿到后才写入，中途失败时什么都不改。未定论的记录以这次结果为准，
 * 但刚看到的记录可能只是索引还没跟上，要等一段时间。等待期间水位不动，否则水位越过它之后，
 * 增量再也取不到这笔交易。
 */
async function saveChanges(
    id: string,
    walletId: string,
    stream: HistoryStream,
    saved: FeedCursor,
    first: HistoryPage,
): Promise<ChainEntry[]> {
    const { network, addresses } = stream;
    const rows = first.items.map((item) => entry(walletId, id, network, item));
    let cursor = first.cursor;
    while (cursor !== null) {
        const page = await fetchHistory(network, addresses, cursor, saved.checkpoint);
        rows.push(...page.items.map((item) => entry(walletId, id, network, item)));
        cursor = page.cursor;
    }
    return db.transaction("rw", db.chainTxs, db.feedCursors, async () => {
        const fresh = await unseen(rows);
        const found = new Set(rows.map((row) => row.id));
        const stored = await db.chainTxs.where("walletId").equals(walletId).toArray();
        const missing = stored.filter((row) => row.stream === id && OPEN.has(row.status) && !found.has(row.id));
        const expired = missing.filter((row) => Date.now() - (row.seenAt ?? 0) >= INDEX_LAG);
        await db.chainTxs.bulkDelete(expired.map((row) => row.id));
        await db.chainTxs.bulkPut(rows);
        if (expired.length === missing.length) await db.feedCursors.update(id, { checkpoint: first.checkpoint });
        return fresh;
    });
}

function snapshotItems(
    stream: HistoryStream,
    group: number,
    saved: FeedCursor | undefined,
    assets: readonly string[],
): SnapshotItem[] {
    const since = saved?.checkpoint ? { since: saved.checkpoint } : {};
    return stream.addresses.flatMap((address) =>
        assets.map((asset) => ({
            network: stream.network,
            address,
            group,
            ...since,
            ...(asset === "native" ? {} : { contract: asset }),
        })),
    );
}

/** 一次请求放不下时分几次，同一批地址不拆开。 */
function batches(groups: readonly SnapshotItem[][]): SnapshotItem[][] {
    const packed: SnapshotItem[][] = [];
    let current: SnapshotItem[] = [];
    for (const items of groups) {
        if (current.length + items.length > MAX_SNAPSHOT_ITEMS && current.length > 0) {
            packed.push(current);
            current = [];
        }
        current.push(...items);
    }
    if (current.length > 0) packed.push(current);
    return packed;
}

/** 没有索引服务的网络收不到增量结果，未定论的记录逐笔查状态。 */
async function settleUnindexed(walletId: string, networks: readonly string[]): Promise<void> {
    const open = db.chainTxs.where("walletId").equals(walletId);
    const rows = await open.filter((row) => networks.includes(row.network) && OPEN.has(row.status)).toArray();
    for (const network of new Set(rows.map((row) => row.network))) {
        const group = rows.filter((row) => row.network === network);
        const seen = await fetchTxs(network, [...new Set(group.map((row) => row.hash))]).catch(() => []);
        const status = new Map(seen.map((tx) => [addressKey(tx.hash), tx.status]));
        await db.chainTxs.bulkPut(
            group.map((row) => ({ ...row, status: status.get(addressKey(row.hash)) ?? row.status })),
        );
    }
}

export interface FeedFailure {
    network: string;
    code: string;
}

export interface FeedResult {
    /** 余额没查到的网络。 */
    failures: FeedFailure[];
    /** 链上记录没同步成的网络。 */
    failed: string[];
    /** 增量补回来的新记录，第一次同步的不算。 */
    missed: ChainEntry[];
}

async function applyResult(
    walletId: string,
    stream: HistoryStream,
    saved: FeedCursor | undefined,
    result: SnapshotResult,
    missed: ChainEntry[],
): Promise<boolean> {
    if (result.height !== null && result.balances) await mergeBalances(stream.network, result.height, result.balances);
    const page = result.history;
    if (!page) return false;
    const id = streamId(walletId, stream);
    if (!saved) await saveFirstPage(id, walletId, stream, page);
    else if (!page.unchanged) missed.push(...(await saveChanges(id, walletId, stream, saved, page)));
    return true;
}

/** 取一次全部网络的最新状态并合并到本地。一个网络失败不影响其他网络。 */
export async function syncFeed(
    walletId: string,
    streams: readonly HistoryStream[],
    assets: (network: string) => readonly string[],
    noHistory: readonly string[] = [],
): Promise<FeedResult> {
    const saved = await db.feedCursors.bulkGet(streams.map((stream) => streamId(walletId, stream)));
    const groups = streams.map((stream, group) => snapshotItems(stream, group, saved[group], assets(stream.network)));

    const failures = new Map<string, string>();
    const failed = new Set<string>();
    const answered = new Set<string>();
    const missed: ChainEntry[] = [];
    const apply = async (result: SnapshotResult) => {
        const index = streams.findIndex(
            (stream) => stream.network === result.network && stream.addresses[0] === result.addresses[0],
        );
        const stream = streams[index];
        if (!stream) return;
        answered.add(streamId(walletId, stream));
        if (result.balances === null) failures.set(stream.network, result.error ?? "unreachable");
        const indexed = !noHistory.includes(stream.network);
        const synced = await applyResult(walletId, stream, saved[index], result, missed).catch(() => false);
        if (indexed && !synced) failed.add(stream.network);
    };
    const replies = await Promise.allSettled(batches(groups).map((items) => fetchSnapshot(items, apply)));
    const lost = replies.find((reply) => reply.status === "rejected");
    const code = lost?.status === "rejected" && lost.reason instanceof GatewayError ? lost.reason.code : "unreachable";
    for (const stream of streams) {
        if (answered.has(streamId(walletId, stream))) continue;
        failures.set(stream.network, code);
        if (!noHistory.includes(stream.network)) failed.add(stream.network);
    }
    await settleUnindexed(walletId, noHistory);
    return { failures: [...failures].map(([network, code]) => ({ network, code })), failed: [...failed], missed };
}

/** 把一条跟块推送写入本地，返回本地原来没有的记录。 */
export async function applyChainUpdate(
    walletId: string,
    stream: HistoryStream,
    update: ChainUpdate,
): Promise<ChainEntry[]> {
    const { network } = stream;
    const id = streamId(walletId, stream);
    const rows = (update.items ?? []).map((item) => entry(walletId, id, network, item));
    return db.transaction("rw", db.balances, db.chainTxs, async () => {
        await mergeBalances(network, update.height, update.balances);
        const fresh = await unseen(rows);
        await db.chainTxs.bulkPut(rows);
        return fresh;
    });
}

/** 往前翻一页。返回取到新记录的批次数。 */
export async function loadOlder(walletId: string, streams: readonly HistoryStream[]): Promise<void> {
    await Promise.allSettled(
        streams.map(async (stream) => {
            const id = streamId(walletId, stream);
            const saved = await db.feedCursors.get(id);
            if (!saved?.older) return;
            try {
                const page = await fetchHistory(stream.network, stream.addresses, saved.older);
                await db.transaction("rw", db.chainTxs, db.feedCursors, async () => {
                    await db.chainTxs.bulkPut(page.items.map((item) => entry(walletId, id, stream.network, item)));
                    await db.feedCursors.update(id, { older: page.cursor });
                });
            } catch (error) {
                // 翻页游标过期时清掉，下次从最新一页重来，已有的记录不受影响。
                if (error instanceof GatewayError && error.code === "invalid_request") {
                    await db.feedCursors.delete(id);
                }
                throw error;
            }
        }),
    );
}

/** 还有未定论的记录时要继续同步，否则它们的状态不会更新。 */
export async function hasOpenEntries(walletId: string, stream: HistoryStream): Promise<boolean> {
    const id = streamId(walletId, stream);
    const open = db.chainTxs.where("walletId").equals(walletId);
    return (await open.filter((row) => row.stream === id && OPEN.has(row.status)).count()) > 0;
}

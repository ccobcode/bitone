// 链上历史的本地缓存和增量同步，见 docs/03-system.md 第 3 节。
// 做法和 delta token 相同：网关不存状态，水位由浏览器保存，下次带上它只取之后的变化。
// 水位是已经不可回滚的位置，水位之后的记录每次都以链上结果为准，这样被重组掉的交易会消失。
import { networkList } from "../core";
import { GatewayError, addressKey, fetchHistory, type HistoryItem } from "../gateway/client";
import type { HistoryStream } from "../gateway/history";
import { db, txKey, type ChainEntry, type FeedCursor } from "./db";

const isBitcoin = (network: string) => networkList().some((n) => n.id === network && n.chain === "bitcoin");

/** 这些状态还可能被重组改变。`failed` 不算：它可能已经在水位之前，删掉就拉不回来了。 */
const OPEN: ReadonlySet<ChainEntry["status"]> = new Set(["unknown", "pending", "included"]);

export function streamId(walletId: string, { network, addresses }: HistoryStream): string {
    return `${walletId}:${network}:${addresses[0]}`;
}

function entry(walletId: string, stream: string, network: string, item: HistoryItem): ChainEntry {
    const key = txKey(network, item.hash);
    const asset = addressKey(item.asset);
    // Bitcoin 的条目按一批地址归纳，地址变多后金额会变，所以一笔只留一条。
    const id = isBitcoin(network) ? key : `${key}:${asset}:${item.from}:${item.to}:${item.amount}`;
    return { ...item, id, walletId, stream, txKey: key, network, amount: item.amount.toString() };
}

/** 第一次下载。第一页的水位最早，下载途中上链的交易下次增量时能补上。边下边存，中断后续传。 */
async function download(id: string, walletId: string, stream: HistoryStream, resume: FeedCursor | null): Promise<void> {
    const { network, addresses } = stream;
    const size = addresses.length;
    let checkpoint = resume?.checkpoint ?? null;
    let cursor = resume?.older ?? null;
    do {
        const page = await fetchHistory(network, addresses, cursor);
        checkpoint ??= page.checkpoint;
        await db.chainTxs.bulkPut(page.items.map((item) => entry(walletId, id, network, item)));
        cursor = page.cursor;
        await db.feedCursors.put({ id, size, checkpoint, older: cursor, done: cursor === null });
    } while (cursor !== null);
}

/** 取水位之后的变化，全部拿到后才写入。未定论的记录以这次结果为准，中途失败时什么都不改。 */
async function update(id: string, walletId: string, stream: HistoryStream, saved: FeedCursor): Promise<void> {
    const { network, addresses } = stream;
    const rows: ChainEntry[] = [];
    let checkpoint: string | null = null;
    let cursor: string | null = null;
    do {
        const page = await fetchHistory(network, addresses, cursor, saved.checkpoint);
        if (page.unchanged) return;
        checkpoint ??= page.checkpoint;
        rows.push(...page.items.map((item) => entry(walletId, id, network, item)));
        cursor = page.cursor;
    } while (cursor !== null);
    await db.transaction("rw", db.chainTxs, db.feedCursors, async () => {
        const open = await db.chainTxs.where("walletId").equals(walletId);
        await open.filter((row) => row.stream === id && OPEN.has(row.status)).delete();
        await db.chainTxs.bulkPut(rows);
        await db.feedCursors.update(id, { checkpoint: checkpoint ?? saved.checkpoint });
    });
}

async function syncStream(walletId: string, stream: HistoryStream): Promise<void> {
    const id = streamId(walletId, stream);
    const saved = await db.feedCursors.get(id);
    // 地址变多后，新地址的历史还没下载过，水位对它们不成立，要从头下载。
    const usable = saved?.size === stream.addresses.length ? saved : null;
    if (usable) await update(id, walletId, stream, usable);
    if (usable?.done) return;
    try {
        // 增量刚推进了水位，要读最新的。
        await download(id, walletId, stream, usable ? ((await db.feedCursors.get(id)) ?? null) : null);
    } catch (error) {
        // 续传游标过期时清掉，下次从头下载，已有的记录不受影响。
        if (usable && error instanceof GatewayError && error.code === "invalid_request") {
            await db.feedCursors.delete(id);
        }
        throw error;
    }
}

/** 同步全部地址，返回失败的网络。一个网络失败不影响其他网络。 */
export async function syncFeed(walletId: string, streams: readonly HistoryStream[]): Promise<string[]> {
    const results = await Promise.allSettled(streams.map((stream) => syncStream(walletId, stream)));
    return [...new Set(streams.filter((_, i) => results[i]?.status === "rejected").map((stream) => stream.network))];
}

/** 还有未定论的记录时要继续同步，否则它们的状态不会更新。 */
export async function hasOpenEntries(walletId: string): Promise<boolean> {
    const open = db.chainTxs.where("walletId").equals(walletId);
    return (await open.filter((row) => OPEN.has(row.status)).count()) > 0;
}

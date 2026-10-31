// 通知什么,通知谁.只有实时到来的新事件才打扰用户:
// 快照,翻页和地址发现带来的历史记录不算,粉尘和清单外代币不算,它们常被用来做地址投毒.
// 通知只用来跳转查看,不带任何操作.到账要看确认状态,提示音不能当成到账依据.
import { networkList } from "../core";
import { t } from "../core/i18n";
import { formatAsset, lookupAsset, priceKey } from "../core/manifest";
import { addressKey, type PriceMap } from "../gateway/client";
import { db, type ChainEntry } from "./db";
import { walletView } from "./feed";

export type NoticeKind = "incoming" | "included" | "failed";

export interface Notice {
    kind: NoticeKind;
    network: string;
    /** 页面里显示的文字,可以带金额. */
    title: string;
    /** 系统通知默认用的文字,不带金额,它可能出现在锁屏或屏幕共享里. */
    summary: string;
    link: string;
    /** 这条代表几笔,合并时累加. */
    count: number;
}

/** 低于这个美元价值的转入不通知. */
const DUST_USD = 1;

export function isTestnet(network: string): boolean {
    return networkList().some((n) => n.id === network && n.testnet);
}

function isDust(row: ChainEntry, decimals: number, prices: PriceMap | null): boolean {
    const price = prices?.get(priceKey(row.network, row.asset));
    if (price === undefined) return false;
    return (Number(row.amount) / 10 ** decimals) * Number(price) < DUST_USD;
}

/** 别人转给这个钱包的,清单内的,有实际金额的转账. */
function worthTelling(row: ChainEntry, own: ReadonlySet<string>, prices: PriceMap | null): boolean {
    if (row.status === "failed" || !row.to || !own.has(addressKey(row.to))) return false;
    if (row.from && own.has(addressKey(row.from))) return false;
    const entry = lookupAsset(row.network, row.asset);
    return entry !== null && BigInt(row.amount) > 0n && !isDust(row, entry.decimals, prices);
}

function incomingNotice(row: ChainEntry): Notice {
    const amount = formatAsset(row.network, row.asset, BigInt(row.amount));
    return {
        kind: "incoming",
        network: row.network,
        title: row.status === "finalized" ? t("收到 {0}", amount) : t("收到 {0},确认中", amount),
        summary: t("收到一笔转账"),
        link: "/history",
        count: 1,
    };
}

export function mergedNotice(first: Notice, count: number): Notice {
    const title = t("收到 {0} 笔转账", count);
    return { ...first, title, summary: title, count };
}

/** 推送带来的新记录,一笔交易只算一次. */
export function incomingNotices(
    rows: readonly ChainEntry[],
    own: ReadonlySet<string>,
    prices: PriceMap | null,
): Notice[] {
    const viewed = rows.map((row) => ({ ...row, ...walletView(row, own) }));
    const unique = new Map(viewed.filter((row) => worthTelling(row, own, prices)).map((row) => [row.txKey, row]));
    return [...unique.values()].map(incomingNotice);
}

/** 增量补回来的新记录,也就是用户不在时发生的.合并成一条. */
export function missedNotice(
    rows: readonly ChainEntry[],
    own: ReadonlySet<string>,
    prices: PriceMap | null,
): Notice | null {
    const [first, ...rest] = incomingNotices(rows, own, prices);
    if (!first || rest.length === 0) return first ?? null;
    const title = t("离线期间收到 {0} 笔转账", rest.length + 1);
    return { ...first, title, summary: title, count: rest.length + 1 };
}

/** 同一件事只通知一次.多个标签页同时看到状态变化时,只有一个能认领. */
export async function claimNotice(key: string): Promise<boolean> {
    const id = `notified:${key}`;
    return db.transaction("rw", db.settings, async () => {
        if (await db.settings.get(id)) return false;
        await db.settings.put({ key: id, value: true });
        return true;
    });
}

type Listener = (notice: Notice, origin: boolean) => void;

const listeners = new Set<Listener>();
const channel = typeof BroadcastChannel === "function" ? new BroadcastChannel("bitone-notices") : null;

if (channel) {
    channel.onmessage = ({ data }: MessageEvent<Notice>) => {
        for (const listener of listeners) listener(data, false);
    };
    // Node 里打开的频道会拖住进程退出,浏览器没有这个方法.
    const { unref } = channel as unknown as { unref?(): void };
    unref?.call(channel);
}

/** 发出通知.发现它的标签页负责声音和系统通知,其他标签页只显示,用户看着哪个都不会错过. */
export function publish(notice: Notice): void {
    for (const listener of listeners) listener(notice, true);
    channel?.postMessage(notice);
}

export function onNotice(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/**
 * 弹系统通知.Android Chrome 只允许通过 Service Worker 发通知,这里的构造函数会抛错.
 * 通知发不出去只是少一次提醒,不能影响调用方,同步也在这条调用链上.
 */
export function systemNotice(body: string, onClick: () => void): void {
    try {
        const shown = new Notification("BitOne", { body });
        shown.onclick = () => {
            onClick();
            shown.close();
        };
    } catch {
        // 页面内的提示和声音不受影响.
    }
}

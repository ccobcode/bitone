// 交易记录：本地 txs 合并缓存的链上历史。本地记录优先，它的状态由状态机决定。
import { networkName } from "../core";
import { explorerLink } from "../core/explorer";
import { formatAsset, lookupAsset } from "../core/manifest";
import { shortAddress } from "../core/text";
import { txKey, type ChainEntry, type FeedCursor, type Note, type TxRecord } from "../data/db";
import { STATE_GROUPS, STATE_LABELS, failureNote, type ChainStatus, type StateGroup } from "../data/machine";
import type { Target } from "./assets";
import { MAX_ADDRESSES, addressKey, type HistoryItem } from "./client";
import { t } from "../core/i18n";

export interface HistoryStream {
    network: string;
    addresses: string[];
}

/** 已同步过的批次保持不变，翻页游标绑定它们。没同步过的地址按单次上限分成新批次。 */
export function historyStreams(
    walletId: string,
    targets: readonly Target[],
    cursors: readonly FeedCursor[],
): HistoryStream[] {
    return targets.flatMap(({ network, addresses }) => {
        const kept = cursors
            .filter((cursor) => cursor.network === network && cursor.id.startsWith(`${walletId}:`))
            .map((cursor) => ({ network, addresses: cursor.addresses }));
        const synced = new Set(kept.flatMap((stream) => stream.addresses));
        const fresh = addresses.filter((address) => !synced.has(address));
        for (let i = 0; i < fresh.length; i += MAX_ADDRESSES) {
            kept.push({ network, addresses: fresh.slice(i, i + MAX_ADDRESSES) });
        }
        return kept;
    });
}

export interface RemoteItem {
    id: string;
    network: string;
    item: HistoryItem;
}

export function remoteItem(row: ChainEntry): RemoteItem {
    return { id: row.id, network: row.network, item: { ...row, amount: BigInt(row.amount) } };
}

export type FeedTone = "neutral" | "accent" | "ok" | "warn" | "danger";

export interface FeedRow {
    key: string;
    network: string;
    title: string;
    party: string;
    amount: string;
    status: string;
    tone: FeedTone;
    group: StateGroup;
    /** 毫秒。内存池里的交易没有时间。 */
    time: number | null;
    note: string | null;
    noteTone: "warn" | "danger" | "muted";
    /** 用户写的备注，只当纯文本显示。 */
    remark: string | null;
    action: string;
    /** 站内路径，或者区块浏览器链接。 */
    href: string | null;
    external: boolean;
}

const TONES: Record<TxRecord["state"], FeedTone> = {
    signing: "accent",
    signed: "accent",
    broadcasting: "neutral",
    broadcast: "neutral",
    confirming: "neutral",
    uncertain: "warn",
    confirmed: "ok",
    failed: "danger",
    replaced: "neutral",
    expired: "neutral",
    abandoned: "neutral",
};

function localNote(tx: TxRecord): Pick<FeedRow, "note" | "noteTone"> {
    switch (tx.state) {
        case "uncertain":
            return { note: t("重试广播同一笔"), noteTone: "warn" };
        case "failed":
            return { note: failureNote(tx), noteTone: "danger" };
        case "replaced":
            return { note: t("被同一序号的交易替换"), noteTone: "muted" };
        case "expired":
            return { note: t("超过过期时间，未上链"), noteTone: "muted" };
        case "abandoned":
            return { note: tx.raw ? t("已取消，签名仍然有效") : t("已取消"), noteTone: "muted" };
        case "confirming":
            return tx.reverted
                ? { note: t("执行失败，等待最终确认"), noteTone: "danger" }
                : { note: null, noteTone: "muted" };
        default:
            return { note: null, noteTone: "muted" };
    }
}

function localTitle(tx: TxRecord): string {
    if (tx.kind === "create-nonce") return t("创建 nonce 账户");
    if (tx.kind === "cancel") return t("取消交易");
    const symbol = lookupAsset(tx.network, tx.asset)?.symbol ?? t("未知代币");
    return tx.kind === "speedup" ? t("加速 {0}", symbol) : t("转出 {0}", symbol);
}

function localRow(tx: TxRecord, remark: string | null): FeedRow {
    const sign = tx.state === "signing" || tx.state === "signed" ? "sign" : "broadcast";
    return {
        key: tx.id,
        network: tx.network,
        title: localTitle(tx),
        party: t("至 {0}", shortAddress(tx.to)),
        amount: `${tx.amount === "0" ? "" : "-"}${formatAsset(tx.network, tx.asset, BigInt(tx.amount))}`,
        status: STATE_LABELS[tx.state],
        tone: tx.reverted ? "danger" : TONES[tx.state],
        group: STATE_GROUPS[tx.state],
        time: tx.createdAt,
        ...localNote(tx),
        remark,
        action:
            tx.state === "signing" || tx.state === "signed"
                ? t("继续")
                : tx.state === "uncertain"
                  ? t("处理")
                  : t("查看"),
        href: `/${tx.state === "signed" ? "broadcast" : sign}?id=${encodeURIComponent(tx.id)}`,
        external: false,
    };
}

const REMOTE_STATES: Record<ChainStatus, Pick<FeedRow, "status" | "tone" | "group">> = {
    unknown: { status: t("已广播"), tone: "neutral", group: "进行中" },
    pending: { status: t("已广播"), tone: "neutral", group: "进行中" },
    included: { status: t("确认中"), tone: "neutral", group: "进行中" },
    finalized: { status: t("已确认"), tone: "ok", group: "已完成" },
    failed: { status: t("链上失败"), tone: "danger", group: "未完成" },
};

function remoteRow({ id, network, item }: RemoteItem, own: ReadonlySet<string>, remark: string | null): FeedRow {
    const out = item.from !== null && own.has(addressKey(item.from));
    const entry = lookupAsset(network, item.asset);
    const symbol = entry?.symbol ?? t("未知代币");
    const party = out ? item.to : item.from;
    const href = explorerLink(network, item.hash);
    return {
        key: id,
        network,
        title: `${out ? t("转出") : t("转入")} ${symbol}`,
        party: party ? `${out ? t("至") : t("来自")} ${shortAddress(party)}` : "",
        amount: `${out ? "-" : "+"}${formatAsset(network, item.asset, item.amount)}`,
        ...REMOTE_STATES[item.status],
        time: item.time === null ? null : item.time * 1000,
        note: item.status === "failed" ? t("执行失败，手续费已扣除") : null,
        noteTone: item.status === "failed" ? "danger" : "muted",
        remark,
        action: t("查看"),
        href,
        external: href !== null,
    };
}

/** 本地认为没有上链，链上却查到了。不能盖住，否则用户会再付一次。 */
const CLOSED_STATES: ReadonlySet<TxRecord["state"]> = new Set<TxRecord["state"]>(["replaced", "expired", "abandoned"]);

/**
 * 合并本地和链上记录，同一笔交易只留本地的。
 * 零金额记录是地址投毒的常见手法，转出也能伪造（`transferFrom(受害者, 投毒地址, 0)`），
 * 所以不分方向都不进列表，见 docs/04-security.md 第 4 节。
 */
export function mergeFeed(
    local: readonly TxRecord[],
    remote: readonly RemoteItem[],
    own: ReadonlySet<string>,
    notes: readonly Note[],
): FeedRow[] {
    const remarks = new Map(notes.map((note) => [note.id, note.text]));
    const known = new Set(local.flatMap((tx) => (tx.hash ? [txKey(tx.network, tx.hash)] : [])));
    const rows = local.map((tx) => localRow(tx, tx.hash ? (remarks.get(txKey(tx.network, tx.hash)) ?? null) : null));
    const closed = new Map(
        local.flatMap((tx, i) =>
            tx.hash && CLOSED_STATES.has(tx.state) ? [[txKey(tx.network, tx.hash), i] as const] : [],
        ),
    );
    for (const entry of remote) {
        const key = txKey(entry.network, entry.item.hash);
        const at = closed.get(key);
        const row = at === undefined ? undefined : rows[at];
        if (row && entry.item.status !== "unknown" && entry.item.status !== "pending") {
            rows[at as number] = {
                ...row,
                ...REMOTE_STATES[entry.item.status],
                note: t("链上查到这笔，本地记录已过时"),
                noteTone: "warn",
            };
        }
        if (known.has(key)) continue;
        if (entry.item.amount === 0n) continue;
        rows.push(remoteRow(entry, own, remarks.get(txKey(entry.network, entry.item.hash)) ?? null));
    }
    // 没有时间的是内存池里的交易，排在最前面。
    const rank = (row: FeedRow) => row.time ?? Number.MAX_SAFE_INTEGER;
    return rows.sort((a, b) => rank(b) - rank(a));
}

export function failedNames(failed: readonly string[]): string {
    return failed.map(networkName).join(t("、"));
}

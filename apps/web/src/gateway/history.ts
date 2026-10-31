// 交易记录和转账请求.记录是链上事实,请求是这个浏览器里的意图,上链之后才进记录.
import { networkName } from "../core";
import { formatAsset, lookupAsset, type AssetEntry } from "../core/manifest";
import { shortAddress } from "../core/text";
import { txKey, type ChainEntry, type FeedCursor, type Note, type TxRecord, type TxState } from "../data/db";
import { walletView } from "../data/feed";
import { STATE_LABELS, failureNote, isOverdue, type ChainStatus } from "../data/machine";
import type { Target } from "./assets";
import { MAX_ADDRESSES, addressKey, chunks, type HistoryItem } from "./client";
import { t } from "../core/i18n";

export type HistoryStream = Target;

/** 已同步过的批次保持不变,翻页游标绑定它们.没同步过的地址按单次上限分成新批次. */
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
        return [...kept, ...chunks(fresh, MAX_ADDRESSES).map((batch) => ({ network, addresses: batch }))];
    });
}

export interface RemoteItem {
    id: string;
    network: string;
    item: HistoryItem;
}

export function remoteItem(row: ChainEntry, own: ReadonlySet<string>): RemoteItem {
    const view = walletView(row, own);
    return { id: row.id, network: row.network, item: { ...row, ...view, amount: BigInt(view.amount) } };
}

export type FeedTone = "neutral" | "accent" | "ok" | "warn" | "danger";

export interface FeedRow {
    key: string;
    network: string;
    /** 内置清单的条目,清单外的代币是 null. */
    entry: AssetEntry | null;
    direction: "in" | "out";
    title: string;
    party: string;
    amount: string;
    status: string;
    tone: FeedTone;
    /** 还没有结果的交易,排在列表最前面. */
    moving: boolean;
    /** 毫秒.内存池里的交易没有时间. */
    time: number | null;
    note: string | null;
    noteTone: "warn" | "danger" | "muted";
    /** 用户写的备注,只当纯文本显示. */
    remark: string | null;
    href: string;
}

/** 上链交易的详情页.本地发起的和网关同步的是同一个地址. */
export function recordHref(network: string, hash: string): string {
    return `/history/${network}/${encodeURIComponent(txKey(network, hash).slice(network.length + 1))}`;
}

/** 本地交易的详情页.上过链的用记录的地址,其余还只是请求,用编号. */
export function txHref(tx: Pick<TxRecord, "id" | "state" | "network" | "hash">): string {
    return tx.hash && LEDGER.has(tx.state) ? recordHref(tx.network, tx.hash) : `/history/${encodeURIComponent(tx.id)}`;
}

function signHref(id: string): string {
    return `/sign/${encodeURIComponent(id)}`;
}

export const TONES: Record<TxState, FeedTone> = {
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
            return { note: t("超过过期时间,未上链"), noteTone: "muted" };
        case "abandoned":
            return tx.raw
                ? { note: t("已取消,签名仍然有效"), noteTone: "warn" }
                : { note: t("已取消"), noteTone: "muted" };
        case "confirming":
            return tx.reverted
                ? { note: t("执行失败,等待最终确认"), noteTone: "danger" }
                : { note: null, noteTone: "muted" };
        default:
            return { note: null, noteTone: "muted" };
    }
}

export function localTitle(tx: TxRecord): string {
    if (tx.kind === "create-nonce") return t("创建 nonce 账户");
    if (tx.kind === "cancel") return t("取消交易");
    const symbol = lookupAsset(tx.network, tx.asset)?.symbol ?? t("未知代币");
    return tx.kind === "speedup" ? t("加速 {0}", symbol) : t("转出 {0}", symbol);
}

function localAmount(tx: TxRecord): string {
    return `${tx.amount === "0" ? "" : "-"}${formatAsset(tx.network, tx.asset, BigInt(tx.amount))}`;
}

const MOVING: ReadonlySet<TxState> = new Set<TxState>(["broadcasting", "broadcast", "uncertain", "confirming"]);

function localRow(tx: TxRecord, remark: string | null): FeedRow {
    return {
        key: tx.id,
        network: tx.network,
        entry: lookupAsset(tx.network, tx.asset),
        direction: "out",
        title: localTitle(tx),
        party: t("至 {0}", shortAddress(tx.to)),
        amount: localAmount(tx),
        status: STATE_LABELS[tx.state],
        tone: tx.reverted ? "danger" : TONES[tx.state],
        moving: MOVING.has(tx.state),
        time: tx.createdAt,
        ...localNote(tx),
        remark,
        href: txHref(tx),
    };
}

const REMOTE_STATES: Record<ChainStatus, Pick<FeedRow, "status" | "tone" | "moving">> = {
    unknown: { status: t("已广播"), tone: "neutral", moving: true },
    pending: { status: t("已广播"), tone: "neutral", moving: true },
    included: { status: t("确认中"), tone: "neutral", moving: true },
    finalized: { status: t("已确认"), tone: "ok", moving: false },
    failed: { status: t("链上失败"), tone: "danger", moving: false },
};

function remoteRow({ id, network, item }: RemoteItem, own: ReadonlySet<string>, remark: string | null): FeedRow {
    const out = item.from !== null && own.has(addressKey(item.from));
    const entry = lookupAsset(network, item.asset);
    const symbol = entry?.symbol ?? t("未知代币");
    const party = out ? item.to : item.from;
    return {
        key: id,
        network,
        entry,
        direction: out ? "out" : "in",
        title: `${out ? t("转出") : t("转入")} ${symbol}`,
        party: party ? `${out ? t("至") : t("来自")} ${shortAddress(party)}` : "",
        amount: `${out ? "-" : "+"}${formatAsset(network, item.asset, item.amount)}`,
        ...REMOTE_STATES[item.status],
        time: item.time === null ? null : item.time * 1000,
        note: item.status === "failed" ? t("执行失败,手续费已扣除") : null,
        noteTone: item.status === "failed" ? "danger" : "muted",
        remark,
        href: recordHref(network, item.hash),
    };
}

/** 进了记录的本地状态.其余要么还没发出去,要么没上链就结束了. */
const LEDGER: ReadonlySet<TxState> = new Set<TxState>([
    "broadcasting",
    "broadcast",
    "uncertain",
    "confirming",
    "confirmed",
    "failed",
]);

/** 本地认为没上链,链上却查到了.不能盖住,否则用户会再付一次. */
const CLOSED: ReadonlySet<TxState> = new Set<TxState>(["replaced", "expired", "abandoned"]);

function onChain(status: ChainStatus): boolean {
    return status !== "unknown" && status !== "pending";
}

function sortRows(rows: FeedRow[]): FeedRow[] {
    // 没有时间的是内存池里的交易,排在最前面.
    const rank = (row: FeedRow) => row.time ?? Number.MAX_SAFE_INTEGER;
    return rows.sort((a, b) => rank(b) - rank(a));
}

/**
 * 交易记录:上过链或者正在上链的交易,同一笔只留本地的.
 * 零金额记录是地址投毒的常见手法,转出也能伪造(`transferFrom(受害者, 投毒地址, 0)`),
 * 所以不分方向都不进列表,见 docs/04-security.md 第 4 节.
 */
export function ledgerRows(
    local: readonly TxRecord[],
    remote: readonly RemoteItem[],
    own: ReadonlySet<string>,
    notes: readonly Note[],
): FeedRow[] {
    const remarks = new Map(notes.map((note) => [note.id, note.text]));
    const hashed = local.flatMap((tx) => (tx.hash ? [{ tx, key: txKey(tx.network, tx.hash) }] : []));
    const listed = new Set(hashed.filter(({ tx }) => LEDGER.has(tx.state)).map(({ key }) => key));
    const offline = new Set(hashed.filter(({ tx }) => !LEDGER.has(tx.state)).map(({ key }) => key));
    const rows = local
        .filter((tx) => LEDGER.has(tx.state))
        .map((tx) => localRow(tx, tx.hash ? (remarks.get(txKey(tx.network, tx.hash)) ?? null) : null));
    for (const entry of remote) {
        const key = txKey(entry.network, entry.item.hash);
        if (listed.has(key) || entry.item.amount === 0n) continue;
        const stale = offline.has(key);
        if (stale && !onChain(entry.item.status)) continue;
        const row = remoteRow(entry, own, remarks.get(key) ?? null);
        rows.push(stale ? { ...row, note: t("链上查到这笔,本地记录已过时"), noteTone: "warn" } : row);
    }
    return sortRows(rows);
}

/** 没上链就结束的请求.不是资金流动,所以不进记录,签名仍然有效的要一直看得到. */
export function unsentRows(local: readonly TxRecord[], remote: readonly RemoteItem[]): FeedRow[] {
    const seen = new Map(
        remote.flatMap(({ network, item }) => (onChain(item.status) ? [[txKey(network, item.hash), item.status]] : [])),
    ) as Map<string, ChainStatus>;
    return sortRows(
        local
            .filter((tx) => CLOSED.has(tx.state))
            .map((tx) => {
                // 这笔没有发生,金额不带符号,不能看起来像一笔转出.
                const row = { ...localRow(tx, null), amount: formatAsset(tx.network, tx.asset, BigInt(tx.amount)) };
                const status = tx.hash ? seen.get(txKey(tx.network, tx.hash)) : undefined;
                if (!status) return row;
                return {
                    ...row,
                    ...REMOTE_STATES[status],
                    note: t("链上查到这笔,本地记录已过时"),
                    noteTone: "warn" as const,
                };
            }),
    );
}

export interface AttentionRow {
    key: string;
    network: string;
    entry: AssetEntry | null;
    title: string;
    party: string;
    status: string;
    tone: "accent" | "warn" | "danger";
    action: string;
    href: string;
}

function attention(
    tx: TxRecord,
    seenAt: number,
    now: number,
): Pick<AttentionRow, "status" | "tone" | "action" | "href"> | null {
    const label = STATE_LABELS[tx.state];
    switch (tx.state) {
        case "signing":
            return { status: label, tone: "accent", action: t("继续"), href: signHref(tx.id) };
        case "signed":
            return { status: label, tone: "accent", action: t("广播"), href: signHref(tx.id) };
        case "uncertain":
            return { status: label, tone: "warn", action: t("处理"), href: txHref(tx) };
        case "broadcast":
            return isOverdue(tx, now)
                ? { status: t("等待时间过长"), tone: "warn", action: t("处理"), href: txHref(tx) }
                : null;
        case "failed":
            return tx.updatedAt > seenAt
                ? { status: label, tone: "danger", action: t("查看"), href: txHref(tx) }
                : null;
        default:
            return null;
    }
}

/** 等着用户动手的事.顶栏和首页共用这一个口径.seenAt 之前的链上失败已经看过,不再提示. */
export function attentionRows(txs: readonly TxRecord[], seenAt: number, now: number): AttentionRow[] {
    return [...txs]
        .sort((a, b) => b.createdAt - a.createdAt)
        .flatMap((tx) => {
            const state = attention(tx, seenAt, now);
            if (!state) return [];
            const { title, party, entry } = localRow(tx, null);
            return [{ key: tx.id, network: tx.network, entry, title, party, ...state }];
        });
}

export function failedNames(failed: readonly string[]): string {
    return failed.map(networkName).join(t(","));
}

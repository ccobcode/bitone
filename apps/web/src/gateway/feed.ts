// 交易记录和待处理共用的查询。纯函数在 history.ts，这里只把本地数据接上去。
import { useMemo } from "react";
import { saveSetting, useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { addressKey } from "./client";
import { attentionRows, ledgerRows, remoteItem, unsentRows, type AttentionRow, type FeedRow } from "./history";

export function useWalletTxs() {
    const { wallet } = useApp();
    return useLive(async () => (wallet ? db.txs.where("walletId").equals(wallet.id).toArray() : []), [wallet]);
}

/** 顶栏和首页共用的待处理。看过交易记录后，不再提示那之前的链上失败。 */
export function useAttention(): AttentionRow[] {
    const txs = useWalletTxs();
    const seenAt = useLive(async () => (await db.settings.get("historySeenAt"))?.value, []);
    return useMemo(() => attentionRows(txs ?? [], typeof seenAt === "number" ? seenAt : 0, Date.now()), [txs, seenAt]);
}

/** 看过这个时间之前的链上失败，顶栏和首页不再提示。 */
export async function markSeen(time: number): Promise<void> {
    const seen = (await db.settings.get("historySeenAt"))?.value;
    if (typeof seen !== "number" || seen < time) await saveSetting("historySeenAt", time);
}

export interface Feed {
    ledger: FeedRow[];
    unsent: FeedRow[];
    /** 已经下载到本地的链上记录数。 */
    downloaded: number;
}

export function useFeed(): Feed {
    const { wallet, addresses } = useApp();
    const local = useWalletTxs();
    const notes = useLive(() => db.notes.toArray(), []);
    const chain = useLive(
        async () => (wallet ? db.chainTxs.where("walletId").equals(wallet.id).toArray() : []),
        [wallet],
    );
    return useMemo(() => {
        const remote = (chain ?? []).map(remoteItem);
        const own = new Set(addresses.map((row) => addressKey(row.address)));
        return {
            ledger: ledgerRows(local ?? [], remote, own, notes ?? []),
            unsent: unsentRows(local ?? [], remote),
            downloaded: (chain ?? []).length,
        };
    }, [local, chain, addresses, notes]);
}

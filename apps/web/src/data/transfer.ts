// 构建待签名交易.各链的数据和编码在 `chains/` 里,这里统一加锁和写入.
// 顺序固定:在账户锁里取链上数据,分配序号,构建,全部通过后才写入占用和 txs.
// 任何一步失败都什么也不写.预览只是提示,不算失败,判断以冷钱包屏幕为准.
import { chainOf, fetchFees } from "../chains";
import type { Built } from "../chains/spec";
import {
    TransferError,
    type RawDraft,
    type RawValues,
    type ReplaceDraft,
    type ReplaceMode,
    type SetupDraft,
    type TransferDraft,
} from "../chains/transfer";
import { GatewayError, addressKey, broadcastRaw } from "../gateway/client";
import { sharedClaims } from "./claims";
import { db, type TxRecord, type TxState } from "./db";
import { mayStillLand } from "./machine";
import { beginBroadcast, cancelTx, finishBroadcast, insertTx, withAccountLock } from "./txs";
import { t } from "../core/i18n";

/**
 * 同样的转账还可能上链时不让再发.那一笔可能已经上链,新交易用的是下一个序号,两笔都会生效.
 * 重建走 createReplacement,复用原来的序号.
 */
async function rejectTwin({ account, network, asset, to, amount }: TransferDraft): Promise<void> {
    const rows = await db.txs.where("state").anyOf("uncertain", "replaced").toArray();
    const twin = rows.find(
        (tx) =>
            mayStillLand(tx) &&
            tx.accountId === account.id &&
            tx.network === network &&
            addressKey(tx.asset) === addressKey(asset) &&
            addressKey(tx.to) === addressKey(to) &&
            tx.amount === amount.toString(),
    );
    if (twin) throw new TransferError("duplicate", t("同样的转账 {0} 结果不明,先处理那一笔.", twin.id));
}

function save({ fields, claims }: Built): Promise<TxRecord> {
    return insertTx(fields, claims);
}

export function createTransfer(draft: TransferDraft): Promise<TxRecord> {
    return withAccountLock(draft.account.id, async () => {
        await rejectTwin(draft);
        return save(await chainOf(draft.account.chain).transfer(draft, fetchFees));
    });
}

/** 这条链首次转账前的准备步骤. */
export function createSetup(draft: SetupDraft): Promise<TxRecord> {
    const { setup } = chainOf(draft.account.chain);
    if (!setup) return Promise.reject(new TransferError("unsupported", t("这笔交易不能签名.")));
    return withAccountLock(draft.account.id, async () => save(await setup(draft)));
}

/** 原始请求页.内容由各链解释,之后的二维码,核验和广播都和转账相同. */
export function createRaw(draft: RawDraft, mode: string, values: RawValues): Promise<TxRecord> {
    const { raw } = chainOf(draft.account.chain);
    return withAccountLock(draft.account.id, async () => save(await raw(draft, mode, values, fetchFees)));
}

/** 已上链的交易不能再替换.已放弃但已签名的交易仍然有效,可以发替换交易作废. */
const REPLACEABLE: ReadonlySet<TxState> = new Set<TxState>([
    "signing",
    "signed",
    "broadcast",
    "uncertain",
    "abandoned",
]);

/** 生成替换交易或重建交易.复用原交易的 nonce 或同一组 UTXO,保证最多一笔上链. */
export function createReplacement(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft): Promise<TxRecord> {
    const { replace } = chainOf(tx.chain);
    if ("refusal" in replace) return Promise.reject(new TransferError("unsupported", replace.refusal()));
    return withAccountLock(tx.accountId, async () => {
        const current = await db.txs.get(tx.id);
        if (!current || !REPLACEABLE.has(current.state))
            throw new TransferError("invalid", t("这笔交易已经上链或结束,不能替换."));
        const claims = await sharedClaims(current);
        if (claims.length === 0) throw new TransferError("invalid", t("这笔交易没有可复用的序号."));
        const built = await replace.build(current, mode, draft, fetchFees);
        return insertTx({ ...built.fields, replaces: current.id }, built.claims, claims);
    });
}

/** W4 取消后发替换交易:先写入新请求占住序号,再取消原交易,中间不会释放. */
export async function replaceAndCancel(tx: TxRecord, draft: ReplaceDraft): Promise<TxRecord> {
    const next = await createReplacement(tx, "cancel", draft);
    await cancelTx(tx.id, t("用户取消,已生成替换交易"));
    return next;
}

/**
 * 再发一次同一笔,不改变状态.Solana 的节点经常丢弃交易,durable nonce 没有推进时原交易一直有效,
 * 重发是幂等的.结果由跟踪逻辑从链上判断.
 */
export async function resendTx(tx: TxRecord): Promise<void> {
    if (tx.raw !== undefined) await broadcastRaw(tx.network, tx.raw);
}

const BROADCAST_FAILURES: Record<string, string> = {
    unreachable: t("连接失败"),
    upstream_unavailable: t("网络超时"),
    rate_limited: t("请求过于频繁"),
};

/** 广播同一笔.被拒绝和超时都不是定论,只有节点接收才算已广播. */
export async function broadcastTx(id: string): Promise<TxRecord> {
    const tx = await beginBroadcast(id);
    if (tx.raw === undefined) throw new Error(t("交易还没有签名"));
    try {
        const hash = await broadcastRaw(tx.network, tx.raw);
        const same = chainOf(tx.chain).caseSensitiveHash
            ? hash === tx.hash
            : hash.toLowerCase() === tx.hash?.toLowerCase();
        return await finishBroadcast(id, same ? null : t("返回的交易哈希与本地不一致"));
    } catch (error) {
        const detail =
            error instanceof GatewayError
                ? error.detail || (BROADCAST_FAILURES[error.code] ?? error.code)
                : t("连接失败");
        return finishBroadcast(id, detail);
    }
}

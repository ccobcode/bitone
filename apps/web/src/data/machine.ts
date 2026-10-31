// 交易状态机,状态图见 docs/03-system.md 第 7 节.这里只有纯函数,写库在 txs.ts.
import { chainOf } from "../chains";
import type { TxRecord, TxState } from "./db";
import { t } from "../core/i18n";

export type TxEvent =
    | "signed"
    | "cancel"
    | "local-expire"
    | "submit"
    | "accepted"
    | "rejected"
    | "retry"
    | "seen"
    | "replaced"
    | "expired"
    | "included"
    | "reorg"
    | "finalized"
    | "reverted";

export const TRANSITIONS: Record<TxState, Partial<Record<TxEvent, TxState>>> = {
    signing: { signed: "signed", cancel: "abandoned", "local-expire": "abandoned" },
    signed: { cancel: "abandoned", submit: "broadcasting", expired: "expired" },
    broadcasting: { accepted: "broadcast", rejected: "uncertain" },
    uncertain: { retry: "broadcasting", seen: "broadcast", replaced: "replaced", expired: "expired" },
    broadcast: { included: "confirming", replaced: "replaced", expired: "expired" },
    confirming: { reorg: "broadcast", finalized: "confirmed", reverted: "failed" },
    replaced: { included: "confirming" },
    confirmed: {},
    failed: {},
    abandoned: {},
    expired: {},
};

export class IllegalTransition extends Error {
    constructor(state: TxState, event: TxEvent) {
        super(t("状态 {0} 不接受 {1}", state, event));
    }
}

export function transition(tx: TxRecord, event: TxEvent, now: number, note?: string): TxRecord {
    const state = TRANSITIONS[tx.state][event];
    if (!state) throw new IllegalTransition(tx.state, event);
    return {
        ...tx,
        state,
        updatedAt: now,
        timeline: [...tx.timeline, note ? { state, at: now, note } : { state, at: now }],
    };
}

export type ChainStatus = "unknown" | "pending" | "included" | "finalized" | "failed";

/** 网关 `/tx` 的一条结果.只有链上数据才算定论. */
export interface ChainObservation {
    hash: string;
    status: ChainStatus;
    stage: string | null;
    confirmations: number;
    block: number | null;
    fee: string | null;
}

export interface ObserveContext {
    now: number;
    /**
     * 这一轮有没有看到替换证据:EVM 的 nonce 已被用掉,Bitcoin 的输入已被花掉,Solana 的 nonce 已推进.
     * 不知道时是 undefined,比如 WebSocket 推送,这时计数不变.
     */
    conflict?: boolean;
}

/** 连续这么多轮看到证据才算被替换.上游之间有延迟,这一步判错没有回头路. */
export const CONFLICT_ROUNDS = 2;
/** 两轮之间至少隔这么久.重连和新订阅都会触发轮询,挨着的两次查询不算两轮. */
export const CONFLICT_SPACING = 10_000;

/** 过期时间过后再等一会儿,节点的时钟和打包都可能晚一点. */
export const EXPIRY_GRACE = 60_000;

/** 被替换后继续看这么久,足够覆盖各条链的最终确定时间. */
export const REPLACED_WATCH = 3 * 60 * 60_000;

const TRACKED: ReadonlySet<TxState> = new Set<TxState>(["broadcast", "uncertain", "confirming"]);

export function isTracked(tx: Pick<TxRecord, "state">): boolean {
    return TRACKED.has(tx.state);
}

/** 要继续查询链上状态的交易. */
export function isWatched(tx: Pick<TxRecord, "state" | "updatedAt">, now: number): boolean {
    return isTracked(tx) || (tx.state === "replaced" && now - tx.updatedAt < REPLACED_WATCH);
}

/** 回滚的结果也可能被重组,到这些阶段才算定论. */
function isFinalStage(stage: string | null): boolean {
    return stage === null || stage === "finalized";
}

/** 链上失败的说明. */
export function failureNote(tx: Pick<TxRecord, "chain">): string {
    return chainOf(tx.chain).failureNote;
}

/** 把链上状态折算成事件.一次观察可能走好几步,比如离线期间已经最终确定. */
export function observe(tx: TxRecord, seen: ChainObservation, { now, conflict }: ObserveContext): TxRecord {
    if (!isTracked(tx) && tx.state !== "replaced") return tx;
    const onChain = seen.status === "included" || seen.status === "finalized" || seen.status === "failed";
    if (tx.state === "replaced" && !onChain) return tx;

    let next = tx;
    const fire = (event: TxEvent, note?: string) => {
        next = transition(next, event, now, note);
    };

    if (onChain) {
        if (next.state === "replaced") fire("included", "链上查到这笔,之前的替换判定作废");
        if (next.state === "uncertain") fire("seen", "链上查到这笔交易");
        if (next.state === "broadcast") fire("included", seen.block === null ? undefined : `区块 ${seen.block}`);
        if (seen.status === "finalized") fire("finalized");
        // 失败优先于阶段,不能显示成成功.但回滚也可能被重组,到最终阶段才进入终态,
        // 否则用户看到"没有转出"后重新发起,重组后两笔都可能生效.
        if (seen.status === "failed" && isFinalStage(seen.stage)) fire("reverted", failureNote(tx));
        next = { ...next, conflicts: 0, reverted: seen.status === "failed" };
    } else {
        if (next.state === "confirming") fire("reorg", "区块重组,回到等待打包");
        if (next.reverted) next = { ...next, reverted: false };
        if (seen.status === "pending") {
            if (next.state === "uncertain") fire("seen", "在网络中查到这笔交易");
            if (next.conflicts !== 0) next = { ...next, conflicts: 0 };
        } else {
            const expired = next.hardExpiry && next.expiresAt !== null && now > next.expiresAt + EXPIRY_GRACE;
            const spaced = now - (next.conflictAt ?? 0) >= CONFLICT_SPACING;
            if (conflict === false && !expired && next.conflicts !== 0) next = { ...next, conflicts: 0 };
            if ((conflict === true || expired) && spaced)
                next = { ...next, conflicts: next.conflicts + 1, conflictAt: now };
            if (next.conflicts >= CONFLICT_ROUNDS) {
                if (expired) fire("expired", "超过过期时间,未上链");
                else fire("replaced", "链上出现替换交易");
            }
        }
    }

    const changed =
        next.confirmations !== seen.confirmations ||
        next.stage !== seen.stage ||
        next.block !== seen.block ||
        (seen.fee !== null && next.fee !== seen.fee);
    if (!changed)
        return next.conflicts === tx.conflicts && next.reverted === tx.reverted && next.state === tx.state ? tx : next;
    return {
        ...next,
        confirmations: seen.confirmations,
        stage: seen.stage,
        block: seen.block,
        fee: seen.fee ?? next.fee,
        updatedAt: now,
    };
}

/** 这笔交易还占着 nonce,UTXO 等序号.已签名后被取消的不释放,签名仍然有效,见第 3 节. */
export function holdsReservation(tx: Pick<TxRecord, "state" | "raw">): boolean {
    switch (tx.state) {
        case "signing":
        case "signed":
        case "broadcasting":
        case "broadcast":
        case "uncertain":
        case "confirming":
            return true;
        case "abandoned":
            return tx.raw !== undefined;
        default:
            return false;
    }
}

/** 已广播超时,提示加速或取消. */
export function isOverdue(tx: Pick<TxRecord, "state" | "chain" | "broadcastAt">, now: number): boolean {
    const limit = chainOf(tx.chain).overdueAfter;
    return tx.state === "broadcast" && limit !== null && now - (tx.broadcastAt ?? now) > limit;
}

export const STATE_LABELS: Record<TxState, string> = {
    signing: t("待签名"),
    signed: t("已签名"),
    abandoned: t("已放弃"),
    broadcasting: t("广播中"),
    broadcast: t("已广播"),
    uncertain: t("结果不明"),
    replaced: t("已替换"),
    expired: t("已过期"),
    confirming: t("确认中"),
    confirmed: t("已确认"),
    failed: t("链上失败"),
};

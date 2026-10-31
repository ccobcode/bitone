// 交易和占用的存储。状态变化、释放占用都在同一个事务里完成，多标签页不会看到中间状态。
import type { Verified } from "../core";
import { db, type Reservation, type ReservationType, type TxPlan, type TxRecord } from "./db";
import {
    CONFLICT_ROUNDS,
    CONFLICT_SPACING,
    EXPIRY_GRACE,
    holdsReservation,
    observe,
    transition,
    type ChainObservation,
    type ObserveContext,
} from "./machine";
import { announce } from "./sync";
import { t } from "../core/i18n";

/** 超过这个时间还在广播中，多半是页面中途关闭了，按超时处理。比网关请求的时限长。 */
export const BROADCAST_STALE = 60_000;

const scope = () => [db.txs, db.reservations, db.addresses];

/**
 * 按账户加锁，见 docs/03-system.md 第 3 节。多个标签页同时构建时排队，
 * 分配序号、写入占用、写入交易都要在锁里完成。Web Locks 只在安全上下文可用。
 */
export function withAccountLock<T>(accountId: string, work: () => Promise<T>): Promise<T> {
    const locks = globalThis.navigator?.locks;
    if (!locks) return Promise.reject(new Error(t("请用 HTTPS 打开页面后再转账")));
    return locks.request(`bitone:account:${accountId}`, work) as Promise<T>;
}

function dayPrefix(now: number): string {
    const date = new Date(now);
    const pad = (value: number) => String(value).padStart(2, "0");
    return `TX-${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-`;
}

/** 业务编号按天递增，在写入事务里取，多个标签页不会重复。 */
async function nextId(now: number): Promise<string> {
    const prefix = dayPrefix(now);
    const last = await db.txs.where("id").startsWith(prefix).last();
    const count = last ? Number(last.id.slice(prefix.length)) : 0;
    return `${prefix}${String(count + 1).padStart(4, "0")}`;
}

export function reservationId(accountId: string, network: string, type: ReservationType, value: string): string {
    return `${accountId}|${network}|${type}|${value}`;
}

export interface Claim {
    type: ReservationType;
    value: string;
}

export class ReservationConflict extends Error {
    constructor() {
        super(t("序号已被占用"));
    }
}

export type NewTx = Pick<
    TxRecord,
    | "walletId"
    | "accountId"
    | "chain"
    | "network"
    | "kind"
    | "asset"
    | "amount"
    | "from"
    | "to"
    | "request"
    | "requestId"
    | "plan"
    | "expiresAt"
    | "hardExpiry"
    | "replaces"
>;

/** 写入占用和交易。`shared` 是替换交易沿用的占用，其余的必须是新的。 */
export async function insertTx(
    fields: NewTx,
    claims: Claim[],
    shared: Claim[] = [],
    now = Date.now(),
): Promise<TxRecord> {
    const tx = await db.transaction("rw", scope(), async () => {
        const id = await nextId(now);
        const row = (claim: Claim) => reservationId(fields.accountId, fields.network, claim.type, claim.value);
        for (const claim of claims) {
            if (await db.reservations.get(row(claim))) throw new ReservationConflict();
            await db.reservations.add({
                id: row(claim),
                accountId: fields.accountId,
                network: fields.network,
                type: claim.type,
                value: claim.value,
                txIds: [id],
                createdAt: now,
            });
        }
        for (const claim of shared) {
            const existing = await db.reservations.get(row(claim));
            await db.reservations.put({
                id: row(claim),
                accountId: fields.accountId,
                network: fields.network,
                type: claim.type,
                value: claim.value,
                txIds: [...(existing?.txIds ?? []), id],
                createdAt: existing?.createdAt ?? now,
            });
        }
        const record: TxRecord = {
            ...fields,
            id,
            state: "signing",
            createdAt: now,
            updatedAt: now,
            timeline: [{ state: "signing", at: now }],
            attempts: 0,
            conflicts: 0,
            confirmations: 0,
            stage: null,
            block: null,
            fee: null,
        };
        await db.txs.add(record);
        return record;
    });
    announce();
    return tx;
}

/** 有交易上链后，共用同一占用的其他交易都不可能再上链。 */
async function retireSiblings(tx: TxRecord, rows: Reservation[], now: number): Promise<void> {
    const others = new Set(rows.flatMap((row) => row.txIds).filter((id) => id !== tx.id));
    for (const other of await db.txs.bulkGet([...others])) {
        if (!other) continue;
        if (other.state === "signing" || other.state === "signed") {
            await db.txs.put(transition(other, "cancel", now, t("同一序号的另一笔交易已上链")));
        } else if (other.state === "broadcast" || other.state === "uncertain") {
            await db.txs.put({
                ...transition(other, "replaced", now, t("同一序号的另一笔交易已上链")),
                replacedBy: tx.id,
            });
        }
    }
}

async function markChangeUsed(tx: TxRecord): Promise<void> {
    const { plan } = tx;
    if (plan.chain !== "bitcoin" || plan.changeIndex === null) return;
    await db.addresses.update([tx.accountId, 1, plan.changeIndex], { used: true });
}

/** 状态变化后处理占用。上链要等到最终状态才释放，确认中的交易可能被重组退回来。 */
async function settle(before: TxRecord, after: TxRecord, now: number): Promise<void> {
    if (before.state === after.state) return;
    const rows = await db.reservations.where("txIds").equals(after.id).toArray();
    const landed = after.state === "confirming" || after.state === "confirmed" || after.state === "failed";
    if (landed) await retireSiblings(after, rows, now);
    if (after.state === "confirmed" || after.state === "failed") {
        await db.reservations.bulkDelete(rows.map((row) => row.id));
        await markChangeUsed(after);
        return;
    }
    if (landed || holdsReservation(after)) return;
    for (const row of rows) {
        const others = await db.txs.bulkGet(row.txIds.filter((id) => id !== after.id));
        const holders = others.flatMap((tx) => (tx && holdsReservation(tx) ? [tx.id] : []));
        if (holders.length > 0) await db.reservations.put({ ...row, txIds: holders });
        else await db.reservations.delete(row.id);
    }
}

async function apply(id: string, change: (tx: TxRecord) => TxRecord, now: number): Promise<TxRecord> {
    let changed = false;
    const result = await db.transaction("rw", scope(), async () => {
        const before = await db.txs.get(id);
        if (!before) throw new Error(t("交易不存在"));
        const after = change(before);
        if (after === before) return before;
        await db.txs.put(after);
        await settle(before, after, now);
        changed = true;
        return after;
    });
    if (changed) announce();
    return result;
}

export function markSigned(id: string, verified: Verified, now = Date.now()): Promise<TxRecord> {
    return apply(
        id,
        (tx) => ({ ...transition(tx, "signed", now, t("签名与构建记录一致")), raw: verified.raw, hash: verified.hash }),
        now,
    );
}

export function cancelTx(id: string, note: string, now = Date.now()): Promise<TxRecord> {
    return apply(id, (tx) => transition(tx, "cancel", now, note), now);
}

export function beginBroadcast(id: string, now = Date.now()): Promise<TxRecord> {
    return apply(
        id,
        (tx) => ({
            ...transition(tx, tx.state === "uncertain" ? "retry" : "submit", now),
            broadcastAt: now,
            attempts: tx.attempts + 1,
            lastError: undefined,
        }),
        now,
    );
}

export function finishBroadcast(id: string, error: string | null, now = Date.now()): Promise<TxRecord> {
    return apply(
        id,
        (tx) => {
            // 页面超时后交易已经被标成结果不明，这时的结果交给轮询确认。
            if (tx.state !== "broadcasting") return tx;
            if (error === null) return transition(tx, "accepted", now, t("已被网络接收"));
            return { ...transition(tx, "rejected", now, error), lastError: error };
        },
        now,
    );
}

export function applyObservation(id: string, seen: ChainObservation, context: ObserveContext): Promise<TxRecord> {
    return apply(id, (tx) => observe(tx, seen, context), context.now);
}

/** 本地到期的处理，每隔几秒调用一次。 */
export async function sweepExpired(now = Date.now()): Promise<void> {
    // 已放弃的交易过期后签名也作废了，不用再占着。
    for (const tx of await heldByAbandoned()) {
        if (tx.hardExpiry && tx.expiresAt !== null && now > tx.expiresAt + EXPIRY_GRACE) await releaseHolder(tx.id);
    }
    const rows = await db.txs.where("state").anyOf("signing", "signed", "broadcasting").toArray();
    for (const row of rows) {
        if (row.state === "signing" && row.expiresAt !== null && now >= row.expiresAt) {
            const note = row.hardExpiry ? "超过有效期" : "手续费报价已过期";
            await apply(row.id, (tx) => (tx.state === "signing" ? transition(tx, "local-expire", now, note) : tx), now);
        } else if (
            row.state === "signed" &&
            row.hardExpiry &&
            row.expiresAt !== null &&
            now > row.expiresAt + EXPIRY_GRACE
        ) {
            await apply(
                row.id,
                (tx) => (tx.state === "signed" ? transition(tx, "expired", now, t("超过过期时间，未广播")) : tx),
                now,
            );
        } else if (row.state === "broadcasting" && now - (row.broadcastAt ?? 0) > BROADCAST_STALE) {
            await finishBroadcast(row.id, t("广播超时"), now);
        }
    }
}

export async function reservationsOf(accountId: string, network: string): Promise<Reservation[]> {
    const rows = await db.reservations.where("accountId").equals(accountId).toArray();
    return rows.filter((row) => row.network === network);
}

/** 大于等于 `floor` 的第一个没有被占用的序号。 */
export function firstFree(rows: readonly Reservation[], type: ReservationType, floor: number): number {
    const taken = new Set(rows.filter((row) => row.type === type).map((row) => Number(row.value)));
    let value = floor;
    while (taken.has(value)) value += 1;
    return value;
}

/** 大于等于链上 nonce 的第一个空位。中间取消的交易留下的空位要先填上，否则后面的交易永远打包不了。 */
export async function nextNonce(accountId: string, network: string, floor: number): Promise<number> {
    return firstFree(await reservationsOf(accountId, network), "nonce", floor);
}

/** 沿用这笔交易的全部占用，替换交易和重建都走这里，保证最多一笔上链。 */
export async function sharedClaims(tx: Pick<TxRecord, "id">): Promise<Claim[]> {
    const rows = await db.reservations.where("txIds").equals(tx.id).toArray();
    return rows.map(({ type, value }) => ({ type, value }));
}

/** 已放弃但签名仍然有效的交易，等它的序号确定作废后释放。 */
export async function heldByAbandoned(): Promise<TxRecord[]> {
    const rows = await db.txs.where("state").equals("abandoned").toArray();
    const signed = rows.filter((tx) => tx.raw !== undefined);
    const held = await Promise.all(signed.map((tx) => db.reservations.where("txIds").equals(tx.id).count()));
    return signed.filter((_, i) => (held[i] ?? 0) > 0);
}

/** 这笔不再占用。替换交易可能还在途，只去掉自己，别人还占着的不能释放。 */
async function releaseHolder(id: string): Promise<void> {
    await db.transaction("rw", scope(), async () => {
        for (const row of await db.reservations.where("txIds").equals(id).toArray()) {
            const others = await db.txs.bulkGet(row.txIds.filter((txId) => txId !== id));
            const holders = others.flatMap((tx) => (tx && holdsReservation(tx) ? [tx.id] : []));
            if (holders.length > 0) await db.reservations.put({ ...row, txIds: holders });
            else await db.reservations.delete(row.id);
        }
    });
    announce();
}

/** 已放弃的交易这一轮的序号是否已被用掉。和被替换的判定一样，要连续看到才释放。 */
export async function noteSpent(id: string, spent: boolean, now = Date.now()): Promise<void> {
    const tx = await db.txs.get(id);
    if (!tx) return;
    const spaced = now - (tx.conflictAt ?? 0) >= CONFLICT_SPACING;
    if (!spent) {
        if (tx.conflicts !== 0) await db.txs.update(id, { conflicts: 0 });
        return;
    }
    if (!spaced) return;
    const conflicts = tx.conflicts + 1;
    await db.txs.update(id, { conflicts, conflictAt: now });
    if (conflicts >= CONFLICT_ROUNDS) await releaseHolder(id);
}

/**
 * 找到挡路的已放弃交易：它的 nonce 不小于链上 nonce，而且没有别的交易在用。
 * 在它之后构建的交易永远打包不了，要先发替换交易。
 */
export function abandonedHolder(
    rows: readonly Reservation[],
    txs: readonly TxRecord[],
    floor: number,
): TxRecord | null {
    for (const row of rows) {
        if (row.type !== "nonce" || Number(row.value) < floor) continue;
        const owners = txs.filter((tx) => row.txIds.includes(tx.id));
        if (owners.length > 0 && owners.every((tx) => tx.state === "abandoned")) return owners[0] ?? null;
    }
    return null;
}

export function planOf<C extends TxPlan["chain"]>(tx: TxRecord, chain: C): Extract<TxPlan, { chain: C }> {
    if (tx.plan.chain !== chain) throw new Error(t("交易类型不符"));
    return tx.plan as Extract<TxPlan, { chain: C }>;
}

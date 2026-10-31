// 占用的查询.构建交易时按这里的结果分配序号,写入和释放在 txs.ts 的事务里.
import { db, type Reservation, type ReservationType, type TxRecord } from "./db";

export interface Claim {
    type: ReservationType;
    value: string;
}

export async function reservationsOf(accountId: string, network: string): Promise<Reservation[]> {
    const rows = await db.reservations.where("accountId").equals(accountId).toArray();
    return rows.filter((row) => row.network === network);
}

/** 大于等于 `floor` 的第一个没有被占用的序号. */
export function firstFree(rows: readonly Reservation[], type: ReservationType, floor: number): number {
    const taken = new Set(rows.filter((row) => row.type === type).map((row) => Number(row.value)));
    let value = floor;
    while (taken.has(value)) value += 1;
    return value;
}

/** 大于等于链上 nonce 的第一个空位.中间取消的交易留下的空位要先填上,否则后面的交易永远打包不了. */
export async function nextNonce(accountId: string, network: string, floor: number): Promise<number> {
    return firstFree(await reservationsOf(accountId, network), "nonce", floor);
}

/** 沿用这笔交易的全部占用,替换交易和重建都走这里,保证最多一笔上链. */
export async function sharedClaims(tx: Pick<TxRecord, "id">): Promise<Claim[]> {
    const rows = await db.reservations.where("txIds").equals(tx.id).toArray();
    return rows.map(({ type, value }) => ({ type, value }));
}

/**
 * 找到挡路的已放弃交易:它的 nonce 不小于链上 nonce,而且没有别的交易在用.
 * 在它之后构建的交易永远打包不了,要先发替换交易.
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

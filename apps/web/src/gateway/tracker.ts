// 跟踪未完成的交易。WebSocket 推送和 HTTP 轮询都走这里，状态变化只由 machine.ts 决定。
import { db, type TxRecord } from "../data/db";
import { isWatched, type ChainObservation } from "../data/machine";
import { applyObservation, heldByAbandoned, noteSpent, planOf } from "../data/txs";
import { addressKey, fetchNonceAccount, fetchNonces, fetchTxs, fetchUtxos, parseObservation } from "./client";

export const WATCHED_STATES = ["broadcast", "uncertain", "confirming", "replaced"] as const;

export async function watchedTxs(now: number): Promise<TxRecord[]> {
    const rows = await db.txs.where("state").anyOf(WATCHED_STATES).toArray();
    return rows.filter((tx) => tx.hash && isWatched(tx, now));
}

function groupByNetwork(txs: readonly TxRecord[]): Map<string, TxRecord[]> {
    const groups = new Map<string, TxRecord[]>();
    for (const tx of txs) groups.set(tx.network, [...(groups.get(tx.network) ?? []), tx]);
    return groups;
}

/** 这一组交易的序号有没有被用掉。查询失败时没有结果，这一轮不计入。 */
async function spentFlags(network: string, txs: readonly TxRecord[]): Promise<Map<string, boolean>> {
    const flags = new Map<string, boolean>();
    const [first] = txs;
    if (!first) return flags;
    try {
        if (first.chain === "evm") {
            const nonces = await fetchNonces(network, [...new Set(txs.map((tx) => tx.from))]);
            for (const tx of txs) {
                const chain = nonces.get(tx.from);
                if (chain) flags.set(tx.id, chain.latest > planOf(tx, "evm").nonce);
            }
        } else if (first.chain === "bitcoin") {
            const inputs = txs.flatMap((tx) => planOf(tx, "bitcoin").inputs);
            const utxos = await fetchUtxos(network, [...new Set(inputs.map((input) => input.address))]);
            const unspent = new Set(utxos.map((utxo) => `${utxo.txid}:${utxo.vout}`));
            for (const tx of txs) {
                flags.set(
                    tx.id,
                    planOf(tx, "bitcoin").inputs.some((input) => !unspent.has(`${input.txid}:${input.vout}`)),
                );
            }
        } else if (first.chain === "solana") {
            for (const tx of txs) {
                const plan = planOf(tx, "solana");
                if (plan.nonceValue === null) continue;
                const account = await fetchNonceAccount(network, plan.nonceAccount);
                if (account.exists) flags.set(tx.id, account.nonce !== plan.nonceValue);
            }
        }
    } catch {
        flags.clear();
    }
    return flags;
}

/** 已放弃但签名仍然有效的交易，序号确定被用掉后释放占用。 */
async function reconcileAbandoned(supported: readonly string[], now: number): Promise<void> {
    const held = (await heldByAbandoned()).filter((tx) => supported.includes(tx.network));
    await Promise.all(
        [...groupByNetwork(held)].map(async ([network, txs]) => {
            const flags = await spentFlags(network, txs);
            for (const tx of txs) {
                const spent = flags.get(tx.id);
                if (spent !== undefined) await noteSpent(tx.id, spent, now);
            }
        }),
    );
}

/**
 * 查询全部未完成的交易。先取替换证据再取状态，顺序不能反：
 * 否则交易在两次查询之间上链时，看到的是「状态未知，序号已用」，会被误判成已替换。
 * 全部查询都失败时返回 false，交易状态保持不变。
 */
export async function pollOpenTxs(supported: readonly string[], now = Date.now()): Promise<boolean> {
    const tracked = (await watchedTxs(now)).filter((tx) => supported.includes(tx.network));
    const results = await Promise.allSettled(
        [...groupByNetwork(tracked)].map(async ([network, txs]) => {
            const conflicts = await spentFlags(
                network,
                txs.filter((tx) => tx.state !== "replaced"),
            );
            const seen = await fetchTxs(
                network,
                txs.flatMap((tx) => (tx.hash ? [tx.hash] : [])),
            );
            const byHash = new Map(seen.map((item) => [addressKey(item.hash), item]));
            for (const tx of txs) {
                const item = tx.hash ? byHash.get(addressKey(tx.hash)) : undefined;
                if (item) await applyObservation(tx.id, item, { now, conflict: conflicts.get(tx.id) });
            }
        }),
    );
    await reconcileAbandoned(supported, now).catch(() => undefined);
    return results.every((result) => result.status === "fulfilled");
}

async function applyToHash(network: string, seen: ChainObservation, now: number): Promise<void> {
    const rows = await db.txs.where("hash").equals(seen.hash).toArray();
    for (const row of rows) {
        if (row.network === network) await applyObservation(row.id, seen, { now });
    }
}

/** `tx` 频道的推送，内容还没校验。 */
export async function applyPush(network: string, raw: unknown, now = Date.now()): Promise<void> {
    const seen = parseObservation(raw);
    if (seen) await applyToHash(network, seen, now);
}

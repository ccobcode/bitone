// 把账户、地址和网关余额合成资产表。
import { isLegacyKind, networksFor, type Mode } from "../core";
import { lookupAsset, type AssetEntry } from "../core/manifest";
import type { Account, Address, TxRecord } from "../data/db";
import { addressKey, type Balance } from "./client";

export interface Target {
    network: string;
    addresses: string[];
}

function groupByAccount(addresses: readonly Address[]): Map<string, Address[]> {
    const groups = new Map<string, Address[]>();
    for (const address of addresses) {
        const group = groups.get(address.accountId);
        if (group) group.push(address);
        else groups.set(address.accountId, [address]);
    }
    return groups;
}

/** 地址和所属账户。EVM 账户在多个网络上地址相同。 */
export function accountNetworks(account: Account, mode: Mode, supported: readonly string[]): string[] {
    return networksFor(account.chain, mode)
        .map((n) => n.id)
        .filter((id) => supported.includes(id));
}

/** 每个网络要查询的地址。只查网关配置了上游的网络。 */
export function buildTargets(
    accounts: readonly Account[],
    addresses: readonly Address[],
    mode: Mode,
    supported: readonly string[],
): Target[] {
    const byNetwork = new Map<string, Set<string>>();
    const byAccount = groupByAccount(addresses);
    for (const account of accounts) {
        for (const network of accountNetworks(account, mode, supported)) {
            const set = byNetwork.get(network) ?? new Set<string>();
            for (const row of byAccount.get(account.id) ?? []) set.add(row.address);
            byNetwork.set(network, set);
        }
    }
    return [...byNetwork].map(([network, set]) => ({ network, addresses: [...set] }));
}

export interface AssetRow {
    id: string;
    account: Account;
    network: string;
    asset: string;
    /** 不在内置清单时是 null。 */
    entry: AssetEntry | null;
    amount: bigint;
    /** 这个网络还没查到结果时金额不可信，界面不能显示成 0。 */
    loaded: boolean;
    address: string;
}

export function buildRows(
    accounts: readonly Account[],
    addresses: readonly Address[],
    mode: Mode,
    supported: readonly string[],
    balances: ReadonlyMap<string, readonly Balance[]>,
): AssetRow[] {
    const byAccount = groupByAccount(addresses);
    const rows: AssetRow[] = [];
    for (const account of accounts) {
        const owned = byAccount.get(account.id) ?? [];
        const first = owned.find((a) => a.change === 0 && a.index === 0)?.address ?? "";
        const set = new Set(owned.map((a) => a.address));
        for (const network of accountNetworks(account, mode, supported)) {
            const sums = new Map<string, { asset: string; amount: bigint }>();
            if (!isLegacyKind(account.kind)) sums.set("native", { asset: "native", amount: 0n });
            for (const balance of balances.get(network) ?? []) {
                if (!set.has(balance.address)) continue;
                const key = addressKey(balance.asset);
                const sum = sums.get(key) ?? { asset: balance.asset, amount: 0n };
                sums.set(key, { asset: sum.asset, amount: sum.amount + balance.amount });
            }
            for (const [key, { asset, amount }] of sums) {
                if (isLegacyKind(account.kind) && amount === 0n) continue;
                rows.push({
                    id: `${account.id}|${network}|${key}`,
                    account,
                    network,
                    asset,
                    entry: lookupAsset(network, asset),
                    amount,
                    loaded: balances.has(network),
                    address: first,
                });
            }
        }
    }
    // 原生币在前，清单内的代币其次，清单外的放最后。
    const weight = (row: AssetRow) => (row.entry?.native ? 0 : row.entry ? 1 : 2);
    return rows
        .map((row, order) => ({ row, order }))
        .sort((a, b) => weight(a.row) - weight(b.row) || a.order - b.order)
        .map(({ row }) => row);
}

const OUTGOING: ReadonlySet<TxRecord["state"]> = new Set<TxRecord["state"]>([
    "signing",
    "signed",
    "broadcasting",
    "broadcast",
    "uncertain",
]);

/** 待转出的金额。余额只算已上链的部分，这些还没有扣。替换交易和原交易只算一笔。 */
export function pendingAmount(txs: readonly TxRecord[], accountId: string, network: string, asset: string): bigint {
    return txs
        .filter(
            (tx) =>
                tx.kind === "transfer" &&
                OUTGOING.has(tx.state) &&
                tx.accountId === accountId &&
                tx.network === network &&
                addressKey(tx.asset) === addressKey(asset),
        )
        .reduce((sum, tx) => sum + BigInt(tx.amount), 0n);
}

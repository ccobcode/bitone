// 把账户、地址和网关余额合成资产表。
import { isLegacyKind, networksFor, type Mode } from "../core";
import { usdCents } from "../core/amount";
import { lookupAsset, priceKey, type AssetEntry, type IconName } from "../core/manifest";
import type { Account, Address, TxRecord } from "../data/db";
import { addressKey, type Balance, type PriceMap } from "./client";

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

/** 这一行的美元估值，单位是分。价格只来自网关，只用于展示。 */
export function rowCents(row: AssetRow, prices: PriceMap | null): bigint | null {
    if (!row.entry || !row.loaded) return null;
    const price = prices?.get(priceKey(row.network, row.asset));
    return price ? usdCents(row.amount, row.entry.decimals, price) : null;
}

export type ManifestRow = AssetRow & { entry: AssetEntry };

function inManifest(row: AssetRow): row is ManifestRow {
    return row.entry !== null;
}

/** 同一个币在各个网络和账户上的持仓。清单外的代币不进这里，它们的符号可以伪造。 */
export interface AssetGroup {
    /** 清单条目的 icon。条目按网络加合约查出，链上数据决定不了它。 */
    key: IconName;
    entry: AssetEntry;
    rows: ManifestRow[];
    /** 合计，单位见 decimals。同一个币在不同网络的小数位可能不同，比如 BSC 上的 USDT 是 18 位。 */
    amount: bigint;
    decimals: number;
    cents: bigint | null;
    /** 至少有一个网络查到了结果。 */
    loaded: boolean;
    /** 全部网络都查到了，而且合计是零。 */
    empty: boolean;
}

function scale(amount: bigint, from: number, to: number): bigint {
    return amount * 10n ** BigInt(to - from);
}

function byAmount(a: bigint, b: bigint): number {
    return a === b ? 0 : a > b ? -1 : 1;
}

/** 一行持仓折算到这一组的单位，不同小数位的行才能比较和相加。 */
export function groupAmount(group: Pick<AssetGroup, "decimals">, row: ManifestRow, amount = row.amount): bigint {
    return scale(amount, row.entry.decimals, group.decimals);
}

/** 按资产合并，有余额的按估值从高到低排在前面，还没查到的其次，零余额最后。 */
export function groupRows(rows: readonly AssetRow[], prices: PriceMap | null): AssetGroup[] {
    const byKey = new Map<IconName, ManifestRow[]>();
    for (const row of rows.filter(inManifest)) {
        const list = byKey.get(row.entry.icon);
        if (list) list.push(row);
        else byKey.set(row.entry.icon, [row]);
    }
    const groups = [...byKey].map(([key, list]): AssetGroup => {
        const decimals = Math.max(...list.map((row) => row.entry.decimals));
        const priced = list.map((row) => rowCents(row, prices)).filter((cents) => cents !== null);
        const amount = list.reduce((sum, row) => sum + scale(row.amount, row.entry.decimals, decimals), 0n);
        return {
            key,
            entry: list[0]?.entry as AssetEntry,
            rows: list,
            amount,
            decimals,
            cents: priced.length > 0 ? priced.reduce((sum, cents) => sum + cents, 0n) : null,
            loaded: list.some((row) => row.loaded),
            empty: amount === 0n && list.every((row) => row.loaded),
        };
    });
    const weight = (group: AssetGroup) => (group.amount > 0n ? 0 : group.empty ? 2 : 1);
    return groups
        .map((group, order) => ({ group, order }))
        .sort(
            (a, b) =>
                weight(a.group) - weight(b.group) ||
                byAmount(a.group.cents ?? -1n, b.group.cents ?? -1n) ||
                a.order - b.order,
        )
        .map(({ group }) => group);
}

/** 带着这一行进转账页。 */
export function sendLink({ account, network, asset }: Pick<AssetRow, "account" | "network" | "asset">): string {
    return `/send?account=${encodeURIComponent(account.id)}&network=${encodeURIComponent(network)}&asset=${encodeURIComponent(asset)}`;
}

/** 一组里余额最多的一行，转账和收款默认用它。 */
export function largestRow(group: AssetGroup): ManifestRow {
    const sorted = [...group.rows].sort((a, b) => byAmount(groupAmount(group, a), groupAmount(group, b)));
    return sorted[0] as ManifestRow;
}

/** 总览里单独显示的资产数，其余合成「其他」。 */
const SHOWN_SHARES = 4;

export interface Share {
    key: IconName | "other";
    label: string | null;
    cents: bigint;
    percent: number;
}

/** 总资产按资产的占比。没有估值的不参与，占比之和可能因为四舍五入差一点。 */
export function assetShares(groups: readonly AssetGroup[]): Share[] {
    const priced = groups
        .flatMap((group) => (group.cents !== null && group.cents > 0n ? [{ group, cents: group.cents }] : []))
        .sort((a, b) => byAmount(a.cents, b.cents));
    const total = priced.reduce((sum, item) => sum + item.cents, 0n);
    if (total === 0n) return [];
    const percent = (cents: bigint) => Number((cents * 1000n) / total) / 10;
    const shown = priced.length > SHOWN_SHARES + 1 ? priced.slice(0, SHOWN_SHARES) : priced;
    const shares = shown.map(({ group, cents }): Share => ({
        key: group.key,
        label: group.entry.symbol,
        cents,
        percent: percent(cents),
    }));
    const rest = priced.slice(shown.length).reduce((sum, item) => sum + item.cents, 0n);
    return rest > 0n ? [...shares, { key: "other", label: null, cents: rest, percent: percent(rest) }] : shares;
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

/** 这一组待转出的合计，单位和 group.amount 相同。 */
export function groupPending(group: AssetGroup, txs: readonly TxRecord[]): bigint {
    return group.rows.reduce(
        (sum, row) => sum + groupAmount(group, row, pendingAmount(txs, row.account.id, row.network, row.asset)),
        0n,
    );
}

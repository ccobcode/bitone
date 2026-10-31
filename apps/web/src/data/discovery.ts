// Bitcoin 地址发现，只在浏览器里进行，见 docs/03-system.md 第 4.2 节。
//
// docs/09 的限制：`/history` 的条目按一批地址归纳，同一笔交易里第二个自有输出不会出现在
// from 和 to 里。所以这里不看这两个字段，只看「这批地址有没有任何记录」，
// 一批有记录时再二分到单个地址，单个地址的查询不存在归纳的问题。
import { fetchHistory } from "../gateway/client";
import { db, type Account, type Address } from "./db";
import { GAP_LIMIT, MAX_ADDRESS_INDEX, ensureAddress } from "./pairing";
import { t } from "../core/i18n";

/** 空页也可能带游标。连续这么多空页还没结束，就当查询失败，不下结论。 */
const MAX_EMPTY_PAGES = 100;

async function hasHistory(network: string, addresses: readonly string[]): Promise<boolean> {
    let cursor: string | null = null;
    for (let page = 0; page < MAX_EMPTY_PAGES; page++) {
        const result = await fetchHistory(network, addresses, cursor);
        if (result.items.length > 0) return true;
        if (result.cursor === null) return false;
        cursor = result.cursor;
    }
    throw new Error(t("历史查询没有结束"));
}

async function usedAmong(network: string, rows: readonly Address[]): Promise<Address[]> {
    if (
        rows.length === 0 ||
        !(await hasHistory(
            network,
            rows.map((row) => row.address),
        ))
    )
        return [];
    if (rows.length === 1) return [...rows];
    const middle = rows.length >> 1;
    const [left, right] = await Promise.all([
        usedAmong(network, rows.slice(0, middle)),
        usedAmong(network, rows.slice(middle)),
    ]);
    return [...left, ...right];
}

/** 按 gap limit 继续派生：最后一个有记录的地址之后，要有连续 20 个没有记录的地址。 */
async function discoverChain(
    account: Account,
    network: string,
    mode: "mainnet" | "testnet",
    change: 0 | 1,
): Promise<void> {
    let last = -1;
    let next = 0;
    while (next <= last + GAP_LIMIT && next < MAX_ADDRESS_INDEX) {
        const end = Math.min(next + GAP_LIMIT, last + GAP_LIMIT + 1, MAX_ADDRESS_INDEX);
        const rows: Address[] = [];
        for (let index = next; index < end; index++) rows.push(await ensureAddress(account, mode, index, change));
        const fresh = await usedAmong(
            network,
            rows.filter((row) => !row.used),
        );
        for (const row of fresh) await db.addresses.update([row.accountId, row.change, row.index], { used: true });
        for (const row of [...rows.filter((item) => item.used), ...fresh]) last = Math.max(last, row.index);
        next = end;
    }
}

/**
 * 一次发现要逐个查几十个地址，公共节点按 IP 限流。所以先用一个地址探测，上次之后高度没变就不用再发现。
 * 探测的水位在发现之前取，发现途中上链的交易下次还会查到。
 */
export async function discoverAccount(account: Account, network: string, mode: "mainnet" | "testnet"): Promise<void> {
    const key = `discovery:${account.id}:${network}`;
    const saved = (await db.settings.get(key))?.value;
    const first = await ensureAddress(account, mode, 0, 0);
    const probe = await fetchHistory(network, [first.address], null, typeof saved === "string" ? saved : null);
    if (probe.unchanged) return;
    await discoverChain(account, network, mode, 0);
    await discoverChain(account, network, mode, 1);
    await db.settings.put({ key, value: probe.checkpoint });
}

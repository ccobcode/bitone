// Bitcoin 地址发现，只在浏览器里进行，见 docs/03-system.md 第 4.2 节。
// 地址都由这里生成，只有别的钱包用同一个种子时才会多出地址，所以配对时跑一次，之后一天最多一次。
import { fetchUsed } from "../gateway/client";
import { db, type Account, type Address } from "./db";
import { GAP_LIMIT, MAX_ADDRESS_INDEX, draftAddress } from "./pairing";

const DISCOVERY_INTERVAL = 24 * 60 * 60 * 1000;

/** 按 gap limit 继续派生：最后一个有记录的地址之后，要有连续 20 个没有记录的地址。 */
async function discoverChain(
    account: Account,
    network: string,
    mode: "mainnet" | "testnet",
    change: 0 | 1,
): Promise<Address[]> {
    const found: Address[] = [];
    let last = -1;
    let next = 0;
    while (next <= last + GAP_LIMIT && next < MAX_ADDRESS_INDEX) {
        const end = Math.min(next + GAP_LIMIT, last + GAP_LIMIT + 1, MAX_ADDRESS_INDEX);
        const rows: Address[] = [];
        for (let index = next; index < end; index++) rows.push(await draftAddress(account, mode, index, change));
        const unknown = rows.filter((row) => !row.used);
        const used = new Set(
            unknown.length > 0
                ? await fetchUsed(
                      network,
                      unknown.map((row) => row.address),
                  )
                : [],
        );
        for (const row of unknown) if (used.has(row.address)) row.used = true;
        for (const row of rows) if (row.used) last = Math.max(last, row.index);
        found.push(...rows);
        next = end;
    }
    return found;
}

/** 地址一变，页面就要换订阅、重查余额，所以发现结束时才写入，而且只写一次。发现途中其他地方改过的字段以库里为准。 */
async function saveFound(rows: readonly Address[]): Promise<void> {
    await db.transaction("rw", db.addresses, async () => {
        const stored = await db.addresses.bulkGet(rows.map((row) => [row.accountId, row.change, row.index]));
        await db.addresses.bulkPut(rows.map((row, i) => ({ ...row, ...stored[i], used: row.used ?? stored[i]?.used })));
    });
}

export async function discoverAccount(account: Account, network: string, mode: "mainnet" | "testnet"): Promise<void> {
    const key = `discovery:${account.id}:${network}`;
    const last = (await db.settings.get(key))?.value;
    if (typeof last === "number" && Date.now() - last < DISCOVERY_INTERVAL) return;
    await saveFound([
        ...(await discoverChain(account, network, mode, 0)),
        ...(await discoverChain(account, network, mode, 1)),
    ]);
    await db.settings.put({ key, value: Date.now() });
}

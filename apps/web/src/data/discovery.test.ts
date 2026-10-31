import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { PATHS, gatewayError, initCore, pairWallet, stubGateway, type Fixture } from "../testing";
import { clearLocalData, db, type Account } from "./db";
import { discoverAccount } from "./discovery";
import { GAP_LIMIT, ensureAddress } from "./pairing";

let fixture: Fixture;
let account: Account;

beforeAll(initCore);

beforeEach(async () => {
    await clearLocalData();
    fixture = await pairWallet();
    account = fixture.account(PATHS.segwit);
});

afterEach(() => vi.unstubAllGlobals());

const item = (hash: string) => ({
    hash,
    time: 1,
    from: null,
    to: null,
    asset: "native",
    amount: "1",
    status: "finalized",
});

async function address(change: 0 | 1, index: number): Promise<string> {
    const row = await db.addresses.get([account.id, change, index]);
    return row?.address ?? "";
}

/** `used` 里的地址有记录。条目的 from 和 to 都是空的，模拟 docs/09 说的归纳。 */
function historyFor(used: () => Set<string>) {
    return stubGateway({
        "bitcoin/history": ({ addresses }) => ({
            items: (addresses as string[]).filter((a) => used().has(a)).map((a) => item(a)),
            cursor: null,
        }),
    });
}

async function rows(change: 0 | 1) {
    return (await db.addresses.where("accountId").equals(account.id).toArray()).filter((row) => row.change === change);
}

describe("地址发现", () => {
    it("发现结束时一次写入地址，途中不写，否则每个地址都会引发一轮页面查询", async () => {
        const used = new Set<string>();
        for (const index of [0, 19]) used.add(await address(0, index));
        historyFor(() => used);
        const bulkPut = vi.spyOn(db.addresses, "bulkPut");
        const put = vi.spyOn(db.addresses, "put");
        const update = vi.spyOn(db.addresses, "update");

        await discoverAccount(account, "bitcoin", "mainnet");

        expect(bulkPut).toHaveBeenCalledTimes(1);
        expect(put).not.toHaveBeenCalled();
        expect(update).not.toHaveBeenCalled();
        expect((await rows(0)).filter((row) => row.used).map((row) => row.index)).toEqual([0, 19]);
        expect(await rows(0)).toHaveLength(40);
    });

    it("发现途中核对过的地址不会被写回未核对", async () => {
        historyFor(() => new Set());
        const fetchSpy = globalThis.fetch;
        let touched = false;
        vi.stubGlobal("fetch", async (...args: Parameters<typeof fetch>) => {
            if (!touched) {
                touched = true;
                await db.addresses.update([account.id, 0, 3], { verified: true });
            }
            return fetchSpy(...args);
        });
        await discoverAccount(account, "bitcoin", "mainnet");
        expect((await rows(0)).find((row) => row.index === 3)?.verified).toBe(true);
    });

    it("高度没变时只探测一个地址，不再逐个查询", async () => {
        const { calls } = stubGateway({
            "bitcoin/history": ({ since }) => ({
                items: [],
                cursor: null,
                checkpoint: "95",
                unchanged: since === "95",
            }),
        });
        await discoverAccount(account, "bitcoin", "mainnet");
        const full = calls.length;
        expect(full).toBeGreaterThan(1);

        await discoverAccount(account, "bitcoin", "mainnet");
        expect(calls.length).toBe(full + 1);
    });

    it("没有记录时不继续派生", async () => {
        const { calls } = historyFor(() => new Set());
        await discoverAccount(account, "bitcoin", "mainnet");
        expect(await rows(0)).toHaveLength(GAP_LIMIT);
        // 先探测一次，接收和找零再各一批。
        expect(calls).toHaveLength(3);
    });

    it("最后一个有记录的地址之后要有 20 个空地址", async () => {
        const used = new Set([await address(0, 15)]);
        historyFor(() => used);
        await discoverAccount(account, "bitcoin", "mainnet");
        expect(await rows(0)).toHaveLength(15 + GAP_LIMIT + 1);
        expect((await db.addresses.get([account.id, 0, 15]))?.used).toBe(true);
        expect((await db.addresses.get([account.id, 0, 14]))?.used).toBeFalsy();
    });

    it("发现的地址又落在新一批里时继续往后", async () => {
        const first = await address(0, 19);
        const used = new Set([first]);
        historyFor(() => used);
        used.add((await ensureAddress(account, "mainnet", 30)).address);
        await discoverAccount(account, "bitcoin", "mainnet");
        expect(await rows(0)).toHaveLength(30 + GAP_LIMIT + 1);
    });

    it("找零链单独发现", async () => {
        const used = new Set([await address(1, 3)]);
        historyFor(() => used);
        await discoverAccount(account, "bitcoin", "mainnet");
        expect((await db.addresses.get([account.id, 1, 3]))?.used).toBe(true);
        expect(await rows(1)).toHaveLength(3 + GAP_LIMIT + 1);
    });

    it("只有第二个自有输出的地址也能发现，不靠 from 和 to", async () => {
        const hidden = await address(0, 7);
        historyFor(() => new Set([hidden]));
        await discoverAccount(account, "bitcoin", "mainnet");
        expect((await db.addresses.get([account.id, 0, 7]))?.used).toBe(true);
    });

    it("空页带游标时继续查，不能当成没有记录", async () => {
        const target = await address(0, 2);
        stubGateway({
            "bitcoin/history": ({ addresses, cursor }) => {
                if (!(addresses as string[]).includes(target)) return { items: [], cursor: null };
                return cursor === "next" ? { items: [item("a")], cursor: null } : { items: [], cursor: "next" };
            },
        });
        await discoverAccount(account, "bitcoin", "mainnet");
        expect((await db.addresses.get([account.id, 0, 2]))?.used).toBe(true);
    });

    it("一直是空页时算查询失败，不下结论", async () => {
        stubGateway({ "bitcoin/history": () => ({ items: [], cursor: "again" }) });
        await expect(discoverAccount(account, "bitcoin", "mainnet")).rejects.toThrow();
    });

    it("网关不可用时报错，不当成没有记录", async () => {
        stubGateway({ "bitcoin/history": () => gatewayError("upstream_unavailable", "", 503) });
        await expect(discoverAccount(account, "bitcoin", "mainnet")).rejects.toMatchObject({
            code: "upstream_unavailable",
        });
    });

    it("再次发现时不重新查询已确认用过的地址", async () => {
        const used = new Set([await address(0, 1)]);
        historyFor(() => used);
        await discoverAccount(account, "bitcoin", "mainnet");
        const { calls } = historyFor(() => new Set());
        await discoverAccount(account, "bitcoin", "mainnet");
        expect(calls.length).toBeLessThanOrEqual(4);
        expect(await rows(0)).toHaveLength(1 + GAP_LIMIT + 1);
    });
});

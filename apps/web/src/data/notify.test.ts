import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { initCore } from "../testing";
import { fetchHistory, fetchSnapshot, type HistoryItem } from "../gateway/client";
import { db, type ChainEntry } from "./db";
import { applyChainUpdate, syncFeed } from "./feed";
import { claimNotice, incomingNotices, mergedNotice, missedNotice, onNotice, publish, type Notice } from "./notify";

vi.mock("../gateway/client", async (actual) => ({
    ...(await actual<typeof import("../gateway/client")>()),
    fetchHistory: vi.fn(),
    fetchSnapshot: vi.fn(),
}));

const ME = "0x000000000000000000000000000000000000aaaa";
const PEER = "0x000000000000000000000000000000000000bbbb";
const USDT = "0xdAC17F958D2ee523a2206206994597C13D831ec7";
const own = new Set([ME]);
const STREAM = { network: "ethereum", addresses: [ME] };
const ONE_ETH = 10n ** 18n;

function row(extra: Partial<ChainEntry> = {}): ChainEntry {
    const hash = extra.hash ?? "0x1";
    return {
        id: `ethereum:${hash}`,
        walletId: "w",
        stream: "s",
        txKey: `ethereum:${hash}`,
        network: "ethereum",
        hash,
        time: 1,
        from: PEER,
        to: ME,
        asset: "native",
        amount: ONE_ETH.toString(),
        status: "included",
        ...extra,
    };
}

function item(hash: string, status: HistoryItem["status"] = "included"): HistoryItem {
    return { hash, time: 1, from: PEER, to: ME, asset: "native", amount: ONE_ETH, status };
}

const update = (items: HistoryItem[]) => ({ height: 2, previous: 1, balances: [], items });
const page = (items: HistoryItem[]) => ({ items, cursor: null, checkpoint: "100", unchanged: false });

function snapshotOf(items: HistoryItem[]) {
    vi.mocked(fetchSnapshot).mockImplementation(async (_, onResult) => {
        await onResult({
            network: "ethereum",
            addresses: [ME],
            height: 10,
            balances: [],
            history: page(items),
            error: null,
        });
    });
}

beforeAll(initCore);
beforeEach(async () => {
    vi.mocked(fetchHistory).mockReset();
    await Promise.all(db.tables.map((table) => table.clear()));
});

describe("要不要通知", () => {
    const told = (rows: ChainEntry[], prices: Map<string, string> | null = null) =>
        incomingNotices(rows, own, prices).length;

    it("别人转来的清单内资产要通知，确认前写明确认中", () => {
        const [notice] = incomingNotices([row()], own, null);
        expect(notice?.kind).toBe("incoming");
        expect(notice?.title).toContain("确认中");
        expect(notice?.summary).not.toContain("ETH");
    });

    it("最终确认后不再写确认中", () => {
        expect(incomingNotices([row({ status: "finalized" })], own, null)[0]?.title).not.toContain("确认中");
    });

    it("自己转出和转给自己的不通知", () => {
        expect(told([row({ from: ME, to: PEER })])).toBe(0);
        expect(told([row({ from: ME, to: ME })])).toBe(0);
    });

    it("失败的不通知", () => {
        expect(told([row({ status: "failed" })])).toBe(0);
    });

    it("清单外的代币和零金额转账不通知，它们常被用来做地址投毒", () => {
        expect(told([row({ asset: "0x000000000000000000000000000000000000dead" })])).toBe(0);
        expect(told([row({ asset: USDT, amount: "0" })])).toBe(0);
    });

    it("粉尘不通知，查不到价格时不能漏掉", () => {
        const prices = new Map([["ethereum:native", "3000"]]);
        expect(told([row({ amount: "1000" })], prices)).toBe(0);
        expect(told([row()], prices)).toBe(1);
        expect(told([row({ amount: "1000" })], new Map())).toBe(1);
    });

    it("一笔交易里有多条转账时只通知一次", () => {
        expect(told([row({ id: "a" }), row({ id: "b" })])).toBe(1);
    });

    it("EVM 地址的大小写不影响判断", () => {
        expect(told([row({ to: ME.replace("aaaa", "AAAA") })])).toBe(1);
    });
});

describe("合并", () => {
    it("连续的转入合成一条", () => {
        const [first] = incomingNotices([row()], own, null);
        const merged = mergedNotice(first as Notice, 3);
        expect(merged).toMatchObject({ count: 3, title: "收到 3 笔转账", summary: "收到 3 笔转账" });
    });

    it("用户不在时的记录合成一条，只有一笔时照常显示", () => {
        expect(missedNotice([], own, null)).toBeNull();
        expect(missedNotice([row()], own, null)?.title).toContain("确认中");
        const rows = [row({ hash: "0x1" }), row({ hash: "0x2" })];
        expect(missedNotice(rows, own, null)).toMatchObject({ count: 2, title: "离线期间收到 2 笔转账" });
    });
});

describe("什么算新发生", () => {
    it("推送带来的记录只有第一次算新，多个标签页收到同一条也只通知一次", async () => {
        expect(await applyChainUpdate("w", STREAM, update([item("0x1")]))).toHaveLength(1);
        expect(await applyChainUpdate("w", STREAM, update([item("0x1")]))).toHaveLength(0);
    });

    it("状态变化不是新交易，不再通知", async () => {
        await applyChainUpdate("w", STREAM, update([item("0x1", "pending")]));
        expect(await applyChainUpdate("w", STREAM, update([item("0x1", "finalized")]))).toHaveLength(0);
    });

    it("第一次同步下载的历史不算新发生", async () => {
        snapshotOf([item("0x1"), item("0x2")]);
        const { missed } = await syncFeed("w", [STREAM], () => ["native"]);
        expect(missed).toEqual([]);
    });

    it("增量补回来的才算错过的，已经看过的未定论记录不算", async () => {
        snapshotOf([item("0x1")]);
        await syncFeed("w", [STREAM], () => ["native"]);
        snapshotOf([item("0x1", "finalized"), item("0x2")]);
        const { missed } = await syncFeed("w", [STREAM], () => ["native"]);
        expect(missed.map((entry) => entry.hash)).toEqual(["0x2"]);
    });
});

describe("分发", () => {
    it("同一件事只有一个标签页能认领", async () => {
        const results = await Promise.all([claimNotice("tx:included"), claimNotice("tx:included")]);
        expect(results.sort()).toEqual([false, true]);
    });

    it("发现它的标签页负责声音和系统通知", () => {
        const seen: boolean[] = [];
        const stop = onNotice((_, origin) => void seen.push(origin));
        const [notice] = incomingNotices([row()], own, null);
        publish(notice as Notice);
        stop();
        expect(seen).toEqual([true]);
    });
});

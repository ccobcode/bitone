import { beforeEach, describe, expect, it, vi } from "vitest";
import { db, txKey } from "./db";
import { hasOpenEntries, syncFeed } from "./feed";
import { GatewayError, fetchHistory, type HistoryItem, type HistoryPage } from "../gateway/client";

vi.mock("../gateway/client", async (actual) => ({
    ...(await actual<typeof import("../gateway/client")>()),
    fetchHistory: vi.fn(),
}));
vi.mock("../core", () => ({ networkList: () => [{ id: "bitcoin", chain: "bitcoin" }] }));

const history = vi.mocked(fetchHistory);
const WALLET = "w";
const STREAM = { network: "ethereum", addresses: ["0xa"] };

function item(hash: string, status: HistoryItem["status"] = "finalized", amount = 1n): HistoryItem {
    return { hash, time: 1, from: "0xa", to: "0xb", asset: "native", amount, status };
}

const page = (items: HistoryItem[], cursor: string | null = null, checkpoint = "100"): HistoryPage => ({
    items,
    cursor,
    checkpoint,
    unchanged: false,
});

/** 按游标回放页面，没有列出的游标抛错。 */
function pages(map: Record<string, HistoryPage>) {
    history.mockImplementation(async (_network, _addresses, cursor) => {
        const found = map[cursor ?? "top"];
        if (!found) throw new Error(`没有这一页：${cursor}`);
        return found;
    });
}

const hashes = async () => (await db.chainTxs.toArray()).map((row) => row.hash).sort();

beforeEach(async () => {
    history.mockReset();
    await Promise.all(db.tables.map((table) => table.clear()));
});

describe("txKey", () => {
    it("十六进制哈希转小写，Base58 不变", () => {
        const hex = `0x${"AB".repeat(32)}`;
        expect(txKey("ethereum", hex)).toBe(`ethereum:${hex.toLowerCase()}`);
        expect(txKey("tron", "AB".repeat(32))).toBe(`tron:${"ab".repeat(32)}`);
        expect(txKey("solana", "5VERv8NMvzbJMEkV")).toBe("solana:5VERv8NMvzbJMEkV");
    });
});

describe("syncFeed", () => {
    it("第一次下载到底，记下第一页的水位", async () => {
        pages({ top: page([item("a")], "p2", "100"), p2: page([item("b")], null, "105") });
        expect(await syncFeed(WALLET, [STREAM])).toEqual([]);
        expect(await hashes()).toEqual(["a", "b"]);
        const [cursor] = await db.feedCursors.toArray();
        expect(cursor).toMatchObject({ done: true, checkpoint: "100" });
    });

    it("再次打开带上水位，只写入新的", async () => {
        pages({ top: page([item("a")]) });
        await syncFeed(WALLET, [STREAM]);
        history.mockClear();

        pages({ top: page([item("new")], null, "120") });
        await syncFeed(WALLET, [STREAM]);
        expect(history).toHaveBeenCalledTimes(1);
        expect(history).toHaveBeenCalledWith("ethereum", ["0xa"], null, "100");
        expect(await hashes()).toEqual(["a", "new"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("120");
    });

    it("增量也按游标翻完，一次超过一页时不漏", async () => {
        pages({ top: page([item("a")]) });
        await syncFeed(WALLET, [STREAM]);
        pages({ top: page([item("b")], "p2", "120"), p2: page([item("c")], null, "125") });
        await syncFeed(WALLET, [STREAM]);
        expect(await hashes()).toEqual(["a", "b", "c"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("120");
    });

    it("被重组掉的交易从缓存里消失，已定论的留下", async () => {
        pages({ top: page([item("final"), item("gone", "included")]) });
        await syncFeed(WALLET, [STREAM]);
        pages({ top: page([item("new", "included")], null, "120") });
        await syncFeed(WALLET, [STREAM]);
        expect(await hashes()).toEqual(["final", "new"]);
    });

    it("未定论的记录更新成最新状态", async () => {
        pages({ top: page([item("a", "included")]) });
        await syncFeed(WALLET, [STREAM]);
        pages({ top: page([item("a", "finalized")], null, "120") });
        await syncFeed(WALLET, [STREAM]);
        expect((await db.chainTxs.toArray()).map((row) => row.status)).toEqual(["finalized"]);
    });

    it("增量中途失败时不删任何记录，水位不动", async () => {
        pages({ top: page([item("open", "included")]) });
        await syncFeed(WALLET, [STREAM]);
        pages({ top: page([item("new")], "p2", "120") });
        expect(await syncFeed(WALLET, [STREAM])).toEqual(["ethereum"]);
        expect(await hashes()).toEqual(["open"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("100");
    });

    it("没有变化时不动缓存，未定论的记录不能被当成重组", async () => {
        pages({ top: page([item("a", "included")]) });
        await syncFeed(WALLET, [STREAM]);
        pages({ top: { ...page([], null, "100"), unchanged: true } });
        await syncFeed(WALLET, [STREAM]);
        expect(await hashes()).toEqual(["a"]);
    });

    it("下载中断后先取增量，再从断点接着下", async () => {
        pages({ top: page([item("a")], "p2", "100"), p2: page([item("b")], "p3") });
        expect(await syncFeed(WALLET, [STREAM])).toEqual(["ethereum"]);
        expect(await hashes()).toEqual(["a", "b"]);
        history.mockClear();

        history.mockImplementation(async (_network, _addresses, cursor, since) => {
            if (since) return page([item("new")], null, "120");
            if (cursor === "p3") return page([item("c")]);
            throw new Error(`不应该重新下载：${cursor}`);
        });
        expect(await syncFeed(WALLET, [STREAM])).toEqual([]);
        expect(await hashes()).toEqual(["a", "b", "c", "new"]);
        expect((await db.feedCursors.toArray())[0]).toMatchObject({ done: true, checkpoint: "120" });
    });

    it("续传游标过期时清掉，下次从头下载", async () => {
        pages({ top: page([item("a")], "p2") });
        await syncFeed(WALLET, [STREAM]);
        history.mockImplementation(async (_network, _addresses, cursor, since) => {
            if (since) return page([], null, "120");
            if (cursor === "p2") throw new GatewayError("invalid_request");
            return page([item("a")]);
        });
        expect(await syncFeed(WALLET, [STREAM])).toEqual(["ethereum"]);
        expect(await db.feedCursors.count()).toBe(0);
        expect(await hashes()).toEqual(["a"]);
    });

    it("地址变多后从头下载，Bitcoin 一笔只留一条", async () => {
        const stream = { network: "bitcoin", addresses: ["bc1a"] };
        pages({ top: page([item("a", "finalized", 5n)]) });
        await syncFeed(WALLET, [stream]);
        history.mockClear();

        pages({ top: page([item("a", "finalized", 8n)], "p2"), p2: page([]) });
        await syncFeed(WALLET, [{ ...stream, addresses: ["bc1a", "bc1b"] }]);
        expect(history).toHaveBeenCalledTimes(2);
        expect(history).toHaveBeenCalledWith("bitcoin", ["bc1a", "bc1b"], null);
        expect((await db.chainTxs.toArray()).map((row) => row.amount)).toEqual(["8"]);
    });

    it("一个网络失败不影响其他网络", async () => {
        history.mockImplementation(async (network) => {
            if (network === "bsc") throw new GatewayError("upstream_unavailable");
            return page([item("a")]);
        });
        const failed = await syncFeed(WALLET, [STREAM, { network: "bsc", addresses: ["0xa"] }]);
        expect(failed).toEqual(["bsc"]);
        expect(await hashes()).toEqual(["a"]);
    });

    it("有未定论的记录时要继续同步", async () => {
        pages({ top: page([item("a", "included")]) });
        await syncFeed(WALLET, [STREAM]);
        expect(await hasOpenEntries(WALLET)).toBe(true);
        pages({ top: page([item("a", "finalized")], null, "120") });
        await syncFeed(WALLET, [STREAM]);
        expect(await hasOpenEntries(WALLET)).toBe(false);
    });
});

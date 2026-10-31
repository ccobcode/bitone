import { beforeEach, describe, expect, it, vi } from "vitest";
import { db, txKey } from "./db";
import { applyChainUpdate, hasOpenEntries, loadOlder, syncFeed } from "./feed";
import {
    GatewayError,
    fetchHistory,
    fetchSnapshot,
    fetchTxs,
    type HistoryItem,
    type HistoryPage,
    type SnapshotItem,
    type SnapshotResult,
} from "../gateway/client";

vi.mock("../gateway/client", async (actual) => ({
    ...(await actual<typeof import("../gateway/client")>()),
    fetchHistory: vi.fn(),
    fetchSnapshot: vi.fn(),
    fetchTxs: vi.fn(),
}));
vi.mock("../core", () => ({ networkList: () => [{ id: "bitcoin", chain: "bitcoin" }] }));

const history = vi.mocked(fetchHistory);
const snapshot = vi.mocked(fetchSnapshot);
const WALLET = "w";
const STREAM = { network: "ethereum", addresses: ["0xa"] };
const native = () => ["native"];

function item(hash: string, status: HistoryItem["status"] = "finalized", amount = 1n): HistoryItem {
    return { hash, time: 1, from: "0xa", to: "0xb", asset: "native", amount, status };
}

const page = (items: HistoryItem[], cursor: string | null = null, checkpoint = "100"): HistoryPage => ({
    items,
    cursor,
    checkpoint,
    unchanged: false,
});

/** 每批地址回一份结果。`extra` 能改写任何字段，比如让某个网络失败。 */
function answer(
    next: HistoryPage | null,
    extra: (network: string) => Partial<SnapshotResult> = () => ({}),
    balances: SnapshotResult["balances"] = [],
) {
    snapshot.mockImplementation(async (items, onResult) => {
        const groups = new Map<number, SnapshotItem[]>();
        for (const row of items) groups.set(row.group, [...(groups.get(row.group) ?? []), row]);
        for (const rows of groups.values()) {
            const network = rows[0]?.network ?? "";
            const addresses = [...new Set(rows.map((row) => row.address))];
            await onResult({ network, addresses, height: 10, balances, history: next, error: null, ...extra(network) });
        }
    });
}

const hashes = async () => (await db.chainTxs.toArray()).map((row) => row.hash).sort();
const balance = (amount: bigint) => ({ address: "0xa", asset: "native", amount });

beforeEach(async () => {
    history.mockReset();
    snapshot.mockReset();
    vi.mocked(fetchTxs).mockReset();
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
    it("一次请求带上全部地址和资产，代币带合约，原生币不带", async () => {
        answer(page([]));
        const bitcoin = { network: "bitcoin", addresses: ["bc1a", "bc1b"] };
        await syncFeed(WALLET, [STREAM, bitcoin], (network) =>
            network === "bitcoin" ? ["native"] : ["native", "0xt"],
        );
        expect(snapshot).toHaveBeenCalledTimes(1);
        expect(snapshot.mock.calls[0]?.[0]).toEqual([
            { network: "ethereum", address: "0xa", group: 0 },
            { network: "ethereum", address: "0xa", group: 0, contract: "0xt" },
            { network: "bitcoin", address: "bc1a", group: 1 },
            { network: "bitcoin", address: "bc1b", group: 1 },
        ]);
    });

    it("第一次只取最新一页，记下水位和往前翻的游标", async () => {
        answer(page([item("a")], "p2", "100"));
        expect(await syncFeed(WALLET, [STREAM], native)).toEqual({ failed: [], failures: [], missed: [] });
        expect(await hashes()).toEqual(["a"]);
        expect(history).not.toHaveBeenCalled();
        const [cursor] = await db.feedCursors.toArray();
        expect(cursor).toMatchObject({ addresses: ["0xa"], done: false, older: "p2", checkpoint: "100" });
    });

    it("再次打开带上水位，只写入新的", async () => {
        answer(page([item("a")]));
        await syncFeed(WALLET, [STREAM], native);

        answer(page([item("new")], null, "120"));
        await syncFeed(WALLET, [STREAM], native);
        expect(snapshot.mock.lastCall?.[0]).toEqual([{ network: "ethereum", address: "0xa", group: 0, since: "100" }]);
        expect(await hashes()).toEqual(["a", "new"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("120");
    });

    it("增量也按游标翻完，一次超过一页时不漏", async () => {
        answer(page([item("a")]));
        await syncFeed(WALLET, [STREAM], native);
        answer(page([item("b")], "p2", "120"));
        history.mockResolvedValue(page([item("c")], null, "125"));
        await syncFeed(WALLET, [STREAM], native);
        expect(history).toHaveBeenCalledWith("ethereum", ["0xa"], "p2", "100");
        expect(await hashes()).toEqual(["a", "b", "c"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("120");
    });

    it("被重组掉的交易从缓存里消失，已定论的留下", async () => {
        answer(page([item("final"), item("gone", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        answer(page([item("new", "included")], null, "120"));
        await syncFeed(WALLET, [STREAM], native);
        expect(await hashes()).toEqual(["final", "new"]);
    });

    it("未定论的记录更新成最新状态", async () => {
        answer(page([item("a", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        answer(page([item("a", "finalized")], null, "120"));
        await syncFeed(WALLET, [STREAM], native);
        expect((await db.chainTxs.toArray()).map((row) => row.status)).toEqual(["finalized"]);
    });

    it("增量中途失败时不删任何记录，水位不动", async () => {
        answer(page([item("open", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        answer(page([item("new")], "p2", "120"));
        history.mockRejectedValue(new GatewayError("upstream_unavailable"));
        expect((await syncFeed(WALLET, [STREAM], native)).failed).toEqual(["ethereum"]);
        expect(await hashes()).toEqual(["open"]);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("100");
    });

    it("没有变化时不动缓存，未定论的记录不能被当成重组", async () => {
        answer(page([item("a", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        answer({ ...page([], null, "100"), unchanged: true });
        await syncFeed(WALLET, [STREAM], native);
        expect(await hashes()).toEqual(["a"]);
    });

    it("一个网络失败不影响其他网络", async () => {
        answer(page([item("a")]), (network) =>
            network === "bsc" ? { height: null, balances: null, history: null, error: "upstream_unavailable" } : {},
        );
        const result = await syncFeed(WALLET, [STREAM, { network: "bsc", addresses: ["0xa"] }], native);
        expect(result).toEqual({
            failed: ["bsc"],
            failures: [{ network: "bsc", code: "upstream_unavailable" }],
            missed: [],
        });
        expect(await hashes()).toEqual(["a"]);
    });

    it("整个请求失败时每个网络都算失败", async () => {
        snapshot.mockRejectedValue(new GatewayError("rate_limited"));
        expect(await syncFeed(WALLET, [STREAM], native)).toEqual({
            failed: ["ethereum"],
            failures: [{ network: "ethereum", code: "rate_limited" }],
            missed: [],
        });
    });

    it("没有索引服务的网络不算同步失败，未定论的记录逐笔查状态", async () => {
        const stream = { network: "bsc", addresses: ["0xa"] };
        await applyChainUpdate(WALLET, stream, {
            height: 5,
            previous: 4,
            balances: [],
            items: [item("0xAB", "included")],
        });
        answer(null);
        vi.mocked(fetchTxs).mockResolvedValue([{ hash: "0xab", status: "finalized" } as never]);
        expect(await syncFeed(WALLET, [stream], native, ["bsc"])).toEqual({ failed: [], failures: [], missed: [] });
        expect((await db.chainTxs.toArray()).map((row) => row.status)).toEqual(["finalized"]);
        expect(await hasOpenEntries(WALLET, stream)).toBe(false);
    });

    it("有未定论的记录时要继续同步", async () => {
        answer(page([item("a", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        expect(await hasOpenEntries(WALLET, STREAM)).toBe(true);
        expect(await hasOpenEntries(WALLET, { ...STREAM, addresses: ["0xother"] })).toBe(false);
        answer(page([item("a", "finalized")], null, "120"));
        await syncFeed(WALLET, [STREAM], native);
        expect(await hasOpenEntries(WALLET, STREAM)).toBe(false);
    });
});

describe("快照和推送同时发生", () => {
    const amounts = async () => (await db.balances.toArray()).map((row) => [row.amount, row.height]);
    const update = (height: number, amount: bigint) => ({
        height,
        previous: height - 1,
        balances: [balance(amount)],
        items: [],
    });

    it("推送比快照新时，先到的推送不会被快照盖掉", async () => {
        await applyChainUpdate(WALLET, STREAM, update(12, 7n));
        answer(page([]), () => ({ height: 10 }), [balance(5n)]);
        await syncFeed(WALLET, [STREAM], native);
        expect(await amounts()).toEqual([["7", 12]]);
    });

    it("快照比推送新时，迟到的推送不会盖掉快照", async () => {
        answer(page([]), () => ({ height: 12 }), [balance(7n)]);
        await syncFeed(WALLET, [STREAM], native);
        await applyChainUpdate(WALLET, STREAM, update(10, 5n));
        expect(await amounts()).toEqual([["7", 12]]);
    });

    it("同一笔交易在快照和推送里都出现时只留一条", async () => {
        answer(page([item("a", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        await applyChainUpdate(WALLET, STREAM, { ...update(11, 1n), items: [item("a", "finalized")] });
        expect((await db.chainTxs.toArray()).map((row) => row.status)).toEqual(["finalized"]);
    });

    it("Bitcoin 一笔只留一条，金额以最新的归纳为准", async () => {
        const stream = { network: "bitcoin", addresses: ["bc1a"] };
        answer(page([item("a", "finalized", 5n)]));
        await syncFeed(WALLET, [stream], native);
        await applyChainUpdate(WALLET, stream, {
            ...update(11, 1n),
            balances: [],
            items: [item("a", "finalized", 8n)],
        });
        expect((await db.chainTxs.toArray()).map((row) => row.amount)).toEqual(["8"]);
    });
});

describe("loadOlder", () => {
    it("往前翻一页，翻完后不再有游标", async () => {
        answer(page([item("a")], "p2"));
        await syncFeed(WALLET, [STREAM], native);

        history.mockResolvedValue(page([item("b")]));
        await loadOlder(WALLET, [STREAM]);
        expect(history).toHaveBeenCalledWith("ethereum", ["0xa"], "p2");
        expect(await hashes()).toEqual(["a", "b"]);
        expect((await db.feedCursors.toArray())[0]).toMatchObject({ older: null, done: true });

        history.mockClear();
        await loadOlder(WALLET, [STREAM]);
        expect(history).not.toHaveBeenCalled();
    });

    it("翻页游标过期时清掉，下次从最新一页重来，已有记录不受影响", async () => {
        answer(page([item("a")], "p2"));
        await syncFeed(WALLET, [STREAM], native);
        history.mockRejectedValue(new GatewayError("invalid_request"));
        await loadOlder(WALLET, [STREAM]);
        expect(await db.feedCursors.count()).toBe(0);
        expect(await hashes()).toEqual(["a"]);
    });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { must } from "../testing";
import { db, txKey } from "./db";
import { applyChainUpdate, drainQueue, entry, hasOpenEntries, loadOlder, syncFeed, walletView } from "./feed";
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
        expect(cursor).toMatchObject({ addresses: ["0xa"], older: "p2", checkpoint: "100" });
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

    it("被重组掉的交易过了等待时间从缓存里消失，已定论的留下", async () => {
        vi.useFakeTimers({ toFake: ["Date"] });
        answer(page([item("final"), item("gone", "included")]));
        await syncFeed(WALLET, [STREAM], native);
        vi.setSystemTime(Date.now() + 10 * 60_000);
        answer(page([item("new", "included")], null, "120"));
        await syncFeed(WALLET, [STREAM], native);
        vi.useRealTimers();
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
        expect((await db.feedCursors.toArray())[0]).toMatchObject({ older: null });

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

describe("索引比推送慢时", () => {
    const LAG = 10 * 60_000;

    beforeEach(() => {
        vi.useFakeTimers({ toFake: ["Date"] });
    });
    afterEach(() => {
        vi.useRealTimers();
    });

    async function pushed(status: HistoryItem["status"] = "included"): Promise<void> {
        await syncFeed(WALLET, [STREAM], native);
        await applyChainUpdate(WALLET, STREAM, {
            height: 11,
            previous: 10,
            balances: [],
            items: [item("0xnew", status)],
        });
    }

    it("刚推送的记录不会被快照删掉，水位也不越过它", async () => {
        answer(page([], null, "100"));
        await pushed();
        answer(page([], null, "200"));
        await syncFeed(WALLET, [STREAM], native);
        expect(await db.chainTxs.count()).toBe(1);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("100");
    });

    it("重新出现时不算新记录，不会重复通知", async () => {
        answer(page([], null, "100"));
        await pushed();
        await syncFeed(WALLET, [STREAM], native);
        answer(page([item("0xnew")], null, "200"));
        const { missed } = await syncFeed(WALLET, [STREAM], native);
        expect(missed).toEqual([]);
        expect((await db.chainTxs.toArray())[0]?.status).toBe("finalized");
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("200");
    });

    it("过了等待时间还查不到才算被回滚，水位随后前进", async () => {
        answer(page([], null, "100"));
        await pushed();
        vi.setSystemTime(Date.now() + LAG);
        answer(page([], null, "200"));
        await syncFeed(WALLET, [STREAM], native);
        expect(await db.chainTxs.count()).toBe(0);
        expect((await db.feedCursors.toArray())[0]?.checkpoint).toBe("200");
    });
});

describe("记录标识", () => {
    it("本地记录的校验和地址和网关的小写地址是同一条", () => {
        const from = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
        const to = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
        const local = entry(WALLET, "local", "ethereum", { ...item("0x1"), from, to });
        const remote = entry(WALLET, "s", "ethereum", {
            ...item("0x1"),
            from: from.toLowerCase(),
            to: to.toLowerCase(),
        });
        expect(local.id).toBe(remote.id);
    });

    it("Base58 地址区分大小写，不能合并", () => {
        const one = entry(WALLET, "s", "tron", { ...item("ab"), from: "TAbc", to: "TDef" });
        const two = entry(WALLET, "s", "tron", { ...item("ab"), from: "Tabc", to: "TDef" });
        expect(one.id).not.toBe(two.id);
    });
});

describe("walletView", () => {
    const flows = {
        inputs: ["bc1a"],
        outputs: [
            { address: "bc1change", amount: "3" },
            { address: "bc1peer", amount: "7" },
        ],
    };
    // 网关只看到 bc1a 这一批，把找零当成了付款。
    const row = { from: "bc1a", to: "bc1change", amount: "10", flows };

    it("找零在另一批时不算付款，收款方也不是找零地址", () => {
        expect(walletView(row, new Set(["bc1a", "bc1change"]))).toEqual({ from: "bc1a", to: "bc1peer", amount: "7" });
    });

    it("还不知道找零地址时和网关的结果相同，发现之后自动改正", () => {
        expect(walletView(row, new Set(["bc1a"]))).toEqual({ from: "bc1a", to: "bc1change", amount: "10" });
    });

    it("全部付给自己时是转账，金额是移动的部分", () => {
        const own = new Set(["bc1a", "bc1change", "bc1peer"]);
        expect(walletView(row, own)).toEqual({ from: "bc1a", to: "bc1change", amount: "10" });
    });

    it("转入只算自己收到的部分", () => {
        expect(walletView(row, new Set(["bc1peer"]))).toEqual({ from: "bc1a", to: "bc1peer", amount: "7" });
    });

    it("数据输出没有地址，不算收款方", () => {
        const outputs = [{ address: null, amount: "0" }, ...flows.outputs];
        const view = walletView({ ...row, flows: { ...flows, outputs } }, new Set(["bc1a", "bc1change"]));
        expect(view.to).toBe("bc1peer");
    });

    it("没有输入输出时用保存的结果", () => {
        const { from, to, amount } = row;
        expect(walletView({ from, to, amount }, new Set(["bc1peer"]))).toEqual({ from, to, amount });
    });

    it("两批地址都写入同一笔时只留一条，归纳结果和写入顺序无关", async () => {
        const first = { network: "bitcoin", addresses: ["bc1a"] };
        const second = { network: "bitcoin", addresses: ["bc1change"] };
        const seen = { ...item("t"), from: "bc1a", to: "bc1change", amount: 10n, flows };
        const change = { ...item("t"), from: "bc1a", to: "bc1change", amount: 3n, flows };
        answer(page([seen]));
        await syncFeed(WALLET, [first], native);
        answer(page([change]));
        await syncFeed(WALLET, [second], native);
        const rows = await db.chainTxs.toArray();
        expect(rows).toHaveLength(1);
        const own = new Set(["bc1a", "bc1change"]);
        expect(walletView(must(rows[0]), own)).toEqual({ from: "bc1a", to: "bc1peer", amount: "7" });
    });
});

describe("drainQueue", () => {
    it("一批出错不影响后面的批次，跑完后可以再次同步", async () => {
        const run = { running: false, queue: new Map([["a", "a"]]) };
        const failed: string[][] = [];
        const done: string[][] = [];
        const step = async (batch: string[]) => {
            if (batch.includes("a")) {
                run.queue.set("b", "b");
                throw new Error("通知抛错");
            }
            done.push(batch);
        };
        await drainQueue(run, 0, step, (batch) => failed.push(batch));
        expect(run.running).toBe(false);
        expect(failed).toEqual([["a"]]);
        expect(done).toEqual([["b"]]);
    });

    it("等待期间排队的批次合成一轮", async () => {
        const run = { running: false, queue: new Map([["a", "a"]]) };
        const batches: string[][] = [];
        const draining = drainQueue(run, 5, async (batch) => void batches.push(batch), vi.fn());
        expect(run.running).toBe(true);
        run.queue.set("b", "b");
        await draining;
        expect(batches).toEqual([["a", "b"]]);
    });
});

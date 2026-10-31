import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Account, Address } from "../data/db";
import { EVM_ADDRESS as EVM, TRON_ADDRESS, USDT, initCore, stubFetch, stubOffline } from "../testing";
import { buildRows, buildTargets } from "./assets";
import {
    GatewayError,
    fetchSnapshot,
    parseBalances,
    parseUpdate,
    type SnapshotResult,
    parseObservation,
    parsePrices,
    type Balance,
} from "./client";
import { BalanceSocket, retryDelay } from "./socket";

const evmAccount: Account = {
    id: "w:evm",
    walletId: "w",
    chain: "evm",
    kind: "single",
    network: "ethereum",
    path: "m/44'/60'/0'/0/0",
    key: new Uint8Array(33),
};
const legacy: Account = {
    ...evmAccount,
    id: "w:leg",
    chain: "bitcoin",
    kind: "legacy",
    network: "bitcoin",
    path: "m/44'/0'/0'",
};
const address = (accountId: string, value: string, index = 0): Address => ({
    accountId,
    change: 0,
    index,
    address: value,
    verified: false,
});

beforeAll(initCore);

afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
});

describe("parseBalances", () => {
    it("丢弃没请求过的地址和不合法的金额", () => {
        const list = [
            { address: EVM, asset: "native", amount: "1" },
            { address: "0x0000000000000000000000000000000000000001", asset: "native", amount: "9" },
            { address: EVM, asset: USDT, amount: "1e3" },
            { address: EVM, asset: USDT, amount: "0x10" },
            { address: EVM, asset: 5, amount: "1" },
            null,
        ];
        expect(parseBalances(list, [EVM])).toEqual([{ address: EVM, asset: "native", amount: 1n }]);
        expect(parseBalances("oops", [EVM])).toEqual([]);
    });

    it("EVM 地址按小写比对,结果换回本地写法", () => {
        const [row] = parseBalances([{ address: EVM.toLowerCase(), asset: "native", amount: "1" }], [EVM]);
        expect(row?.address).toBe(EVM);
    });

    it("Base58 地址区分大小写", () => {
        const row = { address: TRON_ADDRESS.toLowerCase(), asset: "native", amount: "1" };
        expect(parseBalances([row], [TRON_ADDRESS])).toEqual([]);
    });

    it("重复条目不会累加", () => {
        const entry = { address: EVM, asset: "native", amount: "5" };
        expect(parseBalances([entry, entry], [EVM])).toEqual([{ address: EVM, asset: "native", amount: 5n }]);
    });

    it("去掉资产字符串里的不可见字符", () => {
        const [row] = parseBalances([{ address: EVM, asset: "ab‮cd\u0000", amount: "1" }], [EVM]);
        expect(row?.asset).toBe("abcd");
    });
});

describe("fetchSnapshot", () => {
    const item = { network: "bitcoin", address: "a", group: 0 };
    const collect = async () => {
        const results: SnapshotResult[] = [];
        await fetchSnapshot([item], async (result) => void results.push(result));
        return results;
    };

    it("逐行处理,一行被拆成两段也能拼回来,余额和历史分开报告失败", async () => {
        const encoder = new TextEncoder();
        const first = {
            network: "bitcoin",
            addresses: ["a"],
            height: 7,
            balances: [{ address: "A", asset: "native", amount: "5" }],
            history: null,
        };
        const text = `${JSON.stringify(first)}\n${JSON.stringify({ network: "bsc", addresses: ["b"], error: "upstream_unavailable" })}\n`;
        const body = new ReadableStream({
            start(controller) {
                controller.enqueue(encoder.encode(text.slice(0, 20)));
                controller.enqueue(encoder.encode(text.slice(20)));
                controller.close();
            },
        });
        stubFetch(async () => new Response(body));
        const [ok, failed] = await collect();
        expect(ok).toMatchObject({ height: 7, history: null, error: null });
        expect(ok?.balances).toEqual([]);
        expect(failed).toMatchObject({ height: null, balances: null, error: "upstream_unavailable" });
    });

    it("网关错误带错误码,连不上时也是 GatewayError", async () => {
        stubFetch(async () => Response.json({ error: { code: "rate_limited", message: "x" } }, { status: 429 }));
        await expect(collect()).rejects.toMatchObject({ code: "rate_limited" });
        stubOffline();
        await expect(collect()).rejects.toBeInstanceOf(GatewayError);
    });

    it("正文不完整时报错,不完整的最后一行不算结果", async () => {
        stubFetch(async () => new Response("<html></html>\n"));
        await expect(collect()).rejects.toBeInstanceOf(GatewayError);
        stubFetch(async () => new Response('{"network":"bitcoin"'));
        expect(await collect()).toEqual([]);
    });
});

describe("parseUpdate", () => {
    it("高度不完整的推送不要,txs 为空值表示网关没取到", () => {
        expect(parseUpdate({ height: 2 }, ["a"])).toBeNull();
        expect(parseUpdate({ height: 2, previous: 1, balances: [], txs: null }, ["a"])).toEqual({
            height: 2,
            previous: 1,
            balances: [],
            items: null,
        });
    });
});

describe("资产表", () => {
    const rows = (balances: Balance[]) =>
        buildRows(
            [evmAccount, legacy],
            [address("w:evm", EVM), address("w:leg", "1Legacy")],
            "mainnet",
            ["bitcoin", "ethereum", "bsc"],
            new Map([["ethereum", balances]]),
        );

    it("只查网关支持的网络", () => {
        const targets = buildTargets([evmAccount], [address("w:evm", EVM)], "mainnet", ["ethereum", "bitcoin"]);
        expect(targets).toEqual([{ network: "ethereum", addresses: [EVM] }]);
    });

    it("没有余额时也有原生币行,旧路径账户没有余额时不显示", () => {
        const list = rows([]);
        expect(list.map((r) => r.network)).toEqual(["ethereum", "bsc"]);
        expect(list[0]?.entry?.symbol).toBe("ETH");
    });

    it("还没查到的网络标记为未加载,已查到的不标记", () => {
        const list = rows([]);
        expect(list.find((r) => r.network === "ethereum")?.loaded).toBe(true);
        expect(list.find((r) => r.network === "bsc")?.loaded).toBe(false);
    });

    it("清单外的代币没有符号和小数位", () => {
        const fake = "0x0000000000000000000000000000000000000001";
        const list = rows([{ address: EVM, asset: fake, amount: 1000000n }]);
        const row = list.find((r) => r.asset === fake);
        expect(row?.entry).toBeNull();
        expect(row?.amount).toBe(1000000n);
        expect(list.at(-1)).toBe(row);
    });

    it("同一账户多个地址的余额用 BigInt 相加", () => {
        const big = 9007199254740993n;
        const list = buildRows(
            [{ ...evmAccount, id: "w:btc", chain: "bitcoin", kind: "segwit", network: "bitcoin" }],
            [address("w:btc", "a", 0), address("w:btc", "b", 1)],
            "mainnet",
            ["bitcoin"],
            new Map([
                [
                    "bitcoin",
                    [
                        { address: "a", asset: "native", amount: big },
                        { address: "b", asset: "native", amount: 1n },
                    ],
                ],
            ]),
        );
        expect(list[0]?.amount).toBe(big + 1n);
        expect(list[0]?.address).toBe("a");
    });
});

class FakeSocket {
    static OPEN = 1;
    static last: FakeSocket;
    static count = 0;
    readyState = 0;
    sent: { id: number; params: { network: string; addresses: string[] } }[] = [];
    pings: { id: number; method: string }[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((event: { data: unknown }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(readonly url: string) {
        FakeSocket.last = this;
        FakeSocket.count += 1;
    }
    send(data: string) {
        const message = JSON.parse(data);
        if (message.method === "ping") this.pings.push(message);
        else this.sent.push(message);
    }
    close() {
        this.readyState = 3;
    }
    open() {
        this.readyState = 1;
        this.onopen?.();
    }
    push(data: unknown) {
        this.onmessage?.({ data: JSON.stringify(data) });
    }
}

describe("BalanceSocket", () => {
    function setup() {
        vi.useFakeTimers();
        FakeSocket.count = 0;
        vi.stubGlobal("WebSocket", FakeSocket);
        const events = {
            onStatus: vi.fn(),
            onUpdate: vi.fn(),
            onReady: vi.fn(),
            onBatchReady: vi.fn(),
            onLatency: vi.fn(),
        };
        const socket = new BalanceSocket("ws://x/v1/ws", events);
        socket.setTargets([{ network: "ethereum", addresses: [EVM] }]);
        socket.start();
        return { socket, events };
    }

    it("退避有上限", () => {
        expect(retryDelay(0, 1)).toBe(1000);
        expect(retryDelay(3, 1)).toBe(8000);
        expect(retryDelay(20, 1)).toBe(30_000);
    });

    it("连接后订阅,按订阅 ID 分发推送", () => {
        const { events } = setup();
        FakeSocket.last.open();
        const [request] = FakeSocket.last.sent;
        expect(request?.params).toMatchObject({ channel: "addresses", network: "ethereum", addresses: [EVM] });
        FakeSocket.last.push({ jsonrpc: "2.0", id: request?.id, result: "sub-1" });
        FakeSocket.last.push({
            jsonrpc: "2.0",
            method: "update",
            params: { subscription: "sub-1", height: 5 },
        });
        FakeSocket.last.push({
            jsonrpc: "2.0",
            method: "update",
            params: { subscription: "sub-9", height: 5 },
        });
        expect(events.onUpdate).toHaveBeenCalledTimes(1);
        expect(events.onUpdate).toHaveBeenCalledWith("ethereum", [EVM], { subscription: "sub-1", height: 5 });
    });

    it("全部订阅回复后才通知 HTTP 补一次", () => {
        const { socket, events } = setup();
        socket.setTargets([
            { network: "ethereum", addresses: [EVM] },
            { network: "bsc", addresses: [EVM] },
        ]);
        FakeSocket.last.open();
        const [first, second] = FakeSocket.last.sent;
        expect(events.onReady).not.toHaveBeenCalled();
        FakeSocket.last.push({ jsonrpc: "2.0", id: first?.id, result: "1" });
        expect(events.onReady).not.toHaveBeenCalled();
        FakeSocket.last.push({ jsonrpc: "2.0", id: second?.id, result: "2" });
        expect(events.onReady).toHaveBeenCalledTimes(1);
    });

    it("没有地址时连接打开就算就绪", () => {
        const { socket, events } = setup();
        socket.setTargets([]);
        FakeSocket.last.open();
        expect(events.onReady).toHaveBeenCalledTimes(1);
    });

    it("订阅被拒绝时不算实时", () => {
        const { events } = setup();
        FakeSocket.last.open();
        const [request] = FakeSocket.last.sent;
        FakeSocket.last.push({ jsonrpc: "2.0", id: request?.id, error: { code: -32005, message: "x" } });
        expect(events.onStatus).toHaveBeenLastCalledWith("degraded", null);
        expect(events.onReady).toHaveBeenCalledTimes(1);
    });

    it("断线后退避重连,重新订阅并通知补一次", () => {
        const { events } = setup();
        FakeSocket.last.open();
        FakeSocket.last.push({ jsonrpc: "2.0", id: FakeSocket.last.sent[0]?.id, result: "sub-1" });
        expect(events.onReady).toHaveBeenCalledTimes(1);
        const first = FakeSocket.last;
        first.onclose?.();
        expect(events.onStatus).toHaveBeenLastCalledWith("retrying", expect.any(Number));
        vi.advanceTimersByTime(1000);
        expect(FakeSocket.count).toBe(2);
        expect(FakeSocket.last).not.toBe(first);
        FakeSocket.last.open();
        expect(FakeSocket.last.sent).toHaveLength(1);
        expect(events.onStatus).toHaveBeenLastCalledWith("live", null);
        FakeSocket.last.push({ jsonrpc: "2.0", id: FakeSocket.last.sent[0]?.id, result: "sub-2" });
        expect(events.onReady).toHaveBeenCalledTimes(2);
    });

    it("连续失败时间隔变长,立即重连清零退避", () => {
        const { socket } = setup();
        FakeSocket.last.onclose?.();
        vi.advanceTimersByTime(1000);
        FakeSocket.last.onclose?.();
        vi.advanceTimersByTime(1000);
        expect(FakeSocket.count).toBe(2);
        vi.advanceTimersByTime(1000);
        expect(FakeSocket.count).toBe(3);
        socket.reconnect();
        expect(FakeSocket.count).toBe(4);
    });

    it("停止后不再重连", () => {
        const { socket } = setup();
        FakeSocket.last.open();
        const last = FakeSocket.last;
        socket.stop();
        last.onclose?.();
        vi.advanceTimersByTime(60_000);
        expect(FakeSocket.count).toBe(1);
    });

    it("地址变化时只换这个网络的订阅,不换连接,也不触发全量补查", () => {
        const { socket, events } = setup();
        socket.setTargets([
            { network: "ethereum", addresses: [EVM] },
            { network: "bitcoin", addresses: ["b"] },
        ]);
        FakeSocket.last.open();
        const [ethereum, bitcoin] = FakeSocket.last.sent;
        FakeSocket.last.push({ id: ethereum?.id, result: "s1" });
        FakeSocket.last.push({ id: bitcoin?.id, result: "s2" });
        expect(events.onReady).toHaveBeenCalledTimes(1);

        FakeSocket.last.sent.length = 0;
        socket.setTargets([
            { network: "ethereum", addresses: [EVM, "x"] },
            { network: "bitcoin", addresses: ["b"] },
        ]);
        expect(FakeSocket.count).toBe(1);
        const [unsubscribe, subscribe] = FakeSocket.last.sent as unknown as {
            id: number;
            method: string;
            params: { addresses: string[] };
        }[];
        expect(unsubscribe).toMatchObject({ method: "unsubscribe", params: ["s1"] });
        expect(subscribe).toMatchObject({ method: "subscribe", params: { addresses: [EVM, "x"] } });
        expect(FakeSocket.last.sent).toHaveLength(2);

        FakeSocket.last.push({ id: subscribe?.id, result: "s3" });
        expect(events.onBatchReady).toHaveBeenLastCalledWith({ network: "ethereum", addresses: [EVM, "x"] });
        expect(events.onReady).toHaveBeenCalledTimes(1);
    });

    it("每批地址的订阅确认后才取快照,这批确认时另一批还可以没回复", () => {
        const { socket, events } = setup();
        socket.setTargets([
            { network: "ethereum", addresses: [EVM] },
            { network: "bitcoin", addresses: ["b"] },
        ]);
        FakeSocket.last.open();
        const [ethereum] = FakeSocket.last.sent;
        expect(events.onBatchReady).not.toHaveBeenCalled();
        FakeSocket.last.push({ id: ethereum?.id, result: "s1" });
        expect(events.onBatchReady).toHaveBeenCalledExactlyOnceWith({ network: "ethereum", addresses: [EVM] });
    });

    it("延迟来自 WebSocket 心跳,不用 HTTP", () => {
        const { events } = setup();
        FakeSocket.last.open();
        FakeSocket.last.push({ id: FakeSocket.last.pings[0]?.id, result: "pong" });
        expect(events.onLatency).toHaveBeenCalledTimes(1);
    });

    it("订阅还没回复地址就变了,回复后退订旧的,只用新的", () => {
        const { socket, events } = setup();
        FakeSocket.last.open();
        const old = FakeSocket.last.sent[0];
        FakeSocket.last.push({ id: old?.id, result: "s1" });
        FakeSocket.last.sent.length = 0;

        socket.setTargets([{ network: "ethereum", addresses: [EVM, "x"] }]);
        const first = FakeSocket.last.sent.at(-1);
        socket.setTargets([{ network: "ethereum", addresses: [EVM, "x", "y"] }]);
        const second = FakeSocket.last.sent.at(-1);
        expect(second?.params.addresses).toEqual([EVM, "x", "y"]);

        FakeSocket.last.push({ id: first?.id, result: "late" });
        expect(FakeSocket.last.sent.at(-1)).toMatchObject({ method: "unsubscribe", params: ["late"] });
        expect(events.onBatchReady).not.toHaveBeenCalledWith({ network: "ethereum", addresses: [EVM, "x"] });
        FakeSocket.last.push({ id: second?.id, result: "s3" });
        expect(events.onBatchReady).toHaveBeenLastCalledWith({ network: "ethereum", addresses: [EVM, "x", "y"] });

        FakeSocket.last.push({ method: "update", params: { subscription: "late", address: "x", balances: [] } });
        expect(events.onUpdate).not.toHaveBeenCalled();
    });

    it("地址多的网络不能把其他网络的订阅挤掉", () => {
        const { socket } = setup();
        const many = (network: string, seed: string) => ({
            network,
            addresses: Array.from({ length: 100 }, (_, i) => `${seed}${i}`),
        });
        const crowded = Array.from({ length: 60 }, (_, i) => many("btc-testnet4", `b${i}-`));
        socket.setTargets([...crowded, { network: "ethereum", addresses: [EVM] }]);
        FakeSocket.last.open();
        const networks = FakeSocket.last.sent.map((message) => message.params.network);
        expect(networks).toContain("ethereum");
        expect(FakeSocket.last.sent.flatMap((message) => message.params.addresses).length).toBeLessThanOrEqual(5000);
    });

    it("超过单次上限的地址分批订阅", () => {
        const { socket } = setup();
        socket.setTargets([{ network: "bitcoin", addresses: Array.from({ length: 150 }, (_, i) => `a${i}`) }]);
        FakeSocket.last.open();
        expect(FakeSocket.last.sent.map((m) => m.params.addresses.length)).toEqual([100, 50]);
    });
});

describe("交易和价格订阅", () => {
    const HASH = `0x${"ab".repeat(32)}`;

    function setup(events = {}) {
        vi.useFakeTimers();
        FakeSocket.count = 0;
        vi.stubGlobal("WebSocket", FakeSocket);
        const all = {
            onStatus: vi.fn(),
            onUpdate: vi.fn(),
            onReady: vi.fn(),
            onTx: vi.fn(),
            onTxWatched: vi.fn(),
            onPrices: vi.fn(),
            ...events,
        };
        const socket = new BalanceSocket("ws://x/v1/ws", all);
        socket.start();
        return { socket, events: all };
    }

    const reply = (id: number | undefined, result: string) => FakeSocket.last.push({ jsonrpc: "2.0", id, result });

    it("连接时订阅已有的交易,推送交给上层,不换连接", () => {
        const { socket, events } = setup();
        socket.watchTxs([{ network: "ethereum", hash: HASH }]);
        FakeSocket.last.open();
        const [request] = FakeSocket.last.sent;
        expect(request?.params).toMatchObject({ channel: "tx", network: "ethereum", hash: HASH });
        reply(request?.id, "sub-1");
        expect(events.onReady).toHaveBeenCalledTimes(1);
        const tx = { hash: HASH, status: "included" };
        FakeSocket.last.push({ jsonrpc: "2.0", method: "update", params: { subscription: "sub-1", tx } });
        expect(events.onTx).toHaveBeenCalledWith("ethereum", tx);
        expect(FakeSocket.count).toBe(1);
    });

    it("连接打开后新增的交易单独订阅,回复后通知用 HTTP 补一次,基线不会推送", () => {
        const { socket, events } = setup();
        FakeSocket.last.open();
        expect(events.onReady).toHaveBeenCalledTimes(1);
        socket.watchTxs([{ network: "ethereum", hash: HASH }]);
        const [request] = FakeSocket.last.sent;
        reply(request?.id, "sub-1");
        expect(events.onTxWatched).toHaveBeenCalledWith("ethereum", HASH);
        expect(events.onReady).toHaveBeenCalledTimes(1);
        expect(FakeSocket.count).toBe(1);
    });

    it("交易结束后退订,同一笔不重复订阅", () => {
        const { socket } = setup();
        FakeSocket.last.open();
        socket.watchTxs([{ network: "ethereum", hash: HASH }]);
        socket.watchTxs([{ network: "ethereum", hash: HASH }]);
        expect(FakeSocket.last.sent).toHaveLength(1);
        reply(FakeSocket.last.sent[0]?.id, "sub-1");
        socket.watchTxs([]);
        expect(FakeSocket.last.sent[1]).toMatchObject({ method: "unsubscribe", params: ["sub-1"] });
    });

    it("断线重连后重新订阅全部交易", () => {
        const { socket } = setup();
        socket.watchTxs([{ network: "ethereum", hash: HASH }]);
        FakeSocket.last.open();
        FakeSocket.last.onclose?.();
        vi.advanceTimersByTime(1000);
        FakeSocket.last.open();
        expect(FakeSocket.last.sent[0]?.params).toMatchObject({ channel: "tx", hash: HASH });
    });

    it("单连接最多订阅 100 笔,超出的靠轮询", () => {
        const { socket } = setup();
        socket.watchTxs(Array.from({ length: 120 }, (_, i) => ({ network: "ethereum", hash: `h${i}` })));
        FakeSocket.last.open();
        expect(FakeSocket.last.sent).toHaveLength(100);
    });

    it("心跳没有回应时重连,有回应时保持连接", () => {
        const { events } = setup();
        const first = FakeSocket.last;
        first.open();
        reply(first.sent[0]?.id, "sub-1");

        vi.advanceTimersByTime(25_000);
        expect(first.pings.at(-1)).toMatchObject({ method: "ping" });
        first.push({ jsonrpc: "2.0", id: first.sent.at(-1)?.id, result: "pong" });
        vi.advanceTimersByTime(25_000);
        expect(FakeSocket.last).toBe(first);

        // 这一次没有回应,半开连接不会触发 close,要自己断开.
        vi.advanceTimersByTime(25_000);
        expect(events.onStatus).toHaveBeenLastCalledWith("retrying", expect.any(Number));
        vi.advanceTimersByTime(2_000);
        expect(FakeSocket.last).not.toBe(first);
    });

    it("价格频道", () => {
        const { socket, events } = setup();
        socket.watchPrices();
        FakeSocket.last.open();
        const [request] = FakeSocket.last.sent;
        expect(request?.params).toEqual({ channel: "prices" });
        reply(request?.id, "sub-1");
        FakeSocket.last.push({
            jsonrpc: "2.0",
            method: "update",
            params: { subscription: "sub-1", prices: { a: "1" } },
        });
        expect(events.onPrices).toHaveBeenCalledWith({ a: "1" });
    });
});

describe("parseObservation", () => {
    it("校验字段,金额不是整数时当成没有", () => {
        expect(
            parseObservation({
                hash: "a",
                status: "failed",
                confirmations: 2,
                stage: "latest",
                block: 5,
                fee: "21000",
            }),
        ).toEqual({
            hash: "a",
            status: "failed",
            confirmations: 2,
            stage: "latest",
            block: 5,
            fee: "21000",
        });
        expect(
            parseObservation({ hash: "a", status: "pending", fee: "1e5", block: "x", confirmations: -1 }),
        ).toMatchObject({
            fee: null,
            block: null,
            confirmations: 0,
        });
        expect(parseObservation({ hash: "a", status: "weird" })).toBeNull();
        expect(parseObservation(null)).toBeNull();
    });
});

describe("parsePrices", () => {
    it("键统一成小写合约,Base58 不变", () => {
        const prices = parsePrices({
            [`ethereum:${USDT}`]: "1.00",
            "tron:TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t": "1.00",
            bad: "1",
            "x:y": 2,
        });
        expect([...prices.keys()]).toEqual([
            `ethereum:${USDT.toLowerCase()}`,
            "tron:TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        ]);
    });
});

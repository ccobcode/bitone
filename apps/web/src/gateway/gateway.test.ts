import { readFileSync } from "node:fs";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { initCoreSync } from "../core";
import type { Account, Address } from "../data/db";
import { applyUpdate, buildRows, buildTargets } from "./assets";
import {
    GatewayError,
    MAX_ADDRESSES,
    fetchBalances,
    parseBalances,
    parseObservation,
    parsePrices,
    type Balance,
} from "./client";
import { BalanceSocket, retryDelay } from "./socket";

const EVM = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
const USDT = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

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

beforeAll(() => {
    initCoreSync(readFileSync(new URL("../core/pkg/bitone_wasm_bg.wasm", import.meta.url)));
});

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

    it("EVM 地址按小写比对，结果换回本地写法", () => {
        const [row] = parseBalances([{ address: EVM.toLowerCase(), asset: "native", amount: "1" }], [EVM]);
        expect(row?.address).toBe(EVM);
    });

    it("Base58 地址区分大小写", () => {
        const tron = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";
        expect(parseBalances([{ address: tron.toLowerCase(), asset: "native", amount: "1" }], [tron])).toEqual([]);
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

describe("fetchBalances", () => {
    it("超过单次上限时分批请求", async () => {
        const fetchMock = vi.fn(async () => Response.json({ balances: [] }));
        vi.stubGlobal("fetch", fetchMock);
        const addresses = Array.from({ length: MAX_ADDRESSES * 2 + 1 }, (_, i) => `a${i}`);
        await fetchBalances("bitcoin", addresses);
        expect(fetchMock).toHaveBeenCalledTimes(3);
    });

    it("网关错误带错误码，连不上时也是 GatewayError", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () =>
                Response.json({ error: { code: "upstream_unavailable", message: "x" } }, { status: 503 }),
            ),
        );
        await expect(fetchBalances("bitcoin", ["a"])).rejects.toMatchObject({ code: "upstream_unavailable" });
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => Promise.reject(new TypeError("failed"))),
        );
        await expect(fetchBalances("bitcoin", ["a"])).rejects.toBeInstanceOf(GatewayError);
    });
});

describe("fetchBalances 的异常响应", () => {
    it("正文不完整时报错，不当成余额为 0", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => new Response("<html></html>")),
        );
        await expect(fetchBalances("bitcoin", ["a"])).rejects.toBeInstanceOf(GatewayError);
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => Response.json({})),
        );
        await expect(fetchBalances("bitcoin", ["a"])).rejects.toBeInstanceOf(GatewayError);
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

    it("没有余额时也有原生币行，旧路径账户没有余额时不显示", () => {
        const list = rows([]);
        expect(list.map((r) => r.network)).toEqual(["ethereum", "bsc"]);
        expect(list[0]?.entry?.symbol).toBe("ETH");
    });

    it("还没查到的网络标记为未加载，已查到的不标记", () => {
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

    it("推送替换该地址的余额，不动其他地址", () => {
        const other = { address: "b", asset: "native", amount: 2n };
        const next = applyUpdate([{ address: "a", asset: "native", amount: 1n }, other], "a", [
            { address: "a", asset: "native", amount: 7n },
        ]);
        expect(next).toEqual([other, { address: "a", asset: "native", amount: 7n }]);
    });
});

class FakeSocket {
    static OPEN = 1;
    static last: FakeSocket;
    static count = 0;
    readyState = 0;
    sent: { id: number; params: { network: string; addresses: string[] } }[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((event: { data: unknown }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(readonly url: string) {
        FakeSocket.last = this;
        FakeSocket.count += 1;
    }
    send(data: string) {
        this.sent.push(JSON.parse(data));
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
        const events = { onStatus: vi.fn(), onUpdate: vi.fn(), onReady: vi.fn() };
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

    it("连接后订阅，按订阅 ID 分发推送", () => {
        const { events } = setup();
        FakeSocket.last.open();
        const [request] = FakeSocket.last.sent;
        expect(request?.params).toMatchObject({ channel: "addresses", network: "ethereum", addresses: [EVM] });
        FakeSocket.last.push({ jsonrpc: "2.0", id: request?.id, result: "sub-1" });
        FakeSocket.last.push({
            jsonrpc: "2.0",
            method: "update",
            params: { subscription: "sub-1", address: EVM, balances: [] },
        });
        FakeSocket.last.push({
            jsonrpc: "2.0",
            method: "update",
            params: { subscription: "sub-9", address: EVM, balances: [] },
        });
        expect(events.onUpdate).toHaveBeenCalledTimes(1);
        expect(events.onUpdate).toHaveBeenCalledWith("ethereum", EVM, []);
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

    it("断线后退避重连，重新订阅并通知补一次", () => {
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

    it("连续失败时间隔变长，立即重连清零退避", () => {
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

    it("地址变化时换连接重新订阅", () => {
        const { socket } = setup();
        FakeSocket.last.open();
        socket.setTargets([{ network: "ethereum", addresses: [EVM, "x"] }]);
        expect(FakeSocket.count).toBe(2);
        FakeSocket.last.open();
        expect(FakeSocket.last.sent[0]?.params.addresses).toEqual([EVM, "x"]);
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

    it("连接时订阅已有的交易，推送交给上层，不换连接", () => {
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

    it("连接打开后新增的交易单独订阅，回复后通知用 HTTP 补一次，基线不会推送", () => {
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

    it("交易结束后退订，同一笔不重复订阅", () => {
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

    it("单连接最多订阅 100 笔，超出的靠轮询", () => {
        const { socket } = setup();
        socket.watchTxs(Array.from({ length: 120 }, (_, i) => ({ network: "ethereum", hash: `h${i}` })));
        FakeSocket.last.open();
        expect(FakeSocket.last.sent).toHaveLength(100);
    });

    it("心跳没有回应时重连，有回应时保持连接", () => {
        const { events } = setup();
        const first = FakeSocket.last;
        first.open();
        reply(first.sent[0]?.id, "sub-1");

        vi.advanceTimersByTime(25_000);
        expect(first.sent.at(-1)).toMatchObject({ method: "ping" });
        first.push({ jsonrpc: "2.0", id: first.sent.at(-1)?.id, result: "pong" });
        vi.advanceTimersByTime(25_000);
        expect(FakeSocket.last).toBe(first);

        // 这一次没有回应，半开连接不会触发 close，要自己断开。
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
    it("校验字段，金额不是整数时当成没有", () => {
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
    it("键统一成小写合约，Base58 不变", () => {
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

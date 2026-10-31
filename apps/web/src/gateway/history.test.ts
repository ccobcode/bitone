import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EVM_ADDRESS, PEER_EVM, USDT, initCore } from "../testing";
import type { Account, Address, Note, TxRecord } from "../data/db";
import { addressKey, type HistoryItem } from "./client";
import { historyStreams, mergeFeed, type RemoteItem } from "./history";

beforeAll(initCore);
afterEach(() => vi.unstubAllGlobals());

const item = (extra: Partial<HistoryItem> = {}): HistoryItem => ({
    hash: "0x1",
    time: 1_800_000_000,
    from: PEER_EVM,
    to: EVM_ADDRESS,
    asset: "native",
    amount: 10n ** 18n,
    status: "finalized",
    ...extra,
});

const remote = (extra: Partial<HistoryItem> = {}): RemoteItem => ({
    id: `ethereum:${extra.hash ?? "0x1"}:${extra.asset ?? "native"}`,
    network: "ethereum",
    item: item(extra),
});
const own = new Set([addressKey(EVM_ADDRESS)]);

function local(extra: Partial<TxRecord> = {}): TxRecord {
    return {
        id: "TX-1",
        walletId: "w",
        accountId: "a",
        chain: "evm",
        network: "ethereum",
        kind: "transfer",
        state: "broadcast",
        asset: "native",
        amount: "1000000000000000000",
        from: EVM_ADDRESS,
        to: PEER_EVM,
        request: new Uint8Array(1),
        requestId: "id",
        hash: "0x1",
        plan: { chain: "evm", nonce: 1, gasLimit: 21000, maxFee: "1", maxPriority: "1" },
        createdAt: 1_800_000_500_000,
        updatedAt: 0,
        expiresAt: null,
        hardExpiry: false,
        timeline: [],
        attempts: 0,
        conflicts: 0,
        confirmations: 0,
        stage: null,
        block: null,
        fee: null,
        ...extra,
    };
}

describe("historyStreams", () => {
    it("EVM 地址在每个网络都查，只查支持的网络，超过上限分批", () => {
        const evm = { id: "e", chain: "evm", kind: "single", network: "ethereum" } as Account;
        const btc = { id: "b", chain: "bitcoin", kind: "segwit", network: "bitcoin" } as Account;
        const rows = [
            { accountId: "e", address: EVM_ADDRESS },
            ...Array.from({ length: 150 }, (_, i) => ({ accountId: "b", address: `bc1q${i}` })),
        ] as Address[];
        const streams = historyStreams([evm, btc], rows, "mainnet", ["ethereum", "bsc", "bitcoin"]);
        expect(streams.map((stream) => [stream.network, stream.addresses.length])).toEqual([
            ["ethereum", 1],
            ["bsc", 1],
            ["bitcoin", 100],
            ["bitcoin", 50],
        ]);
    });
});

describe("mergeFeed", () => {
    it("同一笔以本地记录为准，大小写不同也算同一笔", () => {
        const rows = mergeFeed([local({ hash: "0xABC" })], [remote({ hash: "0xabc" })], own, []);
        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({ key: "TX-1", status: "已广播" });
    });

    it("链上状态的文案和分组", () => {
        const status = (value: HistoryItem["status"]) => mergeFeed([], [remote({ status: value })], own, [])[0];
        expect(status("finalized")).toMatchObject({ status: "已确认", group: "已完成", tone: "ok" });
        expect(status("included")).toMatchObject({ status: "确认中", group: "进行中" });
        expect(status("failed")).toMatchObject({ status: "链上失败", group: "未完成", tone: "danger" });
    });

    it("本地链上失败不显示成成功", () => {
        expect(mergeFeed([local({ state: "failed" })], [], own, [])[0]).toMatchObject({
            status: "链上失败",
            tone: "danger",
            group: "未完成",
            note: "合约回滚，手续费已扣除",
        });
        expect(mergeFeed([local({ state: "failed", chain: "tron" })], [], own, [])[0]?.note).toBe(
            "执行失败，手续费已扣除",
        );
    });

    it("全部生命周期状态都有文案和入口", () => {
        const states = [
            "signing",
            "signed",
            "abandoned",
            "broadcasting",
            "broadcast",
            "uncertain",
            "replaced",
            "expired",
            "confirming",
            "confirmed",
            "failed",
        ] as const;
        for (const state of states) {
            const row = mergeFeed([local({ state })], [], own, [])[0];
            expect(row?.status, state).toBeTruthy();
            expect(row?.href, state).toMatch(/^\/(sign|broadcast)\?id=/);
        }
        expect(mergeFeed([local({ state: "signing" })], [], own, [])[0]).toMatchObject({
            action: "继续",
            href: "/sign?id=TX-1",
        });
        expect(mergeFeed([local({ state: "signed" })], [], own, [])[0]).toMatchObject({
            action: "继续",
            href: "/broadcast?id=TX-1",
        });
        expect(mergeFeed([local({ state: "uncertain" })], [], own, [])[0]).toMatchObject({
            action: "处理",
            note: "重试广播同一笔",
        });
    });

    it("已取消的交易说明签名是否仍然有效", () => {
        expect(mergeFeed([local({ state: "abandoned" })], [], own, [])[0]?.note).toBe("已取消");
        expect(mergeFeed([local({ state: "abandoned", raw: "aa" })], [], own, [])[0]?.note).toBe(
            "已取消，签名仍然有效",
        );
    });

    it("零金额转入不进列表，投毒者常用这个手法", () => {
        expect(mergeFeed([], [remote({ amount: 0n, asset: USDT })], own, [])).toEqual([]);
    });

    it("伪造的零金额转出也不进列表，投毒者用 transferFrom 就能造出来", () => {
        expect(mergeFeed([], [remote({ amount: 0n, from: EVM_ADDRESS, to: PEER_EVM })], own, [])).toEqual([]);
        expect(mergeFeed([], [remote({ amount: 0n, from: null })], own, [])).toEqual([]);
    });

    it("本地自己发的零金额交易还在", () => {
        expect(mergeFeed([local({ kind: "cancel", amount: "0", to: EVM_ADDRESS })], [], own, [])).toHaveLength(1);
    });

    it("本地认为没上链但链上查到了，不能被本地记录盖住", () => {
        const rows = mergeFeed([local({ state: "replaced" })], [remote({ status: "finalized" })], own, []);
        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({ status: "已确认", tone: "ok", noteTone: "warn" });
        expect(mergeFeed([local({ state: "replaced" })], [remote({ status: "pending" })], own, [])[0]?.status).toBe(
            "已替换",
        );
    });

    it("还没最终确定的回滚显示执行失败，不显示成进行中的普通交易", () => {
        expect(mergeFeed([local({ state: "confirming", reverted: true })], [], own, [])[0]).toMatchObject({
            status: "确认中",
            tone: "danger",
            note: "执行失败，等待最终确认",
        });
    });

    it("转入转出的方向和金额符号", () => {
        expect(mergeFeed([], [remote()], own, [])[0]).toMatchObject({
            title: "转入 ETH",
            amount: "+1.0000 ETH",
            external: true,
        });
        expect(mergeFeed([], [remote({ from: EVM_ADDRESS, to: PEER_EVM })], own, [])[0]).toMatchObject({
            title: "转出 ETH",
            amount: "-1.0000 ETH",
            party: "至 0x7099…79C8",
        });
    });

    it("清单外的代币显示原始整数，不显示符号", () => {
        const fake = "0x0000000000000000000000000000000000000001";
        expect(mergeFeed([], [remote({ asset: fake, amount: 5n })], own, [])[0]).toMatchObject({
            title: "转入 未知代币",
            amount: "+5 未知代币",
        });
    });

    it("内存池里的交易排在最前面", () => {
        const rows = mergeFeed(
            [],
            [remote({ hash: "0x1" }), remote({ hash: "0x2", time: null, status: "pending" })],
            own,
            [],
        );
        expect(rows[0]?.time).toBeNull();
    });

    it("备注按网络加哈希对上，本地和链上记录都能显示", () => {
        const notes: Note[] = [{ id: "ethereum:0x1", text: "房租", updatedAt: 0 }];
        expect(mergeFeed([local()], [], own, notes)[0]?.remark).toBe("房租");
        expect(mergeFeed([], [remote()], own, notes)[0]?.remark).toBe("房租");
    });
});

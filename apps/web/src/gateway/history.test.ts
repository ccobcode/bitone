import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EVM_ADDRESS, PEER_EVM, USDT, initCore } from "../testing";
import type { FeedCursor, Note, TxRecord } from "../data/db";
import { addressKey, type HistoryItem } from "./client";
import { attentionRows, historyStreams, ledgerRows, recordHref, txHref, unsentRows, type RemoteItem } from "./history";

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
    const addresses = Array.from({ length: 150 }, (_, i) => `bc1q${i}`);

    it("没同步过的地址按单次上限分批", () => {
        const streams = historyStreams("w", [{ network: "bitcoin", addresses }], []);
        expect(streams.map((stream) => stream.addresses.length)).toEqual([100, 50]);
    });

    it("新地址另成一批,已同步的批次不动,这样水位和翻页游标都不失效", () => {
        const synced = addresses.slice(0, 2);
        const cursor = (id: string, network: string) =>
            ({ id, network, addresses: synced, checkpoint: "1", older: null }) as FeedCursor;
        const cursors = [cursor("w:bitcoin:bc1q0", "bitcoin"), cursor("other:bitcoin:x", "bitcoin")];
        const target = { network: "bitcoin", addresses: ["bc1q9", ...synced] };
        expect(historyStreams("w", [target], cursors)).toEqual([
            { network: "bitcoin", addresses: synced },
            { network: "bitcoin", addresses: ["bc1q9"] },
        ]);
    });
});

describe("ledgerRows", () => {
    it("同一笔以本地记录为准,大小写不同也算同一笔", () => {
        const rows = ledgerRows([local({ hash: "0xABC" })], [remote({ hash: "0xabc" })], own, []);
        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({ key: "TX-1", status: "已广播" });
    });

    it("链上状态的文案和进度", () => {
        const status = (value: HistoryItem["status"]) => ledgerRows([], [remote({ status: value })], own, [])[0];
        expect(status("finalized")).toMatchObject({ status: "已确认", moving: false, tone: "ok" });
        expect(status("included")).toMatchObject({ status: "确认中", moving: true });
        expect(status("failed")).toMatchObject({ status: "链上失败", moving: false, tone: "danger" });
    });

    it("本地链上失败不显示成成功", () => {
        expect(ledgerRows([local({ state: "failed" })], [], own, [])[0]).toMatchObject({
            status: "链上失败",
            tone: "danger",
            moving: false,
            note: "合约回滚,手续费已扣除",
        });
        expect(ledgerRows([local({ state: "failed", chain: "tron" })], [], own, [])[0]?.note).toBe(
            "执行失败,手续费已扣除",
        );
    });

    it("只放上过链或正在上链的交易,请求不算记录", () => {
        const listed = ["broadcasting", "broadcast", "uncertain", "confirming", "confirmed", "failed"] as const;
        for (const state of listed) {
            expect(ledgerRows([local({ state })], [], own, []), state).toHaveLength(1);
        }
        for (const state of ["signing", "signed", "abandoned", "expired", "replaced"] as const) {
            expect(ledgerRows([local({ state })], [], own, []), state).toEqual([]);
        }
    });

    it("每一行都进同一个详情页,本地和链上都不跳外链", () => {
        expect(ledgerRows([local()], [], own, [])[0]?.href).toBe("/history/ethereum/0x1");
        expect(ledgerRows([], [remote()], own, [])[0]?.href).toBe("/history/ethereum/0x1");
    });

    it("链接按交易标识规范化,十六进制不分大小写,Solana 区分", () => {
        expect(recordHref("ethereum", "0xABC")).toBe("/history/ethereum/0xabc");
        expect(recordHref("solana", "AbC")).toBe("/history/solana/AbC");
    });

    it("没上链的请求还用编号", () => {
        const href = (state: TxRecord["state"]) => txHref({ id: "TX-1", state, network: "ethereum", hash: "0x1" });
        for (const state of ["signing", "signed", "abandoned", "expired", "replaced"] as const) {
            expect(href(state), state).toBe("/history/TX-1");
        }
        expect(href("confirmed")).toBe("/history/ethereum/0x1");
        expect(txHref({ id: "TX-1", state: "broadcasting", network: "ethereum" })).toBe("/history/TX-1");
    });

    it("零金额转入不进列表,投毒者常用这个手法", () => {
        expect(ledgerRows([], [remote({ amount: 0n, asset: USDT })], own, [])).toEqual([]);
    });

    it("伪造的零金额转出也不进列表,投毒者用 transferFrom 就能造出来", () => {
        expect(ledgerRows([], [remote({ amount: 0n, from: EVM_ADDRESS, to: PEER_EVM })], own, [])).toEqual([]);
        expect(ledgerRows([], [remote({ amount: 0n, from: null })], own, [])).toEqual([]);
    });

    it("本地自己发的零金额交易还在", () => {
        expect(ledgerRows([local({ kind: "cancel", amount: "0", to: EVM_ADDRESS })], [], own, [])).toHaveLength(1);
    });

    it("本地认为没上链但链上查到了,记录里要有这笔", () => {
        const rows = ledgerRows([local({ state: "replaced" })], [remote({ status: "finalized" })], own, []);
        expect(rows).toHaveLength(1);
        expect(rows[0]).toMatchObject({ status: "已确认", tone: "ok", noteTone: "warn" });
        expect(ledgerRows([local({ state: "replaced" })], [remote({ status: "pending" })], own, [])).toEqual([]);
    });

    it("已签名还没广播的交易在链上出现了,同样不能盖住", () => {
        const rows = ledgerRows([local({ state: "signed" })], [remote({ status: "finalized" })], own, []);
        expect(rows).toHaveLength(1);
        expect(rows[0]?.noteTone).toBe("warn");
    });

    it("还没最终确定的回滚显示执行失败,不显示成进行中的普通交易", () => {
        expect(ledgerRows([local({ state: "confirming", reverted: true })], [], own, [])[0]).toMatchObject({
            status: "确认中",
            tone: "danger",
            note: "执行失败,等待最终确认",
        });
    });

    it("转入转出的方向和金额符号", () => {
        expect(ledgerRows([], [remote()], own, [])[0]).toMatchObject({
            title: "转入 ETH",
            direction: "in",
            amount: "+1.0000 ETH",
        });
        expect(ledgerRows([], [remote({ from: EVM_ADDRESS, to: PEER_EVM })], own, [])[0]).toMatchObject({
            title: "转出 ETH",
            direction: "out",
            amount: "-1.0000 ETH",
            party: "至 0x7099…79C8",
        });
    });

    it("清单外的代币显示原始整数,不显示符号", () => {
        const fake = "0x0000000000000000000000000000000000000001";
        expect(ledgerRows([], [remote({ asset: fake, amount: 5n })], own, [])[0]).toMatchObject({
            title: "转入 未知代币",
            amount: "+5 未知代币",
            entry: null,
        });
    });

    it("内存池里的交易排在最前面", () => {
        const rows = ledgerRows(
            [],
            [remote({ hash: "0x1" }), remote({ hash: "0x2", time: null, status: "pending" })],
            own,
            [],
        );
        expect(rows[0]?.time).toBeNull();
    });

    it("备注按网络加哈希对上,本地和链上记录都能显示", () => {
        const notes: Note[] = [{ id: "ethereum:0x1", text: "房租", updatedAt: 0 }];
        expect(ledgerRows([local()], [], own, notes)[0]?.remark).toBe("房租");
        expect(ledgerRows([], [remote()], own, notes)[0]?.remark).toBe("房租");
    });
});

describe("unsentRows", () => {
    it("只放没上链就结束的请求", () => {
        const states = ["abandoned", "expired", "replaced"] as const;
        for (const state of states) expect(unsentRows([local({ state })], []), state).toHaveLength(1);
        expect(unsentRows([local({ state: "signing" }), local({ state: "confirmed" })], [])).toEqual([]);
    });

    it("金额不带符号,这笔没有发生", () => {
        expect(unsentRows([local({ state: "expired" })], [])[0]?.amount).toBe("1.0000 ETH");
    });

    it("已取消的交易说明签名是否仍然有效,有效的要警告", () => {
        expect(unsentRows([local({ state: "abandoned" })], [])[0]).toMatchObject({ note: "已取消", noteTone: "muted" });
        expect(unsentRows([local({ state: "abandoned", raw: "aa" })], [])[0]).toMatchObject({
            note: "已取消,签名仍然有效",
            noteTone: "warn",
        });
    });

    it("链上查到这笔时说明本地记录已过时", () => {
        const rows = unsentRows([local({ state: "expired" })], [remote({ status: "included" })]);
        expect(rows[0]).toMatchObject({ status: "确认中", noteTone: "warn", note: "链上查到这笔,本地记录已过时" });
        expect(unsentRows([local({ state: "expired" })], [remote({ status: "pending" })])[0]?.status).toBe("已过期");
    });
});

describe("attentionRows", () => {
    const NOW = 1_800_000_500_000;

    it("待签名,已签名,结果不明都要动手,已签名不能漏", () => {
        const rows = (state: TxRecord["state"]) => attentionRows([local({ state })], 0, NOW);
        expect(rows("signing")[0]).toMatchObject({ status: "待签名", action: "继续", href: "/sign/TX-1" });
        expect(rows("signed")[0]).toMatchObject({ status: "已签名", action: "广播", href: "/sign/TX-1" });
        expect(rows("uncertain")[0]).toMatchObject({
            status: "结果不明",
            action: "处理",
            href: "/history/ethereum/0x1",
        });
    });

    it("等得太久的交易要提示加速,刚广播的不用", () => {
        const stuck = local({ broadcastAt: NOW - 2 * 60 * 60_000 });
        expect(attentionRows([stuck], 0, NOW)[0]).toMatchObject({ status: "等待时间过长", tone: "warn" });
        expect(attentionRows([local({ broadcastAt: NOW })], 0, NOW)).toEqual([]);
    });

    it("链上失败看过之后不再提示", () => {
        const failed = local({ state: "failed", updatedAt: 100 });
        expect(attentionRows([failed], 50, NOW)[0]).toMatchObject({ tone: "danger", href: "/history/ethereum/0x1" });
        expect(attentionRows([failed], 100, NOW)).toEqual([]);
    });

    it("其他状态都不打扰", () => {
        for (const state of ["confirming", "confirmed", "abandoned", "expired", "replaced"] as const) {
            expect(attentionRows([local({ state })], 0, NOW), state).toEqual([]);
        }
    });

    it("最新的排在前面", () => {
        const rows = attentionRows(
            [local({ id: "a", state: "signing", createdAt: 1 }), local({ id: "b", state: "signing", createdAt: 2 })],
            0,
            NOW,
        );
        expect(rows.map((row) => row.key)).toEqual(["b", "a"]);
    });
});

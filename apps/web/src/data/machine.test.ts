import { describe, expect, it } from "vitest";
import type { TxRecord, TxState } from "./db";
import {
    CONFLICT_ROUNDS,
    CONFLICT_SPACING,
    EXPIRY_GRACE,
    IllegalTransition,
    REPLACED_WATCH,
    STATE_LABELS,
    TRANSITIONS,
    holdsReservation,
    isOverdue,
    isWatched,
    mayStillLand,
    observe,
    transition,
    type ChainObservation,
    type TxEvent,
} from "./machine";

const NOW = 1_800_000_000_000;

function tx(state: TxState, extra: Partial<TxRecord> = {}): TxRecord {
    return {
        id: "TX-1",
        walletId: "w",
        accountId: "a",
        chain: "evm",
        network: "ethereum",
        kind: "transfer",
        state,
        asset: "native",
        amount: "1",
        from: "f",
        to: "t",
        request: new Uint8Array(1),
        requestId: "id",
        hash: "0xabc",
        plan: { chain: "evm", nonce: 1, gasLimit: 21000, maxFee: "1", maxPriority: "1" },
        createdAt: NOW,
        updatedAt: NOW,
        expiresAt: null,
        hardExpiry: false,
        timeline: [{ state, at: NOW }],
        attempts: 0,
        conflicts: 0,
        confirmations: 0,
        stage: null,
        block: null,
        fee: null,
        ...extra,
    };
}

const seen = (status: ChainObservation["status"], extra: Partial<ChainObservation> = {}): ChainObservation => ({
    hash: "0xabc",
    status,
    stage: null,
    confirmations: 0,
    block: null,
    fee: null,
    ...extra,
});

// 逐条对应 docs/03-system.md 第 7 节的状态图,最后四条是文档之外加的,见 machine.ts 开头.
const EDGES: [TxState, TxEvent, TxState][] = [
    ["signing", "signed", "signed"],
    ["signing", "cancel", "abandoned"],
    ["signing", "local-expire", "abandoned"],
    ["signed", "cancel", "abandoned"],
    ["signed", "submit", "broadcasting"],
    ["broadcasting", "accepted", "broadcast"],
    ["broadcasting", "rejected", "uncertain"],
    ["uncertain", "retry", "broadcasting"],
    ["uncertain", "seen", "broadcast"],
    ["uncertain", "replaced", "replaced"],
    ["broadcast", "included", "confirming"],
    ["broadcast", "conflict", "uncertain"],
    ["broadcast", "replaced", "replaced"],
    ["broadcast", "expired", "expired"],
    ["confirming", "reorg", "broadcast"],
    ["confirming", "finalized", "confirmed"],
    ["confirming", "reverted", "failed"],
    ["signed", "expired", "expired"],
    ["uncertain", "expired", "expired"],
    ["replaced", "included", "confirming"],
];

const STATES = Object.keys(TRANSITIONS) as TxState[];
const EVENTS: TxEvent[] = [
    "signed",
    "cancel",
    "local-expire",
    "submit",
    "accepted",
    "rejected",
    "retry",
    "seen",
    "conflict",
    "replaced",
    "expired",
    "included",
    "reorg",
    "finalized",
    "reverted",
];

describe("状态图", () => {
    it.each(EDGES)("%s 收到 %s 进入 %s", (from, event, to) => {
        const next = transition(tx(from), event, NOW + 1, "备注");
        expect(next.state).toBe(to);
        expect(next.updatedAt).toBe(NOW + 1);
        expect(next.timeline.at(-1)).toEqual({ state: to, at: NOW + 1, note: "备注" });
    });

    it("其余组合全部拒绝", () => {
        const allowed = new Set(EDGES.map(([from, event]) => `${from} ${event}`));
        let rejected = 0;
        for (const state of STATES) {
            for (const event of EVENTS) {
                if (allowed.has(`${state} ${event}`)) continue;
                expect(() => transition(tx(state), event, NOW), `${state} ${event}`).toThrow(IllegalTransition);
                rejected += 1;
            }
        }
        expect(rejected).toBe(STATES.length * EVENTS.length - EDGES.length);
    });

    it("已确认和链上失败是终态,已确认不再回退", () => {
        for (const state of ["confirmed", "failed"] as const) {
            expect(TRANSITIONS[state]).toEqual({});
            expect(observe(tx(state), seen("unknown"), { now: NOW, conflict: true })).toEqual(tx(state));
        }
    });

    it("每个状态都有文案", () => {
        for (const state of STATES) expect(STATE_LABELS[state]).toBeTruthy();
        expect(STATE_LABELS.failed).toBe("链上失败");
    });
});

describe("observe", () => {
    it("打包进入确认中", () => {
        const next = observe(
            tx("broadcast"),
            seen("included", { block: 7, stage: "latest", fee: "21000", confirmations: 1 }),
            { now: NOW },
        );
        expect(next).toMatchObject({ state: "confirming", block: 7, stage: "latest", fee: "21000", confirmations: 1 });
        expect(next.timeline.at(-1)?.note).toBe("区块 7");
    });

    it("阶段变化只更新进度", () => {
        const next = observe(tx("confirming"), seen("included", { stage: "safe", confirmations: 3 }), { now: NOW });
        expect(next).toMatchObject({ state: "confirming", stage: "safe", confirmations: 3 });
    });

    it("达到最终性才是已确认", () => {
        expect(observe(tx("confirming"), seen("finalized", { stage: "finalized" }), { now: NOW }).state).toBe(
            "confirmed",
        );
        expect(observe(tx("confirming"), seen("included", { stage: "safe" }), { now: NOW }).state).toBe("confirming");
    });

    it("离线期间走完的状态一次补齐,每一步都留在时间线", () => {
        const next = observe(tx("broadcast"), seen("finalized"), { now: NOW });
        expect(next.state).toBe("confirmed");
        expect(next.timeline.map((entry) => entry.state)).toEqual(["broadcast", "confirming", "confirmed"]);
        const unsure = observe(tx("uncertain"), seen("finalized"), { now: NOW });
        expect(unsure.timeline.map((entry) => entry.state)).toEqual([
            "uncertain",
            "broadcast",
            "confirming",
            "confirmed",
        ]);
    });

    it("合约回滚是链上失败,不能算成功", () => {
        const failed = seen("failed", { stage: "finalized", fee: "42000", block: 9 });
        for (const state of ["broadcast", "uncertain", "confirming"] as const) {
            const next = observe(tx(state), failed, { now: NOW });
            expect(next.state).toBe("failed");
            expect(next.fee).toBe("42000");
            expect(next.timeline.at(-1)?.note).toContain("手续费已扣除");
        }
    });

    it("还没到最终阶段的回滚不进终态,但不能显示成成功", () => {
        const next = observe(tx("broadcast"), seen("failed", { stage: "latest", fee: "42000" }), { now: NOW });
        expect(next).toMatchObject({ state: "confirming", reverted: true, fee: "42000" });
        expect(observe(next, seen("failed", { stage: "safe" }), { now: NOW }).state).toBe("confirming");
        expect(observe(next, seen("failed", { stage: "finalized" }), { now: NOW }).state).toBe("failed");
    });

    it("回滚被重组后重新执行成功,失败标记清除", () => {
        const failed = observe(tx("broadcast"), seen("failed", { stage: "latest" }), { now: NOW });
        const rolled = observe(failed, seen("unknown"), { now: NOW });
        expect(rolled).toMatchObject({ state: "broadcast", reverted: false });
        const again = observe(failed, seen("included", { stage: "latest" }), { now: NOW });
        expect(again).toMatchObject({ state: "confirming", reverted: false });
    });

    it("重组回退到已广播", () => {
        for (const status of ["unknown", "pending"] as const) {
            const next = observe(tx("confirming", { block: 5, stage: "latest" }), seen(status), { now: NOW });
            expect(next).toMatchObject({ state: "broadcast", block: null, stage: null });
        }
    });

    it("结果不明时节点查到就是已广播,查不到保持不变", () => {
        expect(observe(tx("uncertain"), seen("pending"), { now: NOW }).state).toBe("broadcast");
        expect(observe(tx("uncertain"), seen("unknown"), { now: NOW }).state).toBe("uncertain");
    });

    it("证据只来自网关时不算被替换,连续两轮后是结果不明,不释放占用", () => {
        for (const state of ["broadcast", "uncertain"] as const) {
            const first = observe(tx(state), seen("unknown"), { now: NOW, conflict: true });
            expect(first).toMatchObject({ state, conflicts: 1 });
            const second = observe(first, seen("unknown"), { now: NOW + CONFLICT_SPACING, conflict: true });
            expect(second.state).toBe("uncertain");
            expect(second.conflicts).toBe(CONFLICT_ROUNDS);
            expect(holdsReservation(second)).toBe(true);
        }
        const moved = observe(tx("broadcast", { conflicts: 1 }), seen("unknown"), { now: NOW, conflict: true });
        expect(moved.lastError).toContain("没有定论");
        expect(moved.timeline).toHaveLength(2);
        const again = observe(moved, seen("unknown"), { now: NOW + CONFLICT_SPACING, conflict: true });
        expect(again.timeline).toHaveLength(2);
    });

    it("挨着的两次查询不算两轮", () => {
        const first = observe(tx("broadcast"), seen("unknown"), { now: NOW, conflict: true });
        const again = observe(first, seen("unknown"), { now: NOW + 1000, conflict: true });
        expect(again).toMatchObject({ state: "broadcast", conflicts: 1 });
    });

    it("证据消失时清零,推送不带证据时计数不变", () => {
        const first = observe(tx("broadcast"), seen("unknown"), { now: NOW, conflict: true });
        expect(observe(first, seen("unknown"), { now: NOW, conflict: false }).conflicts).toBe(0);
        expect(observe(first, seen("unknown"), { now: NOW }).conflicts).toBe(1);
        expect(observe(first, seen("pending"), { now: NOW }).conflicts).toBe(0);
    });

    it("交易其实上链了就不算被替换", () => {
        const next = observe(tx("broadcast", { conflicts: 1 }), seen("included"), { now: NOW, conflict: true });
        expect(next).toMatchObject({ state: "confirming", conflicts: 0 });
    });

    it("TRON 超过过期时间,宽限之后才算已过期", () => {
        const base = { chain: "tron" as const, hardExpiry: true, expiresAt: NOW };
        expect(observe(tx("broadcast", base), seen("unknown"), { now: NOW + 1000 }).state).toBe("broadcast");
        const late = NOW + EXPIRY_GRACE + 1;
        for (const state of ["broadcast", "uncertain"] as const) {
            const first = observe(tx(state, base), seen("unknown"), { now: late });
            expect(first).toMatchObject({ state, conflicts: 1 });
            expect(observe(first, seen("unknown"), { now: late + CONFLICT_SPACING }).state).toBe("expired");
        }
        expect(observe(tx("broadcast", base), seen("pending"), { now: late }).state).toBe("broadcast");
    });

    it("EVM 和 Bitcoin 的报价到期不影响已广播的交易", () => {
        const quoted = tx("broadcast", { expiresAt: NOW, hardExpiry: false });
        expect(observe(quoted, seen("unknown"), { now: NOW + EXPIRY_GRACE * 10 }).state).toBe("broadcast");
    });

    it("没有变化时返回原对象", () => {
        const record = tx("broadcast");
        expect(observe(record, seen("pending"), { now: NOW })).toBe(record);
    });

    it("不跟踪的状态不受影响", () => {
        for (const state of ["signing", "signed", "broadcasting", "abandoned", "expired"] as const) {
            const record = tx(state);
            expect(observe(record, seen("finalized"), { now: NOW })).toBe(record);
        }
    });

    it("已替换的交易其实上链了,要如实显示", () => {
        const next = observe(tx("replaced"), seen("included", { stage: "latest", block: 7 }), { now: NOW });
        expect(next).toMatchObject({ state: "confirming", block: 7 });
        expect(next.timeline.at(-1)?.note).toContain("替换判定作废");
        expect(observe(tx("replaced"), seen("finalized"), { now: NOW }).state).toBe("confirmed");
    });

    it("已替换的交易查不到时保持不变,看够时间后不再查询", () => {
        const record = tx("replaced", { updatedAt: NOW });
        expect(observe(record, seen("unknown"), { now: NOW, conflict: true })).toBe(record);
        expect(observe(record, seen("pending"), { now: NOW })).toBe(record);
        expect(isWatched(record, NOW + REPLACED_WATCH - 1)).toBe(true);
        expect(isWatched(record, NOW + REPLACED_WATCH)).toBe(false);
        expect(isWatched(tx("broadcast"), NOW + REPLACED_WATCH)).toBe(true);
    });
});

describe("占用和超时", () => {
    it("已签名后取消不释放,签名仍然有效", () => {
        expect(holdsReservation(tx("abandoned", { raw: "aa" }))).toBe(true);
        expect(holdsReservation(tx("abandoned", { raw: undefined }))).toBe(false);
        for (const state of ["signing", "signed", "broadcasting", "broadcast", "uncertain", "confirming"] as const) {
            expect(holdsReservation(tx(state))).toBe(true);
        }
        for (const state of ["confirmed", "failed", "replaced", "expired"] as const) {
            expect(holdsReservation(tx(state))).toBe(false);
        }
    });

    it("已替换要有本地的替换交易才算定论", () => {
        expect(mayStillLand(tx("uncertain"))).toBe(true);
        expect(mayStillLand(tx("replaced"))).toBe(true);
        expect(mayStillLand(tx("replaced", { replacedBy: "TX-2" }))).toBe(false);
        expect(mayStillLand(tx("broadcast"))).toBe(false);
    });

    it("已广播超时:Bitcoin 24 小时,EVM 30 分钟,其他链不提示", () => {
        const at = (chain: TxRecord["chain"]) => tx("broadcast", { chain, broadcastAt: NOW });
        expect(isOverdue(at("evm"), NOW + 30 * 60_000)).toBe(false);
        expect(isOverdue(at("evm"), NOW + 30 * 60_000 + 1)).toBe(true);
        expect(isOverdue(at("bitcoin"), NOW + 30 * 60_000 + 1)).toBe(false);
        expect(isOverdue(at("bitcoin"), NOW + 24 * 60 * 60_000 + 1)).toBe(true);
        expect(isOverdue(at("tron"), NOW + 48 * 60 * 60_000)).toBe(false);
    });
});

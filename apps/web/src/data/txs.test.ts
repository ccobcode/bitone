import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { EVM_ADDRESS, PEER_EVM, initCore } from "../testing";
import { db, clearLocalData, type TxRecord } from "./db";
import { entry } from "./feed";
import { REPLACED_WATCH } from "./machine";
import { applyObservation, deleteRequest, findTx, isDeletable } from "./txs";

beforeAll(initCore);

const NOW = 1_800_000_000_000;

function tx(state: TxRecord["state"], extra: Partial<TxRecord> = {}): TxRecord {
    return { id: "TX-1", state, updatedAt: NOW, ...extra } as TxRecord;
}

describe("isDeletable", () => {
    it("没上链就结束、不占序号的可以删", () => {
        for (const state of ["abandoned", "expired"] as const) expect(isDeletable(tx(state), 0, NOW), state).toBe(true);
    });

    it("还占着序号的不能删，签名可能仍然有效", () => {
        expect(isDeletable(tx("abandoned", { raw: "aa" }), 1, NOW)).toBe(false);
    });

    it("序号释放后，签名有效的那笔也能删", () => {
        expect(isDeletable(tx("abandoned", { raw: "aa" }), 0, NOW)).toBe(true);
    });

    it("刚被替换的还在观察期，过了才能删", () => {
        expect(isDeletable(tx("replaced"), 0, NOW + 1)).toBe(false);
        expect(isDeletable(tx("replaced"), 0, NOW + REPLACED_WATCH)).toBe(true);
    });

    it("其他状态都不能删", () => {
        for (const state of [
            "signing",
            "signed",
            "broadcast",
            "uncertain",
            "confirming",
            "confirmed",
            "failed",
        ] as const) {
            expect(isDeletable(tx(state), 0, NOW), state).toBe(false);
        }
    });
});

describe("deleteRequest", () => {
    beforeEach(clearLocalData);

    it("删除可以删的请求", async () => {
        await db.txs.put(tx("abandoned"));
        await deleteRequest("TX-1", NOW);
        expect(await db.txs.get("TX-1")).toBeUndefined();
    });

    it("写库时再查一次，还占着序号的拒绝删除", async () => {
        await db.txs.put(tx("abandoned", { raw: "aa" }));
        await db.reservations.put({
            id: "r",
            accountId: "a",
            network: "ethereum",
            type: "nonce",
            value: "1",
            txIds: ["TX-1"],
            createdAt: NOW,
        });
        await expect(deleteRequest("TX-1", NOW)).rejects.toThrow();
        expect(await db.txs.get("TX-1")).toBeDefined();
    });
});

describe("有结果的交易存成标准记录", () => {
    beforeEach(clearLocalData);

    const moving = tx("confirming", {
        walletId: "w",
        network: "ethereum",
        chain: "evm",
        hash: "0xABC",
        asset: "native",
        amount: "5",
        from: EVM_ADDRESS,
        to: PEER_EVM,
        timeline: [],
        plan: { chain: "evm", nonce: 1, gasLimit: 21000, maxFee: "1", maxPriority: "1" },
    });
    const seen = (status: "finalized" | "failed") =>
        ({ hash: "0xABC", status, stage: "finalized", confirmations: 9, block: 1, fee: null }) as const;

    it("确认后的记录和网关同步的是同一个标识，后到的直接覆盖", async () => {
        await db.txs.put(moving);
        await applyObservation("TX-1", seen("finalized"), { now: NOW });
        const [saved] = await db.chainTxs.toArray();
        expect(saved).toMatchObject({ walletId: "w", txKey: "ethereum:0xabc", status: "finalized", amount: "5" });
        const item = { hash: "0xABC", time: 1, from: EVM_ADDRESS, to: PEER_EVM, asset: "native", amount: 5n };
        expect(saved?.id).toBe(entry("w", "s", "ethereum", { ...item, status: "finalized" }).id);
    });

    it("链上失败也存，确认中不存", async () => {
        await db.txs.put(moving);
        await applyObservation("TX-1", seen("failed"), { now: NOW });
        expect((await db.chainTxs.toArray())[0]?.status).toBe("failed");
        await clearLocalData();
        await db.txs.put({ ...moving, state: "broadcast" });
        await applyObservation("TX-1", { ...seen("finalized"), status: "included" }, { now: NOW });
        expect(await db.chainTxs.count()).toBe(0);
    });

    it("按哈希找本地交易，十六进制不分大小写", async () => {
        await db.txs.put(moving);
        expect((await findTx("ethereum", "0xabc"))?.id).toBe("TX-1");
        expect(await findTx("bsc", "0xabc")).toBeUndefined();
    });
});

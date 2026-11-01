import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
    EVM_ADDRESS,
    PATHS,
    PEER_EVM,
    draftFor,
    gatewayError,
    initCore,
    stubOffline,
    pairWallet,
    must,
    stubGateway,
    type Fixture,
    type Route,
} from "../testing";
import { clearLocalData, db, type TxRecord } from "../data/db";
import { broadcastTx, createReplacement, createTransfer } from "../data/transfer";
import { cancelTx, markSigned } from "../data/txs";
import { applyPush, pollOpenTxs } from "./tracker";

let fixture: Fixture;
const HASH = `0x${"ab".repeat(32)}`;

beforeAll(initCore);

beforeEach(async () => {
    await clearLocalData();
    fixture = await pairWallet();
});

afterEach(() => vi.unstubAllGlobals());

const status = (name: string, extra = {}) => ({
    txs: [{ hash: HASH, status: name, confirmations: 0, stage: null, block: null, fee: null, ...extra }],
});

const FEES = {
    kind: "evm",
    baseFee: "20000000000",
    slow: { maxFee: "25000000000", priority: "100000000" },
    medium: { maxFee: "30000000000", priority: "1000000000" },
    fast: { maxFee: "40000000000", priority: "2000000000" },
};

function chain(routes: { nonce?: number; tx?: Route; extra?: Record<string, Route> } = {}) {
    return stubGateway({
        "ethereum/fees": () => FEES,
        "ethereum/nonce": () => ({
            nonces: { [EVM_ADDRESS]: { latest: routes.nonce ?? 7, pending: routes.nonce ?? 7 } },
        }),
        "ethereum/estimate": () => ({ gas: "21000" }),
        "ethereum/broadcast": () => ({ hash: HASH, status: "accepted" }),
        "ethereum/tx": routes.tx ?? (() => status("pending")),
        ...routes.extra,
    });
}

const draft = () => draftFor(fixture, PATHS.evm, EVM_ADDRESS, "ethereum", PEER_EVM, 1000n);

async function sent(): Promise<TxRecord> {
    const tx = await createTransfer(draft());
    await markSigned(tx.id, { raw: "02aa", hash: HASH });
    return broadcastTx(tx.id);
}

const state = async (id: string) => (await db.txs.get(id))?.state;

describe("轮询", () => {
    it("按链上状态推进,停在最终状态", async () => {
        let reply: unknown = status("included", { stage: "latest", block: 9, fee: "21000" });
        chain({ tx: () => reply });
        const tx = await sent();
        await pollOpenTxs(["ethereum"]);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "confirming", block: 9, fee: "21000" });
        reply = status("finalized", { stage: "finalized", block: 9, fee: "21000" });
        await pollOpenTxs(["ethereum"]);
        expect(await state(tx.id)).toBe("confirmed");
    });

    it("回滚的交易显示链上失败", async () => {
        let stage = "latest";
        chain({ tx: () => status("failed", { stage, block: 9, fee: "42000" }) });
        const tx = await sent();
        await pollOpenTxs(["ethereum"]);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "confirming", reverted: true, fee: "42000" });
        stage = "finalized";
        await pollOpenTxs(["ethereum"]);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "failed", fee: "42000" });
    });

    it("已替换的交易继续查询,重组后上链要如实显示", async () => {
        chain({ tx: () => status("unknown") });
        const tx = await sent();
        chain({ nonce: 8, tx: () => status("unknown") });
        const now = Date.now();
        await pollOpenTxs(["ethereum"], now);
        await pollOpenTxs(["ethereum"], now + 15_000);
        expect(await state(tx.id)).toBe("replaced");
        chain({ nonce: 8, tx: () => status("included", { stage: "latest", block: 12 }) });
        await pollOpenTxs(["ethereum"], now + 20_000);
        expect(await state(tx.id)).toBe("confirming");
    });

    it("先取 nonce 再取状态,交易在两次查询之间上链不会被误判", async () => {
        const order: string[] = [];
        chain({ tx: () => status("included") });
        const tx = await sent();
        vi.stubGlobal(
            "fetch",
            vi.fn(async (path: string) => {
                if (path.endsWith("/nonce")) {
                    order.push("nonce");
                    return Response.json({ nonces: { [EVM_ADDRESS]: { latest: 8, pending: 8 } } });
                }
                order.push("tx");
                return Response.json(status("included"));
            }),
        );
        await pollOpenTxs(["ethereum"]);
        expect(order).toEqual(["nonce", "tx"]);
        expect(await state(tx.id)).toBe("confirming");
    });

    it("nonce 被用掉且查不到交易,连续两轮后才算已替换", async () => {
        chain({ nonce: 7, tx: () => status("unknown") });
        const tx = await sent();
        chain({ nonce: 8, tx: () => status("unknown") });
        const now = Date.now();
        await pollOpenTxs(["ethereum"], now);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "broadcast", conflicts: 1 });
        await pollOpenTxs(["ethereum"], now + 1000);
        expect(await state(tx.id)).toBe("broadcast");
        await pollOpenTxs(["ethereum"], now + 15_000);
        expect(await state(tx.id)).toBe("replaced");
        expect(await db.reservations.count()).toBe(0);
    });

    it("nonce 没变时查不到交易不算被替换", async () => {
        chain({ tx: () => status("unknown") });
        const tx = await sent();
        await pollOpenTxs(["ethereum"]);
        await pollOpenTxs(["ethereum"]);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "broadcast", conflicts: 0 });
    });

    it("结果不明时链上查到就转成已广播", async () => {
        chain({ extra: { "ethereum/broadcast": () => gatewayError("broadcast_rejected", "x", 422) } });
        const tx = await sent();
        expect(tx.state).toBe("uncertain");
        chain({ tx: () => status("pending") });
        await pollOpenTxs(["ethereum"]);
        expect(await state(tx.id)).toBe("broadcast");
    });

    it("结果不明时被替换交易抢先也能结束", async () => {
        chain({ nonce: 7, extra: { "ethereum/broadcast": () => gatewayError("broadcast_rejected", "x", 422) } });
        const tx = await sent();
        chain({ nonce: 8, tx: () => status("unknown") });
        const now = Date.now();
        await pollOpenTxs(["ethereum"], now);
        await pollOpenTxs(["ethereum"], now + 15_000);
        expect(await state(tx.id)).toBe("replaced");
    });

    it("网关断开时状态不变", async () => {
        chain();
        const tx = await sent();
        stubOffline();
        expect(await pollOpenTxs(["ethereum"])).toBe(false);
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "broadcast", conflicts: 0 });
    });

    it("只查网关支持的网络", async () => {
        const { calls } = chain();
        await sent();
        calls.length = 0;
        expect(await pollOpenTxs(["bitcoin"])).toBe(true);
        expect(calls).toEqual([]);
    });

    it("已放弃但已签名的交易,nonce 被用掉后释放占用", async () => {
        chain({ nonce: 7 });
        const tx = await createTransfer(draft());
        await markSigned(tx.id, { raw: "02aa", hash: HASH });
        await cancelTx(tx.id, "用户取消");
        expect(await db.reservations.count()).toBe(1);
        await pollOpenTxs(["ethereum"]);
        expect(await db.reservations.count()).toBe(1);
        chain({ nonce: 8 });
        const now = Date.now();
        await pollOpenTxs(["ethereum"], now);
        expect(await db.reservations.count()).toBe(1);
        await pollOpenTxs(["ethereum"], now + 15_000);
        expect(await db.reservations.count()).toBe(0);
    });

    it("已放弃的交易释放占用时,在途的替换交易继续占着", async () => {
        chain({ nonce: 7 });
        const tx = await createTransfer(draft());
        await markSigned(tx.id, { raw: "02aa", hash: HASH });
        await cancelTx(tx.id, "用户取消");
        const account = fixture.account(PATHS.evm);
        const next = await createReplacement(must(await db.txs.get(tx.id)), "cancel", {
            wallet: fixture.wallet,
            accounts: fixture.accounts,
            account,
        });
        chain({ nonce: 8 });
        const now = Date.now();
        await pollOpenTxs(["ethereum"], now);
        await pollOpenTxs(["ethereum"], now + 15_000);
        expect(await db.reservations.toArray()).toMatchObject([{ txIds: [next.id] }]);
    });

    it("替换交易和原交易都被跟踪,先上链的算数", async () => {
        chain();
        const tx = await sent();
        const next = await createReplacement(tx, "speedup", {
            wallet: fixture.wallet,
            accounts: fixture.accounts,
            account: fixture.account(PATHS.evm),
        });
        const other = `0x${"cd".repeat(32)}`;
        await markSigned(next.id, { raw: "02bb", hash: other });
        chain({ extra: { "ethereum/broadcast": () => ({ hash: other, status: "accepted" }) } });
        await broadcastTx(next.id);
        chain({
            tx: ({ hashes }) => ({
                txs: (hashes as string[]).map((hash) => ({
                    hash,
                    status: hash === other ? "included" : "unknown",
                    confirmations: 1,
                    stage: "latest",
                    block: 3,
                    fee: "1",
                })),
            }),
        });
        await pollOpenTxs(["ethereum"]);
        expect(await state(next.id)).toBe("confirming");
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "replaced", replacedBy: next.id });
    });
});

describe("推送", () => {
    it("按哈希找到交易,内容先校验", async () => {
        chain();
        const tx = await sent();
        await applyPush("ethereum", { hash: HASH, status: "bogus" });
        expect(await state(tx.id)).toBe("broadcast");
        await applyPush("ethereum", { hash: `0x${"00".repeat(32)}`, status: "finalized" });
        expect(await state(tx.id)).toBe("broadcast");
        await applyPush("ethereum", {
            hash: HASH,
            status: "included",
            stage: "latest",
            block: 4,
            confirmations: 1,
            fee: "21000",
        });
        expect(await state(tx.id)).toBe("confirming");
    });

    it("别的网络的同一个哈希不算", async () => {
        chain();
        const tx = await sent();
        await applyPush("bsc", { hash: HASH, status: "finalized" });
        expect(await state(tx.id)).toBe("broadcast");
    });
});

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { deriveAddress } from "../chains";
import { bitcoinFeeInfo, bitcoinReplacement } from "../chains/bitcoin/core";
import { needsPrevTx, spendableCoins } from "../chains/bitcoin/transfer";
import { bumpedEvmFees } from "../chains/evm/transfer";
import { solanaNonceAddress } from "../chains/solana/core";
import { NONCE_RENT, SOLANA_BLOCKHASH_WINDOW } from "../chains/solana/transfer";
import { QUOTE_LIFETIME, TransferError, reviewRequest } from "../chains/transfer";
import { TRON_DEFAULT_FEE_LIMIT, TRON_LIFETIME, tronCost, tronFeeLimit } from "../chains/tron/transfer";
import { accountKeyList, previewRequest, type Review } from "../core";
import {
    BTC_ADDRESS,
    EVM_ADDRESS,
    PATHS,
    PEER_EVM,
    SOLANA_ADDRESS,
    TRON_ADDRESS,
    USDT,
    draftFor,
    fakePrevTx,
    gatewayError,
    initCore,
    stubOffline,
    pairWallet,
    scriptFor,
    stubGateway,
    must,
    type Fixture,
    type Route,
} from "../testing";
import { clearLocalData, db, planOf } from "./db";
import { CONFLICT_SPACING } from "./machine";
import { broadcastTx, createReplacement, createSetup, createTransfer, replaceAndCancel, resendTx } from "./transfer";
import {
    applyObservation,
    cancelTx,
    confirmReplaced,
    markSigned,
    noteSpent,
    releaseAbandoned,
    sweepExpired,
    withAccountLock,
} from "./txs";

let fixture: Fixture;

const EVM_FEES = {
    kind: "evm",
    baseFee: "20000000000",
    slow: { maxFee: "25000000000", priority: "100000000" },
    medium: { maxFee: "30000000000", priority: "1000000000" },
    fast: { maxFee: "40000000000", priority: "2000000000" },
};

const evmDraft = (amount = 1000n) => draftFor(fixture, PATHS.evm, EVM_ADDRESS, "ethereum", PEER_EVM, amount);

function evmGateway(pending = 7, extra: Record<string, Route> = {}) {
    return stubGateway({
        "ethereum/fees": () => EVM_FEES,
        "ethereum/nonce": () => ({ nonces: { [EVM_ADDRESS]: { latest: pending, pending } } }),
        "ethereum/estimate": () => ({ gas: "21000" }),
        ...extra,
    });
}

const keys = () => accountKeyList(fixture.accounts);

/** 全部输出都回到本钱包.前序交易的输出也叫这个名字,嵌套得更深. */
function ownOutputsOnly(review: Review): boolean {
    const outputs = review.fields.filter((row) => row.name === "output");
    const depth = Math.min(...outputs.map((row) => row.depth));
    return outputs.filter((row) => row.depth === depth).every((row) => row.own);
}

const side = (path: string) => ({ wallet: fixture.wallet, accounts: fixture.accounts, account: fixture.account(path) });

beforeAll(initCore);

beforeEach(async () => {
    await clearLocalData();
    fixture = await pairWallet();
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
});

describe("EVM 构建", () => {
    it("写入待签名记录和 nonce 占用,请求能被核心库解析", async () => {
        evmGateway();
        const before = Date.now();
        const tx = await createTransfer(evmDraft());
        expect(tx).toMatchObject({ state: "signing", chain: "evm", network: "ethereum", to: PEER_EVM, amount: "1000" });
        expect(planOf(tx, "evm")).toMatchObject({ nonce: 7, gasLimit: 21000, maxFee: EVM_FEES.medium.maxFee });
        expect(tx.id).toMatch(/^TX-\d{8}-0001$/);
        // 30 分钟是手续费报价,不是签名的有效期.
        expect(tx.hardExpiry).toBe(false);
        expect(tx.expiresAt).toBeGreaterThanOrEqual(before + QUOTE_LIFETIME);
        const review = previewRequest(tx.request, keys());
        expect(review).toMatchObject({
            risk: "transfer",
            confirm: "tap",
            summary: { counterparty: PEER_EVM },
        });
        expect(await db.reservations.toArray()).toMatchObject([{ type: "nonce", value: "7", txIds: [tx.id] }]);
    });

    it("十笔并发构建,nonce 连续且不重复", async () => {
        evmGateway();
        const list = await Promise.all(Array.from({ length: 10 }, (_, i) => createTransfer(evmDraft(BigInt(i + 1)))));
        const nonces = list.map((tx) => planOf(tx, "evm").nonce).sort((a, b) => a - b);
        expect(nonces).toEqual([7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        expect(new Set(list.map((tx) => tx.id)).size).toBe(10);
        expect(await db.reservations.count()).toBe(10);
    });

    it("锁是按账户的,另一个账户不用排队", async () => {
        let release = () => {};
        const held = new Promise<void>((resolve) => {
            release = resolve;
        });
        const locked = withAccountLock(fixture.account(PATHS.evm).id, () => held);
        const other = await withAccountLock(fixture.account(PATHS.tron).id, async () => "done");
        expect(other).toBe("done");
        release();
        await locked;
    });

    it("同一账户的第二个构建要等锁释放后才读占用", async () => {
        evmGateway();
        const order: string[] = [];
        let release = () => {};
        const held = new Promise<void>((resolve) => {
            release = resolve;
        });
        const locked = withAccountLock(fixture.account(PATHS.evm).id, async () => {
            order.push("持锁");
            await held;
            order.push("放锁");
        });
        const building = createTransfer(evmDraft()).then((tx) => {
            order.push("构建");
            return tx;
        });
        await new Promise((resolve) => setTimeout(resolve, 20));
        release();
        await Promise.all([locked, building]);
        expect(order).toEqual(["持锁", "放锁", "构建"]);
    });

    it("取消中间一笔后,空出来的 nonce 先用掉", async () => {
        evmGateway();
        const first = await createTransfer(evmDraft());
        await createTransfer(evmDraft());
        await cancelTx(first.id, "用户取消");
        const third = await createTransfer(evmDraft());
        expect(planOf(third, "evm").nonce).toBe(7);
    });

    it("取消已签名的交易不释放,新交易不能跳过它,否则永远打包不了", async () => {
        evmGateway();
        const first = await createTransfer(evmDraft());
        await markSigned(first.id, { raw: "aa", hash: "0x1" });
        await cancelTx(first.id, "用户取消");
        expect(await db.reservations.count()).toBe(1);
        await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "nonce-held" });
        expect(await db.txs.count()).toBe(1);
    });

    it("手续费无法确认时,核验结果记在交易上", async () => {
        evmGateway();
        const tx = await createTransfer(evmDraft());
        const signed = await markSigned(tx.id, { raw: "aa", hash: "0x1", feeUnconfirmed: true });
        expect(signed.feeUnconfirmed).toBe(true);
    });

    it("发了替换交易之后,新交易接在它后面", async () => {
        evmGateway();
        const first = await createTransfer(evmDraft());
        await markSigned(first.id, { raw: "aa", hash: "0x1" });
        await cancelTx(first.id, "用户取消");
        const replacement = await createReplacement(must(await db.txs.get(first.id)), "cancel", side(PATHS.evm));
        expect(planOf(replacement, "evm").nonce).toBe(7);
        expect(planOf(await createTransfer(evmDraft()), "evm").nonce).toBe(8);
    });

    it("同样的转账结果不明时不让再发,新交易用下一个 nonce,两笔都会生效", async () => {
        evmGateway(7, { "ethereum/broadcast": () => gatewayError("broadcast_rejected", "x", 422) });
        const tx = await createTransfer(evmDraft());
        await markSigned(tx.id, { raw: "aa", hash: "0x1" });
        await broadcastTx(tx.id);
        expect((await db.txs.get(tx.id))?.state).toBe("uncertain");
        await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
        expect(planOf(await createTransfer(evmDraft(999n)), "evm").nonce).toBe(8);
    });

    describe("网关谎报序号已被用掉", () => {
        const unknown = {
            hash: "0x1",
            status: "unknown",
            stage: null,
            confirmations: 0,
            block: null,
            fee: null,
        } as const;

        async function sent() {
            evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x1", status: "accepted" }) });
            const tx = await createTransfer(evmDraft());
            await markSigned(tx.id, { raw: "aa", hash: "0x1" });
            await broadcastTx(tx.id);
            return tx;
        }

        it("占用不释放,同样的转账发不出去,nonce 不会被跳过", async () => {
            const tx = await sent();
            const now = Date.now();
            await applyObservation(tx.id, unknown, { now, conflict: true });
            await applyObservation(tx.id, unknown, { now: now + CONFLICT_SPACING, conflict: true });
            expect((await db.txs.get(tx.id))?.state).toBe("uncertain");
            expect(await db.reservations.toArray()).toMatchObject([{ value: "7", txIds: [tx.id] }]);
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
        });

        it("早先标成已替换,但替换交易不是这个浏览器发出的,同样的转账也发不出去", async () => {
            const tx = await sent();
            await db.txs.update(tx.id, { state: "replaced" });
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
            expect(planOf(await createTransfer(evmDraft(999n)), "evm").nonce).toBe(8);
        });

        it("替换交易上链也只是网关的说法,用户确认之前同样的转账发不出去", async () => {
            const tx = await sent();
            await db.txs.update(tx.id, { state: "replaced", replacedBy: "TX-2" });
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
            await confirmReplaced(tx.id);
            expect((await createTransfer(evmDraft())).state).toBe("signing");
        });

        it("这个浏览器的替换交易上链才标成已替换,原交易和它共用占用", async () => {
            const tx = await sent();
            const now = Date.now();
            await applyObservation(tx.id, unknown, { now, conflict: true });
            await applyObservation(tx.id, unknown, { now: now + CONFLICT_SPACING, conflict: true });
            const next = await createReplacement(must(await db.txs.get(tx.id)), "rebuild", side(PATHS.evm));
            await markSigned(next.id, { raw: "bb", hash: "0x2" });
            evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x2", status: "accepted" }) });
            await broadcastTx(next.id);
            const included = { ...unknown, hash: "0x2", status: "included", stage: "latest", block: 5 } as const;
            await applyObservation(next.id, included, { now });
            expect(await db.txs.get(tx.id)).toMatchObject({ state: "replaced", replacedBy: next.id });
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
        });

        it("确认过的替换在重组后又被替换时要重新确认", async () => {
            const tx = await sent();
            const now = Date.now();
            const next = await createReplacement(must(await db.txs.get(tx.id)), "rebuild", side(PATHS.evm));
            await markSigned(next.id, { raw: "bb", hash: "0x2" });
            evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x2", status: "accepted" }) });
            await broadcastTx(next.id);
            const included = { ...unknown, hash: "0x2", status: "included", stage: "latest", block: 5 } as const;
            await applyObservation(next.id, included, { now });
            await confirmReplaced(tx.id);
            expect((await db.txs.get(tx.id))?.replacedConfirmedAt).toBeTypeOf("number");
            await applyObservation(next.id, unknown, { now: now + 1 });
            await applyObservation(tx.id, { ...included, hash: "0x1" }, { now: now + 2 });
            await applyObservation(tx.id, unknown, { now: now + 3 });
            await applyObservation(next.id, included, { now: now + 4 });
            expect(await db.txs.get(tx.id)).toMatchObject({ state: "replaced", replacedBy: next.id });
            expect((await db.txs.get(tx.id))?.replacedConfirmedAt).toBeUndefined();
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "duplicate" });
        });
    });

    it("本地过期释放占用,已过期的待签名交易变成已放弃", async () => {
        evmGateway();
        const tx = await createTransfer(evmDraft());
        await sweepExpired(must(tx.expiresAt) - 1);
        expect((await db.txs.get(tx.id))?.state).toBe("signing");
        await sweepExpired(must(tx.expiresAt));
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "abandoned" });
        expect(await db.reservations.count()).toBe(0);
    });

    it("估算失败不拦截,按默认 Gas 上限生成请求", async () => {
        evmGateway(7, { "ethereum/estimate": () => gatewayError("invalid_request", "execution reverted") });
        const native = await createTransfer(evmDraft());
        expect(planOf(native, "evm").gasLimit).toBe(21_000);
        const token = await createTransfer(draftFor(fixture, PATHS.evm, EVM_ADDRESS, "ethereum", PEER_EVM, 5n, USDT));
        expect(planOf(token, "evm").gasLimit).toBe(65_000);
    });

    it("手动填写 Gas 上限时不估算", async () => {
        const { calls } = evmGateway();
        const tx = await createTransfer({ ...evmDraft(), gasLimit: 100_000n });
        expect(planOf(tx, "evm").gasLimit).toBe(100_000);
        expect(calls).not.toContain("ethereum/estimate");
    });

    it("ERC-20 的调用数据来自核心库,Gas 留余量", async () => {
        let call: Record<string, unknown> = {};
        evmGateway(7, {
            "ethereum/estimate": (body: Record<string, unknown>) => {
                call = body;
                return { gas: "50000" };
            },
        });
        const tx = await createTransfer({
            ...draftFor(fixture, PATHS.evm, EVM_ADDRESS, "ethereum", PEER_EVM, 2_500_000n, USDT),
        });
        expect(call.to).toBe(USDT);
        expect(call.data).toMatch(/^0xa9059cbb0{24}70997970c51812dc3a010c7d01b50e0d17dc79c8/);
        expect(planOf(tx, "evm").gasLimit).toBe(60_000);
        expect(previewRequest(tx.request, keys()).summary.amount).toMatchObject({ symbol: "USDT" });
    });

    it("本机预览判定禁止时照样写入,二维码交给冷钱包判断", async () => {
        evmGateway();
        const draft = { ...evmDraft(), account: { ...fixture.account(PATHS.evm), path: PATHS.tron } };
        const tx = await createTransfer(draft);
        expect(tx.state).toBe("signing");
        expect(previewRequest(tx.request, keys()).confirm).toBe("refuse");
        expect(await db.reservations.count()).toBe(1);
    });

    it("预览解析不了时是 null,不抛错", () => {
        expect(reviewRequest(new Uint8Array([1, 2, 3]), fixture.accounts)).toBeNull();
    });
});

describe("Bitcoin 构建", () => {
    const account = () => fixture.account(PATHS.segwit);
    const taproot = () => fixture.account(PATHS.taproot);

    function coins(address: string, amounts: number[]) {
        const script = scriptFor(address);
        return amounts.map((amount, i) => ({ ...fakePrevTx(script, amount, i), amount, address }));
    }

    function bitcoinGateway(list: ReturnType<typeof coins>, extra: Record<string, Route> = {}) {
        return stubGateway({
            "bitcoin/fees": () => ({ kind: "bitcoin", slow: 1, medium: 3, fast: 8 }),
            "bitcoin/utxos": ({ addresses }) => ({
                utxos: list
                    .filter((coin) => (addresses as string[]).includes(coin.address))
                    .map((coin) => ({
                        address: coin.address,
                        txid: coin.txid,
                        vout: 0,
                        amount: String(coin.amount),
                        height: 100,
                    })),
            }),
            "bitcoin/raw-txs": () => ({ txs: Object.fromEntries(list.map((coin) => [coin.txid, coin.hex])) }),
            ...extra,
        });
    }

    const draft = (amount: bigint, path = PATHS.segwit, from = BTC_ADDRESS) =>
        draftFor(fixture, path, from, "bitcoin", "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", amount);

    it("选币,取回前序交易,构建 PSBT,占用输入和找零索引", async () => {
        const list = coins(BTC_ADDRESS, [100_000]);
        const { calls } = bitcoinGateway(list);
        const tx = await createTransfer(draft(40_000n));
        const plan = planOf(tx, "bitcoin");
        expect(calls).toContain("bitcoin/raw-txs");
        expect(plan.inputs).toMatchObject([{ txid: list[0]?.txid, vout: 0, amount: 100_000, address: BTC_ADDRESS }]);
        expect(plan.changeIndex).toBe(0);
        expect(plan.changePath).toBe("m/84'/0'/0'/1/0");
        expect(plan.change + plan.fee + 40_000).toBe(100_000);
        expect(plan.vsize).toBeGreaterThan(100);
        const review = previewRequest(tx.request, keys());
        expect(review).toMatchObject({ risk: "transfer", confirm: "tap" });
        const rows = await db.reservations.toArray();
        expect(rows.map((row) => row.type).sort()).toEqual(["change", "utxo"]);
    });

    it("并发构建不会用到同一个 UTXO,第三笔余额不足", async () => {
        const list = coins(BTC_ADDRESS, [50_000, 50_000]);
        bitcoinGateway(list);
        const results = await Promise.allSettled([
            createTransfer(draft(10_000n)),
            createTransfer(draft(10_000n)),
            createTransfer(draft(10_000n)),
        ]);
        const done = results.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
        const failed = results.flatMap((result) => (result.status === "rejected" ? [result.reason] : []));
        expect(done).toHaveLength(2);
        expect(failed).toHaveLength(1);
        expect(failed[0]).toBeInstanceOf(TransferError);
        expect(failed[0]).toMatchObject({ code: "insufficient" });
        const used = done.map((tx) => planOf(tx, "bitcoin").inputs[0]?.txid);
        expect(new Set(used).size).toBe(2);
        const change = done.map((tx) => planOf(tx, "bitcoin").changeIndex);
        expect(new Set(change).size).toBe(2);
    });

    it("内存池里的 UTXO 不用,已占用的 UTXO 不用", async () => {
        const list = coins(BTC_ADDRESS, [50_000]);
        stubGateway({
            "bitcoin/utxos": () => ({
                utxos: [
                    { address: BTC_ADDRESS, txid: list[0]?.txid, vout: 0, amount: "50000", height: 0 },
                    { address: "bc1qother", txid: "a".repeat(64), vout: 0, amount: "50000", height: 10 },
                ],
            }),
        });
        expect(await spendableCoins(account(), "bitcoin")).toEqual([]);
    });

    it("网关给的前序交易和输入对不上时拒绝构建", async () => {
        const list = coins(BTC_ADDRESS, [100_000]);
        const other = fakePrevTx(scriptFor(BTC_ADDRESS), 100_000, 9);
        bitcoinGateway(list, { "bitcoin/raw-txs": () => ({ txs: { [must(list[0]).txid]: other.hex } }) });
        await expect(createTransfer(draft(40_000n))).rejects.toMatchObject({ code: "prev-tx" });
        expect(await db.txs.count()).toBe(0);
        expect(await db.reservations.count()).toBe(0);
    });

    it("网关虚报金额时拒绝构建,前序交易里的金额才算数", async () => {
        const list = coins(BTC_ADDRESS, [100_000]);
        bitcoinGateway(list.map((coin) => ({ ...coin, amount: 900_000 })));
        await expect(createTransfer(draft(40_000n))).rejects.toMatchObject({ code: "prev-tx" });
    });

    it("网关没有返回前序交易时拒绝构建", async () => {
        const list = coins(BTC_ADDRESS, [100_000]);
        bitcoinGateway(list, { "bitcoin/raw-txs": () => ({ txs: {} }) });
        await expect(createTransfer(draft(40_000n))).rejects.toMatchObject({ code: "prev-tx" });
    });

    it("全 Taproot 不取前序交易,SegWit 和混合输入都要取", async () => {
        expect(needsPrevTx([{ path: "m/86'/0'/0'/0/0" }])).toBe(false);
        expect(needsPrevTx([{ path: "m/84'/0'/0'/0/0" }])).toBe(true);
        expect(needsPrevTx([{ path: "m/86'/0'/0'/0/0" }, { path: "m/84'/0'/0'/0/0" }])).toBe(true);
        expect(needsPrevTx([{ path: "m/86'/0'/0'/0/0" }, { path: "m/49'/0'/0'/0/0" }])).toBe(true);

        const address = deriveAddress(taproot(), "mainnet", 0, 0);
        const { calls } = bitcoinGateway(coins(address, [100_000]));
        const tx = await createTransfer(draft(40_000n, PATHS.taproot, address));
        expect(calls).not.toContain("bitcoin/raw-txs");
        expect(previewRequest(tx.request, keys()).confirm).toBe("tap");
    });

    it("前序交易太大,超过二维码上限时提示减少输入,什么都不写", async () => {
        const big = fakePrevTx(scriptFor(BTC_ADDRESS), 100_000, 0, 9000);
        bitcoinGateway([{ ...big, amount: 100_000, address: BTC_ADDRESS }]);
        await expect(createTransfer(draft(40_000n))).rejects.toMatchObject({
            code: "too-large",
            message: expect.stringContaining("减少输入"),
        });
        expect(await db.txs.count()).toBe(0);
        expect(await db.reservations.count()).toBe(0);
    });

    it("金额低于粉尘线时用核心库的错误,门限按收款脚本类型算", async () => {
        bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
        await expect(createTransfer(draft(100n))).rejects.toMatchObject({
            code: "invalid",
            message: "金额太小,节点不会转发.",
        });
        expect(await db.txs.count()).toBe(0);
        // P2WPKH 的门限是 294,不是 546.
        expect((await createTransfer(draft(300n))).amount).toBe("300");
    });

    it("网关把 txid 改成大写也绕不过占用", async () => {
        const list = coins(BTC_ADDRESS, [50_000]);
        bitcoinGateway(list);
        await createTransfer(draft(10_000n));
        stubGateway({
            "bitcoin/utxos": () => ({
                utxos: [
                    {
                        address: BTC_ADDRESS,
                        txid: must(list[0]).txid.toUpperCase(),
                        vout: 0,
                        amount: "50000",
                        height: 100,
                    },
                ],
            }),
        });
        expect(await spendableCoins(account(), "bitcoin")).toEqual([]);
    });

    it("网关再怎么报输入已被花掉,已放弃交易的 UTXO 也不会被再次选中,用户确认后才放开", async () => {
        bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
        const tx = await createTransfer(draft(40_000n));
        await markSigned(tx.id, { raw: "aa", hash: "b".repeat(64) });
        await cancelTx(tx.id, "用户取消");
        const now = Date.now();
        for (let round = 0; round < 12; round++) await noteSpent(tx.id, true, now + round * CONFLICT_SPACING);
        expect(await spendableCoins(account(), "bitcoin")).toEqual([]);
        await releaseAbandoned(tx.id);
        expect(await spendableCoins(account(), "bitcoin")).toHaveLength(1);
    });

    it("找零太小时没有找零,不占用找零索引", async () => {
        bitcoinGateway(coins(BTC_ADDRESS, [40_500]));
        const tx = await createTransfer(draft(40_000n));
        const plan = planOf(tx, "bitcoin");
        expect(plan).toMatchObject({ change: 0, changeIndex: null, changePath: null, fee: 500 });
        expect(plan.vsize).toBeGreaterThan(100);
        expect(await db.reservations.count()).toBe(1);
    });

    it("已用过的找零索引跳过", async () => {
        bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
        await db.addresses.update([account().id, 1, 0], { used: true });
        expect(planOf(await createTransfer(draft(40_000n)), "bitcoin").changeIndex).toBe(1);
    });

    describe("RBF", () => {
        it("加速用同一组输入和找零索引,手续费更高,不产生新占用", async () => {
            const list = coins(BTC_ADDRESS, [100_000]);
            bitcoinGateway(list);
            const tx = await createTransfer(draft(40_000n));
            const before = await db.reservations.count();
            const next = await createReplacement(tx, "speedup", side(PATHS.segwit));
            const [old, plan] = [planOf(tx, "bitcoin"), planOf(next, "bitcoin")];
            expect(plan.inputs).toEqual(old.inputs);
            expect(plan.changeIndex).toBe(old.changeIndex);
            expect(plan.fee).toBeGreaterThan(old.fee + old.vsize - 1);
            expect(plan.fee).toBe(old.vsize * 8);
            expect(plan.change).toBe(100_000 - 40_000 - plan.fee);
            expect(next).toMatchObject({ kind: "speedup", replaces: tx.id, to: tx.to, amount: tx.amount });
            expect(await db.reservations.count()).toBe(before);
            expect((await db.reservations.toArray()).every((row) => row.txIds.length === 2)).toBe(true);
        });

        it("取消不再指定收款方,全部回到找零路径,冷钱包显示转给自己", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
            const tx = await createTransfer(draft(40_000n));
            const old = planOf(tx, "bitcoin");
            const next = await createReplacement(tx, "cancel", side(PATHS.segwit));
            const plan = planOf(next, "bitcoin");
            expect(next).toMatchObject({ kind: "cancel", amount: "0", replaces: tx.id });
            expect(next.to).not.toBe(tx.to);
            expect(next.to).toBe((await db.addresses.get([account().id, 1, must(old.changeIndex)]))?.address);
            expect(plan.inputs).toEqual(old.inputs);
            expect(plan.changePath).toBe(old.changePath);
            expect(plan.change + plan.fee).toBe(100_000);
            expect(plan.fee).toBeGreaterThan(old.fee);
            const review = previewRequest(next.request, keys());
            expect(review.summary.counterparty ?? null).toBeNull();
            expect(ownOutputsOnly(review)).toBe(true);
            expect(bitcoinFeeInfo(next.request).fee).toBe(plan.fee);
            expect(await db.reservations.count()).toBe(2);
        });

        it("原交易没有找零时,取消要分配一个找零索引并占用", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [40_500]));
            const tx = await createTransfer(draft(40_000n));
            expect(planOf(tx, "bitcoin").changePath).toBeNull();
            const next = await createReplacement(tx, "cancel", side(PATHS.segwit));
            const plan = planOf(next, "bitcoin");
            expect(plan).toMatchObject({ changeIndex: 0, changePath: "m/84'/0'/0'/1/0" });
            expect(plan.change + plan.fee).toBe(40_500);
            const rows = await db.reservations.toArray();
            expect(rows.find((row) => row.type === "change")?.txIds).toEqual([next.id]);
            expect(rows.find((row) => row.type === "utxo")?.txIds).toEqual([tx.id, next.id]);
        });

        it("分配的找零索引不和别的待处理交易冲突", async () => {
            const list = coins(BTC_ADDRESS, [40_500, 100_000]);
            bitcoinGateway(list);
            const busy = await createTransfer(draft(60_000n));
            const tx = await createTransfer(draft(40_000n));
            expect(planOf(busy, "bitcoin").changeIndex).toBe(0);
            const next = await createReplacement(tx, "cancel", side(PATHS.segwit));
            expect(planOf(next, "bitcoin").changeIndex).toBe(1);
        });

        it("取消交易再加速还是取消,不会变回付款", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
            const tx = await createTransfer(draft(40_000n));
            const cancel = await createReplacement(tx, "cancel", side(PATHS.segwit));
            const faster = await createReplacement(cancel, "speedup", side(PATHS.segwit));
            const plan = planOf(faster, "bitcoin");
            expect(faster).toMatchObject({ amount: "0", to: cancel.to });
            expect(plan.fee).toBeGreaterThan(planOf(cancel, "bitcoin").fee);
            expect(plan.change + plan.fee).toBe(100_000);
            expect(ownOutputsOnly(previewRequest(faster.request, keys()))).toBe(true);
        });

        it("输入不够付取消的手续费时拒绝", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
            const tx = await createTransfer(draft(40_000n));
            bitcoinGateway(coins(BTC_ADDRESS, [100_000]), {
                "bitcoin/fees": () => ({ kind: "bitcoin", slow: 1, medium: 3, fast: 5000 }),
            });
            await expect(createReplacement(tx, "cancel", side(PATHS.segwit))).rejects.toMatchObject({
                code: "insufficient",
            });
        });

        it("手续费由核心库算,不低于快档", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [100_000]));
            const tx = await createTransfer(draft(40_000n));
            const old = planOf(tx, "bitcoin");
            const replaced = bitcoinReplacement(100_000n, 40_000n, BigInt(old.fee), BigInt(old.vsize), 8n, "segwit");
            const next = planOf(await createReplacement(tx, "speedup", side(PATHS.segwit)), "bitcoin");
            expect(next).toMatchObject({ fee: replaced.fee, change: replaced.change });
        });

        it("找零不够付更高的手续费时拒绝", async () => {
            bitcoinGateway(coins(BTC_ADDRESS, [41_000]));
            const tx = await createTransfer(draft(40_000n));
            await expect(createReplacement(tx, "speedup", side(PATHS.segwit))).rejects.toMatchObject({
                code: "insufficient",
            });
        });
    });
});

describe("TRON 构建", () => {
    const tronGateway = (timestamp = Date.now(), extra: Record<string, Route> = {}) =>
        stubGateway({
            "tron/fees": () => ({ kind: "tron", energyPrice: 420, bandwidthPrice: 1000 }),
            "tron/estimate": () => ({ energy: "65000", bandwidth: "345", activationFee: "0" }),
            "tron/resources": () => ({ resources: { [TRON_ADDRESS]: { bandwidth: 600, energy: 0, activated: true } } }),
            "tron/ref-block": () => ({ refBlockBytes: "ab12", refBlockHash: "0011223344556677", timestamp }),
            ...extra,
        });
    const draft = (asset = "native", amount = 1_000_000n) =>
        draftFor(fixture, PATHS.tron, TRON_ADDRESS, "tron", "TEkxiTehnzSmSe2XqrBj4w32RUN966rdz8", amount, asset);

    it("过期时间从区块时间起算 30 分钟,到期后签名作废", async () => {
        const timestamp = Date.now() - 5000;
        tronGateway(timestamp);
        const tx = await createTransfer(draft());
        expect(tx.hardExpiry).toBe(true);
        expect(tx.expiresAt).toBe(timestamp + TRON_LIFETIME);
        expect(tx.expiresAt).toBe(timestamp + TRON_LIFETIME);
        expect(planOf(tx, "tron")).toMatchObject({ feeLimit: 0 });
    });

    it("TRC-20 写入 fee_limit,默认全靠燃烧再留 20%", async () => {
        tronGateway();
        const tx = await createTransfer(draft("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"));
        expect(planOf(tx, "tron").feeLimit).toBe(Number(tronFeeLimit(65_000n, 420n)));
        expect(previewRequest(tx.request, keys()).summary.amount).toMatchObject({ symbol: "USDT" });
    });

    it("估算失败不拦截,TRC-20 用默认 fee_limit", async () => {
        tronGateway(Date.now(), { "tron/estimate": () => gatewayError("upstream_unavailable") });
        const tx = await createTransfer(draft("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"));
        expect(planOf(tx, "tron").feeLimit).toBe(Number(TRON_DEFAULT_FEE_LIMIT));
        expect(planOf(await createTransfer(draft()), "tron").feeLimit).toBe(0);
    });

    it("能量,带宽和激活费分开计算", () => {
        const cost = tronCost(
            { energy: 65_000n, bandwidth: 345n, activationFee: 1_100_000n },
            { energy: 15_000n, bandwidth: 100n, activated: true },
            420n,
            1000n,
        );
        expect(cost).toEqual({
            energyBurn: 21_000_000n,
            bandwidthBurn: 345_000n,
            activationFee: 1_100_000n,
            total: 22_445_000n,
        });
        const free = tronCost(
            { energy: 0n, bandwidth: 268n, activationFee: 0n },
            { energy: 0n, bandwidth: 600n, activated: true },
            420n,
            1000n,
        );
        expect(free.total).toBe(0n);
    });

    it("引用区块的时间异常时拒绝,网关不能拉长签名有效期", async () => {
        tronGateway(Date.now() + 20 * 60 * 60_000);
        await expect(createTransfer(draft())).rejects.toMatchObject({ code: "invalid" });
        expect(await db.txs.count()).toBe(0);
    });

    it("不提供加速,取消和重建,重建是另一笔独立的交易", async () => {
        tronGateway();
        const tx = await createTransfer(draft());
        for (const mode of ["speedup", "cancel", "rebuild"] as const) {
            await expect(createReplacement(tx, mode, side(PATHS.tron))).rejects.toMatchObject({ code: "unsupported" });
        }
    });
});

describe("Solana 构建", () => {
    const nonceAccount = () => solanaNonceAddress(SOLANA_ADDRESS);
    const NONCE = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
    const PEER = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    const USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    const solanaGateway = (exists = true, extra: Record<string, Route> = {}) =>
        stubGateway({
            "solana/fees": () => ({ kind: "solana", slow: 0, medium: 1000, fast: 100000 }),
            "solana/nonce-account": () =>
                exists ? { exists: true, nonce: NONCE, lamports: "1447680" } : { exists: false },
            "solana/blockhash": () => ({ blockhash: NONCE }),
            "solana/estimate": () => ({ recipientTokenAccount: true, rent: "0" }),
            ...extra,
        });
    const draft = (asset = "native", amount = 1_000_000n) =>
        draftFor(fixture, PATHS.solana, SOLANA_ADDRESS, "solana", PEER, amount, asset);

    it("查不到收款方的代币账户时按要创建算,不拦截", async () => {
        solanaGateway(true, { "solana/estimate": () => gatewayError("upstream_unavailable") });
        const tx = await createTransfer(draft(USDC));
        expect(planOf(tx, "solana")).toMatchObject({ createAta: true, computeUnits: 200_000 });
    });

    it("没有 nonce 账户时提示先创建", async () => {
        solanaGateway(false);
        await expect(createTransfer(draft())).rejects.toMatchObject({ code: "no-nonce-account" });
    });

    it("创建 nonce 账户只有 1 分钟窗口,到期后本地作废", async () => {
        solanaGateway(false);
        const before = Date.now();
        const tx = await createSetup(draft());
        expect(tx).toMatchObject({
            kind: "create-nonce",
            hardExpiry: true,
            to: nonceAccount(),
            amount: NONCE_RENT.toString(),
        });
        expect(tx.expiresAt).toBeGreaterThanOrEqual(before + SOLANA_BLOCKHASH_WINDOW);
        expect(previewRequest(tx.request, keys()).risk).toBe("recognized");
        await sweepExpired(must(tx.expiresAt));
        expect((await db.txs.get(tx.id))?.state).toBe("abandoned");
        expect(await db.reservations.count()).toBe(0);
    });

    it("已有 nonce 账户时不重复创建", async () => {
        solanaGateway(true);
        await expect(createSetup(draft())).rejects.toMatchObject({ code: "invalid" });
    });

    it("每笔都取当前 nonce 值,第一条指令是 AdvanceNonceAccount,不过期", async () => {
        const { calls } = solanaGateway();
        const tx = await createTransfer(draft());
        expect(calls).toContain("solana/nonce-account");
        expect(planOf(tx, "solana")).toMatchObject({ nonceValue: NONCE, nonceAccount: nonceAccount() });
        expect(tx.expiresAt).toBeNull();
        expect(tx.hardExpiry).toBe(false);
        expect(previewRequest(tx.request, keys()).confirm).not.toBe("refuse");
    });

    it("nonce 值是一次性的,上一笔没结束时拒绝再构建", async () => {
        solanaGateway();
        const results = await Promise.allSettled([createTransfer(draft()), createTransfer(draft())]);
        expect(results.filter((result) => result.status === "fulfilled")).toHaveLength(1);
        const failed = results.find((result) => result.status === "rejected");
        expect((failed as PromiseRejectedResult).reason).toMatchObject({ code: "nonce-busy" });
    });

    it("网关再怎么报 nonce 已推进,已放弃的交易也一直挡着,用户确认后才放开", async () => {
        solanaGateway();
        const tx = await createTransfer(draft());
        await markSigned(tx.id, { raw: "aa", hash: "sig" });
        await cancelTx(tx.id, "用户取消");
        const now = Date.now();
        for (let round = 0; round < 12; round++) await noteSpent(tx.id, true, now + round * CONFLICT_SPACING);
        await expect(createTransfer(draft())).rejects.toMatchObject({ code: "nonce-busy" });
        await releaseAbandoned(tx.id);
        expect((await createTransfer(draft())).state).toBe("signing");
    });

    it("放弃已签名的创建请求后,blockhash 过期就释放,账户不会永远被占住", async () => {
        solanaGateway(false);
        const tx = await createSetup(draft());
        await markSigned(tx.id, { raw: "aa", hash: "sig" });
        await cancelTx(tx.id, "用户取消");
        expect(await db.reservations.count()).toBe(1);
        await sweepExpired(must(tx.expiresAt) + 1);
        expect(await db.reservations.count()).toBe(1);
        await sweepExpired(must(tx.expiresAt) + 61_000);
        expect(await db.reservations.count()).toBe(0);
    });

    it("重发同一笔不改变状态", async () => {
        const raws: unknown[] = [];
        solanaGateway(true, {
            "solana/broadcast": ({ raw }) => {
                raws.push(raw);
                return { hash: "sig", status: "accepted" };
            },
        });
        const tx = await createTransfer(draft());
        await markSigned(tx.id, { raw: "aa", hash: "sig" });
        await broadcastTx(tx.id);
        await resendTx(must(await db.txs.get(tx.id)));
        expect(raws).toEqual(["aa", "aa"]);
        expect((await db.txs.get(tx.id))?.state).toBe("broadcast");
    });

    it("收款方没有代币账户时带上创建指令和租金", async () => {
        solanaGateway(true, { "solana/estimate": () => ({ recipientTokenAccount: false, rent: "2039280" }) });
        const tx = await createTransfer(draft(USDC));
        expect(planOf(tx, "solana")).toMatchObject({ createAta: true });
    });

    it("不提供加速,取消只推进 nonce,冷钱包显示作废已签名的交易,要长按", async () => {
        solanaGateway();
        const tx = await createTransfer(draft());
        await expect(createReplacement(tx, "speedup", side(PATHS.solana))).rejects.toMatchObject({
            code: "unsupported",
        });
        const next = await createReplacement(tx, "cancel", side(PATHS.solana));
        expect(next).toMatchObject({
            kind: "cancel",
            to: SOLANA_ADDRESS,
            amount: "0",
            asset: "native",
            replaces: tx.id,
        });
        expect(planOf(next, "solana")).toMatchObject({
            nonceValue: NONCE,
            computeUnits: 0,
            priority: 0,
            createAta: false,
        });
        const review = previewRequest(next.request, keys());
        expect(review.confirm).toBe("hold");
        expect(JSON.stringify(review)).toContain("作废已签名的交易");
        expect(await db.reservations.count()).toBe(1);
    });

    it("代币转账的取消也只推进 nonce,不会创建代币账户", async () => {
        solanaGateway(true, { "solana/estimate": () => ({ recipientTokenAccount: false, rent: "2039280" }) });
        const tx = await createTransfer(draft(USDC));
        const next = await createReplacement(tx, "cancel", side(PATHS.solana));
        expect(next).toMatchObject({ asset: "native", amount: "0" });
        expect(planOf(next, "solana")).toMatchObject({ createAta: false });
    });

    it("重建还是同一笔转账,复用同一个 nonce 值", async () => {
        solanaGateway();
        const tx = await createTransfer(draft());
        const next = await createReplacement(tx, "rebuild", side(PATHS.solana));
        expect(next).toMatchObject({ kind: "rebuild", to: PEER, amount: tx.amount });
        expect(planOf(next, "solana").nonceValue).toBe(NONCE);
        expect(previewRequest(next.request, keys()).summary.counterparty).toBe(PEER);
    });

    it("取消交易重建时还是推进 nonce", async () => {
        solanaGateway();
        const tx = await createTransfer(draft());
        const cancel = await createReplacement(tx, "cancel", side(PATHS.solana));
        const again = await createReplacement(cancel, "rebuild", side(PATHS.solana));
        expect(again).toMatchObject({ kind: "cancel", amount: "0", to: SOLANA_ADDRESS });
    });
});

describe("替换和重建", () => {
    const seen = (status: "included" | "finalized" | "failed", hash: string) => ({
        hash,
        status,
        stage: "latest",
        confirmations: 1,
        block: 5,
        fee: "21000",
    });

    async function broadcasted() {
        const tx = await createTransfer(evmDraft());
        await markSigned(tx.id, { raw: "aa", hash: "0x1" });
        return tx;
    }

    it("重建复用同一 nonce,占用不增加", async () => {
        evmGateway();
        const tx = await broadcasted();
        const next = await createReplacement(must(await db.txs.get(tx.id)), "rebuild", side(PATHS.evm));
        expect(planOf(next, "evm").nonce).toBe(planOf(tx, "evm").nonce);
        expect(next).toMatchObject({ kind: "rebuild", replaces: tx.id, to: tx.to, amount: tx.amount });
        expect(await db.reservations.count()).toBe(1);
        expect((await db.reservations.toArray())[0]?.txIds).toEqual([tx.id, next.id]);
    });

    it("加速的单价至少高 10%,低于当前快档时用快档", () => {
        const fast = { maxFee: 100n, priority: 10n };
        expect(bumpedEvmFees({ maxFee: "1000", maxPriority: "100" }, fast)).toEqual({ maxFee: 1126n, priority: 113n });
        expect(bumpedEvmFees({ maxFee: "10", maxPriority: "1" }, fast)).toEqual(fast);
    });

    it("取消是同 nonce 转 0 给自己,单价提高", async () => {
        evmGateway();
        const tx = await broadcasted();
        const next = await createReplacement(must(await db.txs.get(tx.id)), "cancel", side(PATHS.evm));
        expect(next).toMatchObject({ kind: "cancel", to: EVM_ADDRESS, amount: "0", asset: "native" });
        expect(planOf(next, "evm")).toMatchObject({ nonce: planOf(tx, "evm").nonce, gasLimit: 21000 });
        expect(BigInt(planOf(next, "evm").maxFee)).toBeGreaterThan(BigInt(planOf(tx, "evm").maxFee));
    });

    it("已上链的交易不能替换", async () => {
        evmGateway(7, {
            "ethereum/broadcast": () => ({ hash: "0x1", status: "accepted" }),
        });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        await applyObservation(tx.id, seen("included", "0x1"), { now: Date.now() });
        await expect(
            createReplacement(must(await db.txs.get(tx.id)), "speedup", side(PATHS.evm)),
        ).rejects.toMatchObject({
            code: "invalid",
        });
    });

    it("放弃替换交易不影响原交易的占用", async () => {
        evmGateway();
        const tx = await broadcasted();
        const next = await createReplacement(must(await db.txs.get(tx.id)), "speedup", side(PATHS.evm));
        await cancelTx(next.id, "用户取消");
        expect((await db.reservations.toArray())[0]?.txIds).toEqual([tx.id]);
    });

    it("W4 发替换交易:原交易放弃,占用留给新请求", async () => {
        evmGateway();
        const tx = await createTransfer(evmDraft());
        const next = await replaceAndCancel(tx, side(PATHS.evm));
        expect((await db.txs.get(tx.id))?.state).toBe("abandoned");
        expect(await db.reservations.toArray()).toMatchObject([{ value: "7", txIds: [next.id] }]);
    });

    it("替换交易上链:原交易已替换,最终确定后释放占用", async () => {
        evmGateway(7, {
            "ethereum/broadcast": ({ raw }) => ({ hash: raw === "aa" ? "0x1" : "0x2", status: "accepted" }),
        });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        const next = await createReplacement(must(await db.txs.get(tx.id)), "speedup", side(PATHS.evm));
        await markSigned(next.id, { raw: "bb", hash: "0x2" });
        await broadcastTx(next.id);
        await applyObservation(next.id, seen("included", "0x2"), { now: Date.now() });
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "replaced", replacedBy: next.id });
        expect(await db.reservations.count()).toBe(1);
        await applyObservation(next.id, seen("finalized", "0x2"), { now: Date.now() });
        expect((await db.txs.get(next.id))?.state).toBe("confirmed");
        expect(await db.reservations.count()).toBe(0);
    });

    it.each(["finalized", "failed"] as const)(
        "替换交易 %s 的说法只来自网关,已放弃的交易继续占着,用户确认后才释放",
        async (status) => {
            evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x2", status: "accepted" }) });
            const tx = await broadcasted();
            await cancelTx(tx.id, "用户取消");
            const next = await createReplacement(must(await db.txs.get(tx.id)), "cancel", side(PATHS.evm));
            await markSigned(next.id, { raw: "bb", hash: "0x2" });
            await broadcastTx(next.id);
            await applyObservation(next.id, { ...seen(status, "0x2"), stage: "finalized" }, { now: Date.now() });
            expect((await db.txs.get(next.id))?.state).toBe(status === "finalized" ? "confirmed" : "failed");
            expect(await db.reservations.toArray()).toMatchObject([{ value: "7", txIds: [tx.id] }]);
            await expect(createTransfer(evmDraft())).rejects.toMatchObject({ code: "nonce-held" });
            await releaseAbandoned(tx.id);
            expect(await db.reservations.count()).toBe(0);
            expect((await createTransfer(evmDraft())).state).toBe("signing");
        },
    );

    it("原交易上链后,已签名没广播的替换请求作废,它的签名继续占着序号", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x1", status: "accepted" }) });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        const next = await createReplacement(must(await db.txs.get(tx.id)), "speedup", side(PATHS.evm));
        await markSigned(next.id, { raw: "bb", hash: "0x2" });
        await applyObservation(tx.id, { ...seen("finalized", "0x1"), stage: "finalized" }, { now: Date.now() });
        expect((await db.txs.get(next.id))?.state).toBe("abandoned");
        expect(await db.reservations.toArray()).toMatchObject([{ value: "7", txIds: [next.id] }]);
    });

    it("原交易抢先上链:还没签名的替换请求作废,已广播的标为已替换", async () => {
        evmGateway(7, {
            "ethereum/broadcast": ({ raw }) => ({ hash: raw === "aa" ? "0x1" : "0x2", status: "accepted" }),
        });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        const pending = await createReplacement(must(await db.txs.get(tx.id)), "speedup", side(PATHS.evm));
        const sent = await createReplacement(must(await db.txs.get(tx.id)), "cancel", side(PATHS.evm));
        await markSigned(sent.id, { raw: "bb", hash: "0x2" });
        await broadcastTx(sent.id);
        await applyObservation(tx.id, seen("included", "0x1"), { now: Date.now() });
        expect((await db.txs.get(pending.id))?.state).toBe("abandoned");
        expect((await db.txs.get(sent.id))?.state).toBe("replaced");
    });

    it("重组时占用不释放,确认中退回已广播后 nonce 不会被再次分配", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x1", status: "accepted" }) });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        await applyObservation(tx.id, seen("included", "0x1"), { now: Date.now() });
        await applyObservation(
            tx.id,
            { hash: "0x1", status: "unknown", stage: null, confirmations: 0, block: null, fee: null },
            { now: Date.now() },
        );
        expect((await db.txs.get(tx.id))?.state).toBe("broadcast");
        expect(await db.reservations.count()).toBe(1);
        const next = await createTransfer(evmDraft());
        expect(planOf(next, "evm").nonce).toBe(8);
    });

    it("链上失败到最终阶段才释放占用,之前重组还可能改变结果", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0x1", status: "accepted" }) });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        await applyObservation(tx.id, seen("failed", "0x1"), { now: Date.now() });
        expect(await db.txs.get(tx.id)).toMatchObject({ state: "confirming", reverted: true });
        expect(await db.reservations.count()).toBe(1);
        await applyObservation(tx.id, { ...seen("failed", "0x1"), stage: "finalized" }, { now: Date.now() });
        expect((await db.txs.get(tx.id))?.state).toBe("failed");
        expect(await db.reservations.count()).toBe(0);
    });

    it("原交易上链后重组,替换交易上链:替换交易如实显示,不会两笔都是已替换", async () => {
        evmGateway(7, {
            "ethereum/broadcast": ({ raw }) => ({ hash: raw === "aa" ? "0x1" : "0x2", status: "accepted" }),
        });
        const a = await broadcasted();
        await broadcastTx(a.id);
        const b = await createReplacement(must(await db.txs.get(a.id)), "speedup", side(PATHS.evm));
        await markSigned(b.id, { raw: "bb", hash: "0x2" });
        await broadcastTx(b.id);

        await applyObservation(a.id, seen("included", "0x1"), { now: Date.now() });
        expect((await db.txs.get(b.id))?.state).toBe("replaced");

        // 重组:原交易退回,替换交易上链.
        const gone = { hash: "0x1", status: "unknown" as const, stage: null, confirmations: 0, block: null, fee: null };
        await applyObservation(a.id, gone, { now: Date.now() });
        await applyObservation(b.id, seen("included", "0x2"), { now: Date.now() });
        expect((await db.txs.get(b.id))?.state).toBe("confirming");
        await applyObservation(b.id, seen("finalized", "0x2"), { now: Date.now() });
        expect((await db.txs.get(b.id))?.state).toBe("confirmed");
    });

    it("结果不明也能取消,节点永久拒绝时不会卡死", async () => {
        evmGateway(7, { "ethereum/broadcast": () => gatewayError("broadcast_rejected", "x", 422) });
        const tx = await broadcasted();
        await broadcastTx(tx.id);
        const next = await createReplacement(must(await db.txs.get(tx.id)), "cancel", side(PATHS.evm));
        expect(next).toMatchObject({ kind: "cancel", replaces: tx.id });
        expect(planOf(next, "evm").nonce).toBe(7);
    });
});

describe("广播", () => {
    async function signed() {
        const tx = await createTransfer(evmDraft());
        await markSigned(tx.id, { raw: "02aa", hash: "0xABC1" });
        return tx;
    }

    it("节点接收后是已广播,哈希大小写不影响比对", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0xabc1", status: "accepted" }) });
        const tx = await signed();
        expect(await broadcastTx(tx.id)).toMatchObject({ state: "broadcast", attempts: 1 });
    });

    it("被拒绝是结果不明,保留节点原文", async () => {
        evmGateway(7, { "ethereum/broadcast": () => gatewayError("broadcast_rejected", "nonce too low\u0000", 422) });
        const tx = await signed();
        expect(await broadcastTx(tx.id)).toMatchObject({ state: "uncertain", lastError: "nonce too low" });
    });

    it("连不上网关是结果不明", async () => {
        const tx = await (async () => {
            evmGateway();
            return signed();
        })();
        stubOffline();
        expect(await broadcastTx(tx.id)).toMatchObject({ state: "uncertain", lastError: "连接失败" });
    });

    it("网关返回的哈希不一致是结果不明,网关可能谎报", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0xbad", status: "accepted" }) });
        const tx = await signed();
        expect((await broadcastTx(tx.id)).state).toBe("uncertain");
    });

    it("重试广播同一笔,原始数据不变", async () => {
        const raws: unknown[] = [];
        let accept = false;
        evmGateway(7, {
            "ethereum/broadcast": ({ raw }) => {
                raws.push(raw);
                return accept ? { hash: "0xabc1", status: "accepted" } : gatewayError("upstream_unavailable", "", 503);
            },
        });
        const tx = await signed();
        await broadcastTx(tx.id);
        accept = true;
        expect(await broadcastTx(tx.id)).toMatchObject({ state: "broadcast", attempts: 2 });
        expect(raws).toEqual(["02aa", "02aa"]);
    });

    it("已广播的交易不能再次发起", async () => {
        evmGateway(7, { "ethereum/broadcast": () => ({ hash: "0xabc1", status: "accepted" }) });
        const tx = await signed();
        await broadcastTx(tx.id);
        await expect(broadcastTx(tx.id)).rejects.toThrow();
    });

    it("页面中途关闭时广播中超时转成结果不明", async () => {
        evmGateway(7, { "ethereum/broadcast": () => new Promise<never>(() => {}) });
        const tx = await signed();
        void broadcastTx(tx.id);
        await vi.waitFor(async () => expect((await db.txs.get(tx.id))?.state).toBe("broadcasting"));
        await sweepExpired(Date.now() + 61_000);
        expect((await db.txs.get(tx.id))?.state).toBe("uncertain");
    });

    it("TRON 已签名但超过过期时间后不能再广播", async () => {
        stubGateway({
            "tron/fees": () => ({ kind: "tron", energyPrice: 420, bandwidthPrice: 1000 }),
            "tron/estimate": () => ({ energy: "0", bandwidth: "268", activationFee: "0" }),
            "tron/resources": () => ({ resources: { [TRON_ADDRESS]: { bandwidth: 600, energy: 0, activated: true } } }),
            "tron/ref-block": () => ({
                refBlockBytes: "ab12",
                refBlockHash: "0011223344556677",
                timestamp: Date.now(),
            }),
        });
        const tx = await createTransfer(
            draftFor(fixture, PATHS.tron, TRON_ADDRESS, "tron", "TEkxiTehnzSmSe2XqrBj4w32RUN966rdz8", 1n),
        );
        await markSigned(tx.id, { raw: "aa", hash: "a".repeat(64) });
        await sweepExpired(must(tx.expiresAt) + 61_000);
        expect((await db.txs.get(tx.id))?.state).toBe("expired");
    });
});

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { amountText } from "../chains/view";
import { buildSolanaTransfer } from "../chains/solana/core";
import { parsePayload, type RawDraft, type RawValues } from "../chains/transfer";
import { buildTronTransfer } from "../chains/tron/core";
import { accountKeyList, previewRequest } from "../core";
import { localTitle } from "../gateway/history";
import {
    BTC_ADDRESS,
    EVM_ADDRESS,
    PATHS,
    PEER_EVM,
    SOLANA_ADDRESS,
    TRON_ADDRESS,
    USDT,
    fakePrevTx,
    gatewayError,
    initCore,
    must,
    pairWallet,
    scriptFor,
    stubGateway,
    stubOffline,
    type Fixture,
    type Route,
} from "../testing";
import { SignedTx } from "../ui/SignedTx";
import { clearLocalData, db, planOf, type TxRecord } from "./db";
import { createRaw, createReplacement } from "./transfer";
import { retryLink } from "./txview";
import { markSigned } from "./txs";

const CONTRACT = "0x5FbDB2315678afecb367f032d93F642f64180aa3";
const PEER_BTC = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const NONCE = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
const NOW = Date.now();

let fixture: Fixture;

const draft = (path: string, from: string, network: string): RawDraft => ({
    wallet: fixture.wallet,
    accounts: fixture.accounts,
    account: fixture.account(path),
    from,
    network,
});

const side = (path: string) => ({ wallet: fixture.wallet, accounts: fixture.accounts, account: fixture.account(path) });
const keys = () => accountKeyList(fixture.accounts);
const hexText = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");

/** 请求的最后一项是载荷:键 6,定长字节串. */
function payloadOf(request: Uint8Array): Uint8Array {
    for (let i = 0; i < request.length; i++) {
        if (request[i] === 6 && request[i + 1] === 0x58 && i + 3 + (request[i + 2] ?? 0) === request.length) {
            return request.slice(i + 3);
        }
    }
    throw new Error("没有载荷");
}

function evmGateway(extra: Record<string, Route> = {}) {
    const tier = { maxFee: "30000000000", priority: "1000000000" };
    return stubGateway({
        "ethereum/fees": () => ({ kind: "evm", baseFee: "20000000000", slow: tier, medium: tier, fast: tier }),
        "ethereum/nonce": () => ({ nonces: { [EVM_ADDRESS]: { latest: 7, pending: 7 } } }),
        "ethereum/estimate": () => ({ gas: "50000" }),
        ...extra,
    });
}

const evmRaw = (mode: string, values: RawValues) => createRaw(draft(PATHS.evm, EVM_ADDRESS, "ethereum"), mode, values);
const dataOf = (tx: TxRecord) => planOf(tx, "evm").call?.data;

beforeAll(initCore);

beforeEach(async () => {
    await clearLocalData();
    fixture = await pairWallet();
});

afterEach(() => {
    vi.unstubAllGlobals();
});

describe("EVM 原始请求", () => {
    it("任意合约的调用数据进入请求,占用 nonce,Gas 留余量", async () => {
        const { calls } = evmGateway();
        const tx = await evmRaw("call", { to: CONTRACT, value: "5", data: "0xdeadbeef" });
        expect(tx).toMatchObject({ kind: "raw", state: "signing", to: CONTRACT, amount: "5", asset: "native" });
        expect(planOf(tx, "evm")).toMatchObject({ nonce: 7, gasLimit: 60_000 });
        expect(hexText(tx.request)).toContain("deadbeef");
        expect(calls).toContain("ethereum/estimate");
        expect(previewRequest(tx.request, keys()).confirm).toBe("hold");
        expect(await db.reservations.toArray()).toMatchObject([{ type: "nonce", value: "7" }]);
    });

    it("部署合约不写收款方,要手动填 Gas 上限", async () => {
        evmGateway();
        await expect(evmRaw("call", { data: "6080" })).rejects.toThrow("部署合约要填写 Gas 上限.");
        await expect(evmRaw("call", { gas: "100000" })).rejects.toThrow("部署合约要填写调用数据.");
        const tx = await evmRaw("call", { data: "6080", gas: "100000" });
        expect(tx.to).toBe("");
        expect(planOf(tx, "evm").call).toMatchObject({ to: null, data: "6080" });
        expect(previewRequest(tx.request, keys())).toMatchObject({
            risk: "unknown",
            summary: { title: "无法识别" },
        });
    });

    it("授权和撤销来自核心库的调用数据,冷钱包按授权判断", async () => {
        evmGateway();
        const approve = await evmRaw("approve", { contract: USDT, spender: PEER_EVM, amount: "1000" });
        expect(dataOf(approve)).toMatch(/^095ea7b30{24}70997970c51812dc3a010c7d01b50e0d17dc79c8/);
        expect(approve).toMatchObject({ to: USDT, amount: "0" });
        expect(previewRequest(approve.request, keys()).risk).toBe("approval");
        const revoke = await evmRaw("approve", { contract: USDT, spender: PEER_EVM, amount: "0" });
        expect(dataOf(revoke)?.slice(-64)).toBe("0".repeat(64));
    });

    it("代币转账不限于内置清单", async () => {
        evmGateway();
        const tx = await evmRaw("token", { contract: CONTRACT, to: PEER_EVM, amount: "9" });
        expect(tx).toMatchObject({ to: PEER_EVM, asset: CONTRACT, amount: "9" });
        expect(planOf(tx, "evm").call?.to).toBe(CONTRACT);
    });

    it("估算失败时要手动填 Gas 上限,填了就继续", async () => {
        evmGateway({ "ethereum/estimate": () => gatewayError("invalid_request", "execution reverted") });
        await expect(evmRaw("call", { to: CONTRACT, data: "01" })).rejects.toThrow("execution reverted");
        expect(await db.txs.count()).toBe(0);
        const tx = await evmRaw("call", { to: CONTRACT, data: "01", gas: "90000" });
        expect(planOf(tx, "evm").gasLimit).toBe(90_000);
    });

    it("输入不对时不写入", async () => {
        evmGateway();
        await expect(evmRaw("call", { to: "0x12", data: "01" })).rejects.toThrow("合约或收款地址无效.");
        await expect(evmRaw("call", { to: CONTRACT, value: "1.5" })).rejects.toThrow("附带金额要填写整数.");
        expect(await db.txs.count()).toBe(0);
    });

    it("加速原样带上调用数据,取消只转 0 给自己", async () => {
        evmGateway();
        const tx = await evmRaw("call", { to: CONTRACT, value: "5", data: "deadbeef", gas: "90000" });
        const faster = await createReplacement(tx, "speedup", side(PATHS.evm));
        expect(hexText(faster.request)).toContain("deadbeef");
        expect(planOf(faster, "evm")).toMatchObject({ nonce: 7, gasLimit: 90_000, call: { data: "deadbeef" } });
        const cancel = await createReplacement(tx, "cancel", side(PATHS.evm));
        expect(hexText(cancel.request)).not.toContain("deadbeef");
        expect(planOf(cancel, "evm").call).toBeUndefined();
    });

    it("记录页和重新发起都按原始请求处理", async () => {
        evmGateway();
        const tx = await evmRaw("approve", { contract: USDT, spender: PEER_EVM, amount: "1" });
        expect(localTitle(tx)).toBe("原始请求");
        expect(amountText(tx)).toBe("原始请求");
        expect(retryLink(tx)).toBe("/raw");
    });
});

describe("Bitcoin 原始请求", () => {
    function gateway(amount: number) {
        const coin = { ...fakePrevTx(scriptFor(BTC_ADDRESS), amount, 0), amount, address: BTC_ADDRESS };
        return stubGateway({
            "bitcoin/fees": () => ({ kind: "bitcoin", slow: 1, medium: 3, fast: 8 }),
            "bitcoin/utxos": () => ({
                utxos: [{ address: BTC_ADDRESS, txid: coin.txid, vout: 0, amount: String(amount), height: 100 }],
            }),
            "bitcoin/raw-txs": () => ({ txs: { [coin.txid]: coin.hex } }),
        });
    }

    const payments = `${PEER_BTC} 10000\n\n${BTC_ADDRESS}   20000\n`;
    const btcRaw = (text: string) =>
        createRaw(draft(PATHS.segwit, BTC_ADDRESS, "bitcoin"), "payments", { payments: text });

    it("多个收款都进 PSBT,手续费按输出个数算", async () => {
        gateway(100_000);
        const tx = await btcRaw(payments);
        const plan = planOf(tx, "bitcoin");
        expect(tx).toMatchObject({ kind: "raw", amount: "30000", to: PEER_BTC });
        expect(plan.payments).toHaveLength(2);
        expect(plan.change + plan.fee + 30_000).toBe(100_000);
        expect(plan.fee / plan.vsize).toBeGreaterThanOrEqual(3);
        expect(previewRequest(tx.request, keys()).confirm).not.toBe("refuse");
        const types = (await db.reservations.toArray()).map((row) => row.type).sort();
        expect(types).toEqual(["change", "utxo"]);
    });

    it("加速保留全部收款,取消全部回到找零", async () => {
        gateway(100_000);
        const tx = await btcRaw(payments);
        const faster = await createReplacement(tx, "speedup", side(PATHS.segwit));
        expect(planOf(faster, "bitcoin").payments).toHaveLength(2);
        expect(planOf(faster, "bitcoin").fee).toBeGreaterThan(planOf(tx, "bitcoin").fee);
        expect(faster.amount).toBe("30000");
        const cancel = await createReplacement(tx, "cancel", side(PATHS.segwit));
        expect(cancel.amount).toBe("0");
    });

    it("列表写错时不写入", async () => {
        gateway(100_000);
        await expect(btcRaw("  \n")).rejects.toThrow("要填写收款列表.");
        await expect(btcRaw(`${PEER_BTC} 0`)).rejects.toThrow("金额无效.");
        await expect(btcRaw("bc1bad 1000")).rejects.toThrow("收款地址无效.");
        await expect(btcRaw(PEER_BTC)).rejects.toThrow("金额要填写整数.");
        expect(await db.txs.count()).toBe(0);
    });
});

describe("粘贴的载荷", () => {
    const tronDraft = () => draft(PATHS.tron, TRON_ADDRESS, "tron");
    const solanaDraft = () => draft(PATHS.solana, SOLANA_ADDRESS, "solana");
    const header = { id: new Uint8Array(16), network: "tron" };

    function tronPayload(expirationMs: number): Uint8Array {
        const { wallet } = fixture;
        return payloadOf(
            buildTronTransfer({
                ...header,
                fingerprint: wallet.fingerprint,
                path: PATHS.tron,
                from: TRON_ADDRESS,
                to: "TEkxiTehnzSmSe2XqrBj4w32RUN966rdz8",
                amount: "1000",
                refBlockBytes: "ab12",
                refBlockHash: "0011223344556677",
                expirationMs,
                timestampMs: NOW,
                feeLimit: 0,
            }),
        );
    }

    function solanaPayload(): Uint8Array {
        return payloadOf(
            buildSolanaTransfer({
                ...header,
                network: "solana",
                fingerprint: fixture.wallet.fingerprint,
                path: PATHS.solana,
                from: SOLANA_ADDRESS,
                to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                amount: 5n,
                decimals: 9,
                nonceValue: NONCE,
                priorityMicroLamports: 0,
                computeUnits: 0,
                createRecipientAta: false,
            }),
        );
    }

    it("十六进制和 base64 得到同样的载荷", () => {
        const bytes = new Uint8Array([0xde, 0xad, 0xbe, 0xef, 0xff]);
        const base64 = btoa(String.fromCharCode(...bytes));
        expect(parsePayload("0xDE AD be\nef ff")).toEqual(bytes);
        expect(parsePayload(base64)).toEqual(bytes);
        expect(parsePayload("")).toEqual(new Uint8Array());
        expect(() => parsePayload("不是数据")).toThrow("数据格式错误");
    });

    it("TRON 不用网关,过期时间取自 raw_data,预览给出对方地址", async () => {
        stubOffline();
        const expires = NOW + 3_600_000;
        const payload = tronPayload(expires);
        const tx = await createRaw(tronDraft(), "payload", { payload: hexText(payload) });
        expect(tx).toMatchObject({ kind: "raw", hardExpiry: true, expiresAt: expires, amount: "0" });
        expect(tx.to).toBe("TEkxiTehnzSmSe2XqrBj4w32RUN966rdz8");
        expect(payloadOf(tx.request)).toEqual(payload);
        expect(previewRequest(tx.request, keys()).risk).toBe("transfer");
    });

    it("Solana 不用网关,没有 nonce 占用,不能替它推进 nonce", async () => {
        stubOffline();
        const payload = solanaPayload();
        const tx = await createRaw(solanaDraft(), "payload", { payload: btoa(String.fromCharCode(...payload)) });
        expect(payloadOf(tx.request)).toEqual(payload);
        expect(tx).toMatchObject({ kind: "raw", hardExpiry: true });
        expect(planOf(tx, "solana").nonceValue).toBeNull();
        expect(await db.reservations.count()).toBe(0);
        await expect(createReplacement(tx, "cancel", side(PATHS.solana))).rejects.toMatchObject({ code: "invalid" });
    });

    it("本机解析不了的载荷照样生成请求,带本地期限", async () => {
        stubOffline();
        const before = Date.now();
        const tx = await createRaw(tronDraft(), "payload", { payload: "ff00ff" });
        expect(tx).toMatchObject({ state: "signing", to: "", hardExpiry: true });
        expect(tx.expiresAt).toBeGreaterThan(before);
        expect(previewRequest(tx.request, keys()).confirm).toBe("refuse");
    });

    it("空内容不写入", async () => {
        await expect(createRaw(tronDraft(), "payload", {})).rejects.toThrow("要填写待签名数据.");
        expect(await db.txs.count()).toBe(0);
    });
});

describe("导出已签名交易", () => {
    it("核验后的十六进制来自本地记录,连不上网关也能显示", async () => {
        stubOffline();
        const tx = await createRaw(draft(PATHS.tron, TRON_ADDRESS, "tron"), "payload", { payload: "ff00ff" });
        await markSigned(tx.id, { raw: "0a0b0c", hash: "abcd" });
        const saved = must(await db.txs.get(tx.id));
        const page = renderToStaticMarkup(createElement(SignedTx, { raw: must(saved.raw) }));
        expect(saved.state).toBe("signed");
        expect(page).toContain("0a0b0c");
        expect(page).toContain("复制十六进制");
    });
});

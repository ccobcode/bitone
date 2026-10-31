import { initCore, loadPairing, must } from "../testing";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { type Mode, type Pairing } from "../core";
import { clearLocalData, db } from "./db";
import { GAP_LIMIT, addressPath, ensureAddress, savePairing, selectableAccounts, walletId } from "./pairing";

let pairing: Pairing;
const allPaths = () => new Set(pairing.accounts.map((a) => a.path));
const id = walletId("73C5DA0A", "mainnet");
const testnetId = walletId("73C5DA0A", "testnet");
const EVM_PATH = "m/44'/60'/0'/0/0";
const save = (name: string, paths = allPaths(), mode: Mode = "mainnet") => savePairing(pairing, { name, paths, mode });

beforeAll(() => {
    initCore();
    pairing = loadPairing();
});

beforeEach(clearLocalData);

describe("selectableAccounts", () => {
    it("只列当前网络模式的账户,EVM 两边都有", () => {
        const paths = (mode: Mode) => selectableAccounts(pairing, mode).map((a) => a.path);
        expect(paths("mainnet")).toEqual([
            "m/84'/0'/0'",
            "m/86'/0'/0'",
            EVM_PATH,
            "m/44'/195'/0'/0/0",
            "m/44'/501'/0'/0'",
        ]);
        expect(paths("testnet")).toEqual([EVM_PATH, "m/84'/1'/0'", "m/86'/1'/0'", "m/44'/1'/0'/0/0", "m/44'/1'/0'/0'"]);
    });
});

describe("savePairing", () => {
    it("一次扫码写入主网和测试网两个钱包", async () => {
        const saved = await save("主钱包");
        expect(saved.id).toBe(id);
        expect(await db.wallets.get(id)).toMatchObject({ fingerprint: "73C5DA0A", name: "主钱包", mode: "mainnet" });
        expect(await db.wallets.get(testnetId)).toMatchObject({ name: "主钱包", mode: "testnet" });
        expect(await db.accounts.where("walletId").equals(id).count()).toBe(5);
        expect(await db.accounts.where("walletId").equals(testnetId).count()).toBe(5);
        // 每个模式两个 Bitcoin 账户各有接收和找零,另外三个账户各一个地址.
        expect(await db.addresses.count()).toBe((GAP_LIMIT * 4 + 3) * 2);
        const row = await db.addresses.get([`${id}:m/84'/0'/0'`, 0, 0]);
        expect(row).toMatchObject({ address: "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu", verified: false });
        const evm = await db.addresses.where("address").equals("0x9858EfFD232B4033E47d90003D41EC34EcaEda94").toArray();
        expect(evm.map((item) => item.accountId).sort()).toEqual([`${id}:${EVM_PATH}`, `${testnetId}:${EVM_PATH}`]);
    });

    it("账户按 scope 分,网络和地址跟着模式走", async () => {
        await save("主钱包");
        expect(await db.accounts.get(`${id}:m/84'/1'/0'`)).toBeUndefined();
        expect(await db.accounts.get(`${testnetId}:m/84'/0'/0'`)).toBeUndefined();
        expect((await db.accounts.get(`${id}:${EVM_PATH}`))?.network).toBe("ethereum");
        expect((await db.accounts.get(`${testnetId}:${EVM_PATH}`))?.network).toBe("sepolia");
        const row = await db.addresses.get([`${testnetId}:m/84'/1'/0'`, 0, 0]);
        expect(row?.address).toMatch(/^tb1q/);
    });

    it("不保存 scope,数据模型仍然主网测试网分开", async () => {
        await save("主钱包");
        expect(await db.accounts.get(`${testnetId}:${EVM_PATH}`)).not.toHaveProperty("scope");
    });

    it("返回当前模式的钱包,切换网络时留在同一个冷钱包", async () => {
        const saved = await save("主钱包", allPaths(), "testnet");
        expect(saved.id).toBe(testnetId);
        expect((await db.settings.get("wallet:mainnet"))?.value).toBe(id);
        expect((await db.settings.get("wallet:testnet"))?.value).toBe(testnetId);
    });

    it("公钥以 Uint8Array 保存", async () => {
        await save("主钱包");
        const account = await db.accounts.get(`${id}:m/44'/501'/0'/0'`);
        expect(account?.key).toBeInstanceOf(Uint8Array);
        expect(account?.key).toHaveLength(32);
    });

    it("只写入勾选的账户,没有账户的模式不建钱包", async () => {
        await save("主钱包", new Set(["m/84'/0'/0'"]));
        expect(await db.accounts.count()).toBe(1);
        expect(await db.wallets.get(testnetId)).toBeUndefined();
    });

    it("勾选 EVM 时两边都写入", async () => {
        await save("主钱包", new Set([EVM_PATH]));
        expect(await db.accounts.count()).toBe(2);
        expect(await db.addresses.count()).toBe(2);
    });

    it("钱包已存在时只补缺的账户,不覆盖名称和核对状态", async () => {
        await save("主钱包", new Set([EVM_PATH]));
        const key: [string, number, number] = [`${testnetId}:${EVM_PATH}`, 0, 0];
        await db.addresses.update(key, { verified: true });
        const before = await db.wallets.get(testnetId);
        await save("新名字");
        expect(await db.wallets.count()).toBe(2);
        expect(await db.wallets.get(testnetId)).toEqual(before);
        expect((await db.wallets.get(id))?.name).toBe("主钱包");
        expect(await db.accounts.count()).toBe(10);
        expect((await db.addresses.get(key))?.verified).toBe(true);
    });

    it("另一个模式的钱包已存在时,当前模式的钱包照常新建", async () => {
        await save("主钱包", new Set(["m/84'/0'/0'"]));
        await save("新名字");
        expect((await db.wallets.get(id))?.name).toBe("主钱包");
        expect((await db.wallets.get(testnetId))?.name).toBe("新名字");
        expect(await db.accounts.count()).toBe(10);
    });

    it("同一路径的公钥不同时保留本地账户", async () => {
        await save("主钱包");
        const accountId = `${id}:m/84'/0'/0'`;
        await db.addresses.update([accountId, 0, 0], { verified: true });
        const other = must(await db.accounts.get(`${id}:m/86'/0'/0'`)).key;
        const changed = {
            ...pairing,
            accounts: pairing.accounts.map((a) => (a.path === "m/84'/0'/0'" ? { ...a, key: other } : a)),
        };
        await savePairing(changed, { name: "主钱包", paths: allPaths(), mode: "mainnet" });
        const row = await db.addresses.get([accountId, 0, 0]);
        expect(row?.verified).toBe(true);
        expect(row?.address).toBe("bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
    });

    it("派生失败时什么都不写", async () => {
        const broken = {
            ...pairing,
            accounts: pairing.accounts.map((a) => ({ ...a, key: new Uint8Array(3) })),
        };
        await expect(savePairing(broken, { name: "主钱包", paths: allPaths(), mode: "mainnet" })).rejects.toThrow();
        expect(await db.wallets.count()).toBe(0);
    });
});

describe("ensureAddress", () => {
    it("按需派生下一个地址并保存", async () => {
        await save("主钱包");
        const account = must(await db.accounts.get(`${id}:m/84'/0'/0'`));
        const before = await db.addresses.count();
        const row = await ensureAddress(account, "mainnet", GAP_LIMIT);
        expect(row.address).toMatch(/^bc1q/);
        expect(await db.addresses.count()).toBe(before + 1);
        expect(addressPath(account, row)).toBe(`m/84'/0'/0'/0/${GAP_LIMIT}`);
        await expect(ensureAddress(account, "mainnet", 100_000)).rejects.toThrow();
    });
});

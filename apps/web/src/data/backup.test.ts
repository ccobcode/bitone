import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { accountKeyList, deriveAddress } from "../core";
import { BTC_ADDRESS, PATHS, PEER_EVM, initCore, loadPairing, must, pairWallet, type Fixture } from "../testing";
import {
    BACKUP_FORMAT,
    BACKUP_VERSION,
    BackupError,
    KDF_ITERATIONS,
    exportBackup,
    importBackup,
    passphraseLongEnough,
} from "./backup";
import { clearLocalData, db } from "./db";
import { GAP_LIMIT, savePairing, walletId } from "./pairing";

const PASS = "一二三四五六七八九十ab";
const ID = walletId("73C5DA0A", "mainnet");

let fixture: Fixture;

beforeAll(initCore);

beforeEach(async () => {
    await clearLocalData();
    fixture = await pairWallet();
    await db.contacts.add({
        id: "c1",
        name: "运营备用金",
        chain: "evm",
        mode: "mainnet",
        address: PEER_EVM,
        pending: false,
        createdAt: 1,
    });
    await db.notes.add({ id: "ethereum:0xabc", text: "房租", updatedAt: 2 });
    await db.addresses.update([fixture.account(PATHS.evm).id, 0, 0], { verified: true });
});

afterEach(() => vi.unstubAllGlobals());

async function decrypted(file: string, pass = PASS): Promise<string> {
    const { salt, iv, data, format, version, iterations } = JSON.parse(file) as Record<string, string>;
    const bytes = (text: string | undefined) => Uint8Array.from(atob(text ?? ""), (c) => c.charCodeAt(0));
    const material = await crypto.subtle.importKey("raw", new TextEncoder().encode(pass), "PBKDF2", false, [
        "deriveKey",
    ]);
    const key = await crypto.subtle.deriveKey(
        { name: "PBKDF2", hash: "SHA-256", salt: bytes(salt), iterations: KDF_ITERATIONS },
        material,
        { name: "AES-GCM", length: 256 },
        false,
        ["decrypt"],
    );
    const additionalData = new TextEncoder().encode(`${format}\n${version}\n${iterations}\n${salt}\n${iv}`);
    return new TextDecoder().decode(
        await crypto.subtle.decrypt({ name: "AES-GCM", iv: bytes(iv), additionalData }, key, bytes(data)),
    );
}

describe("导出", () => {
    it("文件带格式版本，用标准算法解得开，迭代 60 万次", async () => {
        const file = JSON.parse(await exportBackup(PASS)) as Record<string, unknown>;
        expect(file).toMatchObject({ format: BACKUP_FORMAT, version: BACKUP_VERSION, iterations: 600_000 });
        expect(atob(file.salt as string)).toHaveLength(16);
        expect(atob(file.iv as string)).toHaveLength(12);
        const plain = JSON.parse(await decrypted(JSON.stringify(file))) as { wallets: unknown[]; contacts: unknown[] };
        expect(plain.wallets).toHaveLength(2);
        expect(plain.contacts).toEqual([{ name: "运营备用金", chain: "evm", mode: "mainnet", address: PEER_EVM }]);
    });

    it("每次的盐和 IV 都不同", async () => {
        const [a, b] = [JSON.parse(await exportBackup(PASS)), JSON.parse(await exportBackup(PASS))];
        expect(a.salt).not.toBe(b.salt);
        expect(a.iv).not.toBe(b.iv);
    });

    it("密文里看不到地址和名称", async () => {
        const file = await exportBackup(PASS);
        expect(file).not.toContain(PEER_EVM);
        expect(file).not.toContain("运营备用金");
    });

    it("不含网关地址、设置和交易", async () => {
        await db.settings.put({ key: "gateway", value: "https://gw.example" });
        const plain = await decrypted(await exportBackup(PASS));
        expect(plain).not.toMatch(/gateway|gw\.example|settings|http/i);
        expect(Object.keys(JSON.parse(plain) as object).sort()).toEqual(["accounts", "contacts", "notes", "wallets"]);
    });

    it("不含核对状态", async () => {
        expect(await decrypted(await exportBackup(PASS))).not.toContain("verified");
    });

    it("口令至少 12 个字符，按字符计算", async () => {
        expect(passphraseLongEnough("12345678901")).toBe(false);
        expect(passphraseLongEnough("123456789012")).toBe(true);
        expect(passphraseLongEnough("😀😀😀😀😀😀😀😀😀😀😀")).toBe(false);
        await expect(exportBackup("short")).rejects.toBeInstanceOf(BackupError);
    });
});

describe("导入", () => {
    it("错误口令解不开，什么都不写", async () => {
        const file = await exportBackup(PASS);
        await clearLocalData();
        await expect(importBackup(file, "错误的口令错误的口令错误的口令")).rejects.toThrow("口令错误");
        expect(await db.wallets.count()).toBe(0);
    });

    it("全新浏览器：钱包恢复，账户未核对，联系人待确认，备注恢复", async () => {
        const file = await exportBackup(PASS);
        await clearLocalData();
        const summary = await importBackup(file, PASS);
        expect(summary).toMatchObject({ wallets: 2, accounts: 10, conflicts: 0, contacts: 1, notes: 1 });
        expect(summary.first?.id).toBe(ID);
        expect(await db.contacts.toArray()).toMatchObject([{ name: "运营备用金", address: PEER_EVM, pending: true }]);
        expect((await db.notes.toArray())[0]?.text).toBe("房租");
        const rows = await db.addresses.toArray();
        expect(rows).toHaveLength((GAP_LIMIT * 4 + 3) * 2);
        expect(rows.every((row) => !row.verified)).toBe(true);
        expect(rows.find((row) => row.address === BTC_ADDRESS)).toBeTruthy();
    });

    it("已有账户的核对状态也清掉", async () => {
        await importBackup(await exportBackup(PASS), PASS);
        expect((await db.addresses.get([fixture.account(PATHS.evm).id, 0, 0]))?.verified).toBe(false);
    });

    it("已有的联系人保持原样，备份不能改名或改状态", async () => {
        const file = await exportBackup(PASS);
        await db.contacts.update("c1", { name: "本地的名字" });
        const summary = await importBackup(file, PASS);
        expect(summary.contacts).toBe(0);
        expect(await db.contacts.toArray()).toMatchObject([{ id: "c1", name: "本地的名字", pending: false }]);
    });

    it("与本地公钥不同的账户保留本地的，恶意备份换不掉已配对的账户", async () => {
        const pairing = loadPairing();
        const other = pairing.accounts.find((a) => a.path === PATHS.taproot)?.key;
        const attacker = {
            ...pairing,
            accounts: pairing.accounts.map((a) => (a.path === PATHS.segwit ? { ...a, key: must(other) } : a)),
        };
        await clearLocalData();
        await savePairing(attacker, {
            name: "攻击者",
            paths: new Set(attacker.accounts.map((a) => a.path)),
            mode: "mainnet",
        });
        const file = await exportBackup(PASS);
        await clearLocalData();
        await pairWallet();
        const summary = await importBackup(file, PASS);
        expect(summary.conflicts).toBe(1);
        expect((await db.addresses.get([`${ID}:${PATHS.segwit}`, 0, 0]))?.address).toBe(BTC_ADDRESS);
        expect((await db.wallets.get(ID))?.name).toBe("主钱包");
    });

    it("恶意备份不能给已配对的钱包新增账户，只能由冷钱包重新配对", async () => {
        const file = await exportBackup(PASS);
        await clearLocalData();
        const pairing = loadPairing();
        await savePairing(pairing, { name: "主钱包", paths: new Set([PATHS.evm]), mode: "mainnet" });
        const before = await db.addresses.count();
        const summary = await importBackup(file, PASS);
        expect(summary.conflicts).toBeGreaterThan(0);
        expect(await db.accounts.get(`${ID}:${PATHS.segwit}`)).toBeUndefined();
        expect(await db.addresses.count()).toBe(before);
    });

    it("地址从公钥重新派生，与备份里写的无关", async () => {
        const file = await exportBackup(PASS);
        await clearLocalData();
        await importBackup(file, PASS);
        const account = must(await db.accounts.get(`${ID}:${PATHS.segwit}`));
        expect((await db.addresses.get([account.id, 0, 1]))?.address).toBe(deriveAddress(account, "mainnet", 0, 1));
        expect(accountKeyList([account])).toHaveLength(1);
    });

    it("不是备份文件、版本不对、迭代次数被改都拒绝", async () => {
        const file = JSON.parse(await exportBackup(PASS)) as Record<string, unknown>;
        await expect(importBackup("{}", PASS)).rejects.toThrow("不是备份文件");
        await expect(importBackup("<html>", PASS)).rejects.toThrow("不是备份文件");
        await expect(importBackup(JSON.stringify({ ...file, version: 2 }), PASS)).rejects.toThrow("版本");
        await expect(importBackup(JSON.stringify({ ...file, iterations: 1e12 }), PASS)).rejects.toThrow("参数");
    });

    it("文件被改动时解不开", async () => {
        const file = JSON.parse(await exportBackup(PASS)) as { data: string; salt: string };
        const data = `${file.data.slice(0, 8)}AAAA${file.data.slice(12)}`;
        await expect(importBackup(JSON.stringify({ ...file, data }), PASS)).rejects.toThrow("口令错误");
    });
});

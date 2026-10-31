import { Dexie } from "dexie";
import { describe, expect, it } from "vitest";
import { BitOneDb } from "./db";

describe("表结构升级", () => {
    it("备注的键统一成小写哈希", async () => {
        const name = "bitone";
        await new BitOneDb().delete();
        const hash = `0x${"AB".repeat(32)}`;

        const old = new Dexie(name);
        old.version(2).stores({ notes: "id" });
        await old.table("notes").add({ id: `ethereum:${hash}`, text: "房租", updatedAt: 1 });
        await old.table("notes").add({ id: "solana:5VERv8NMvzbJMEkV", text: "工资", updatedAt: 1 });
        old.close();

        const db = new BitOneDb();
        expect((await db.notes.get(`ethereum:${hash.toLowerCase()}`))?.text).toBe("房租");
        expect((await db.notes.get("solana:5VERv8NMvzbJMEkV"))?.text).toBe("工资");
        expect(await db.notes.count()).toBe(2);
        db.close();
    });

    it("M1 的数据升级后还在,新索引可用", async () => {
        const name = "bitone";
        await new BitOneDb().delete();

        const old = new Dexie(name);
        old.version(1).stores({
            wallets: "id, mode",
            accounts: "id, walletId",
            addresses: "[accountId+change+index], accountId, address",
            settings: "key",
            txs: "id",
            reservations: "id",
            contacts: "id",
        });
        await old.table("wallets").add({ id: "w", fingerprint: "F", name: "主钱包", mode: "mainnet", pairedAt: 1 });
        await old.table("settings").add({ key: "theme", value: "dark" });
        old.close();

        const db = new BitOneDb();
        expect((await db.wallets.get("w"))?.name).toBe("主钱包");
        expect((await db.settings.get("theme"))?.value).toBe("dark");
        expect(await db.txs.where("state").equals("signing").count()).toBe(0);
        expect(await db.reservations.where("txIds").equals("x").count()).toBe(0);
        expect(await db.notes.count()).toBe(0);
        db.close();
    });
});

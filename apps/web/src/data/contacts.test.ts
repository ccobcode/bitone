import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { BTC_ADDRESS, EVM_ADDRESS, PEER_EVM, initCore } from "../testing";
import { checkRecipient, confirmContact, removeContact, saveContact, similarContact, usableContacts } from "./contacts";
import { clearLocalData, db, type Contact } from "./db";

const LOOKALIKE = "0x7099aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadc79C8";

const contact = (extra: Partial<Contact> = {}): Contact => ({
    id: "c1",
    name: "运营备用金",
    chain: "evm",
    mode: "mainnet",
    address: PEER_EVM,
    pending: false,
    createdAt: 0,
    ...extra,
});

beforeAll(initCore);
beforeEach(clearLocalData);
afterEach(() => vi.unstubAllGlobals());

describe("地址簿", () => {
    it("保存规范写法，名称去掉不可见字符", async () => {
        const saved = await saveContact({
            name: " 运营‮备用金 ",
            chain: "evm",
            mode: "mainnet",
            address: PEER_EVM.toLowerCase(),
        });
        expect(saved).toMatchObject({ name: "运营备用金", address: PEER_EVM, pending: false });
    });

    it("拒绝不属于这条链的地址和空名称", async () => {
        await expect(saveContact({ name: "a", chain: "evm", mode: "mainnet", address: BTC_ADDRESS })).rejects.toThrow(
            "地址格式错误",
        );
        await expect(saveContact({ name: "  ", chain: "evm", mode: "mainnet", address: PEER_EVM })).rejects.toThrow(
            "名称",
        );
    });

    it("同一地址不重复，大小写不同也算同一个", async () => {
        await saveContact({ name: "a", chain: "evm", mode: "mainnet", address: PEER_EVM });
        await expect(
            saveContact({ name: "b", chain: "evm", mode: "mainnet", address: PEER_EVM.toLowerCase() }),
        ).rejects.toThrow("已在地址簿");
    });

    it("编辑保留确认状态，待确认的联系人改名后仍然待确认", async () => {
        await db.contacts.add(contact({ pending: true }));
        const saved = await saveContact({ id: "c1", name: "新名字", chain: "evm", mode: "mainnet", address: PEER_EVM });
        expect(saved).toMatchObject({ id: "c1", name: "新名字", pending: true });
    });

    it("确认和删除", async () => {
        await db.contacts.add(contact({ pending: true }));
        expect(usableContacts(await db.contacts.toArray())).toEqual([]);
        await confirmContact("c1");
        expect(usableContacts(await db.contacts.toArray())).toHaveLength(1);
        await removeContact("c1");
        expect(await db.contacts.count()).toBe(0);
    });
});

describe("地址簿的相似地址", () => {
    it("保存和确认时能查到首尾相同的已确认联系人", () => {
        expect(similarContact(LOOKALIKE, [contact()])?.name).toBe("运营备用金");
        expect(similarContact(PEER_EVM, [contact()])).toBeNull();
        expect(similarContact(LOOKALIKE, [contact({ pending: true })])).toBeNull();
    });
});

describe("收款地址检查", () => {
    const confirmed = (to: string) => [{ to, state: "confirmed" as const }];

    it("地址簿里的已确认联系人", () => {
        expect(checkRecipient(PEER_EVM, [contact()], [], []).contact?.name).toBe("运营备用金");
    });

    it("待确认的联系人不算联系人，要单独提示", () => {
        const check = checkRecipient(PEER_EVM, [contact({ pending: true })], [], []);
        expect(check.contact).toBeNull();
        expect(check.pending?.name).toBe("运营备用金");
    });

    it("本地确认上链过的地址才不算首次转账", () => {
        expect(checkRecipient(PEER_EVM, [], [], []).firstTime).toBe(true);
        expect(checkRecipient(PEER_EVM, [], confirmed(PEER_EVM), []).firstTime).toBe(false);
        expect(checkRecipient(PEER_EVM.toLowerCase(), [], confirmed(PEER_EVM), []).firstTime).toBe(false);
    });

    it("没有成功的转账不算转账过", () => {
        for (const state of ["signing", "failed", "broadcast", "abandoned", "replaced"] as const) {
            expect(checkRecipient(PEER_EVM, [], [{ to: PEER_EVM, state }], []).firstTime).toBe(true);
        }
    });

    it("与联系人首尾相同但中间不同时警告", () => {
        const check = checkRecipient(LOOKALIKE, [contact()], [], []);
        expect(check.similar).toEqual({ name: "「运营备用金」", address: PEER_EVM });
        expect(check.contact).toBeNull();
    });

    it("与转账记录和本钱包地址相似时也警告", () => {
        expect(checkRecipient(LOOKALIKE, [], confirmed(PEER_EVM), []).similar?.name).toBe("转账记录");
        expect(checkRecipient(LOOKALIKE, [], [], [PEER_EVM]).similar?.name).toBe("本钱包地址");
    });

    it("待确认的联系人不作为比对依据，同一个地址不算相似", () => {
        expect(checkRecipient(LOOKALIKE, [contact({ pending: true })], [], []).similar).toBeNull();
        expect(checkRecipient(PEER_EVM, [contact()], [], []).similar).toBeNull();
        expect(checkRecipient(PEER_EVM.toLowerCase(), [contact()], [], []).similar).toBeNull();
        expect(checkRecipient(EVM_ADDRESS, [contact()], [], []).similar).toBeNull();
    });
});

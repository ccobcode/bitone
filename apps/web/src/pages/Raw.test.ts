import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { clearLocalData, db } from "../data/db";
import { initCore, pairWallet } from "../testing";
import Raw from "./Raw";

const app = vi.hoisted(() => ({ current: {} }));
vi.mock("../data/app", () => ({ useApp: () => app.current }));

const page = () => renderToStaticMarkup(createElement(MemoryRouter, null, createElement(Raw)));

describe("原始请求页", () => {
    beforeAll(initCore);

    it.each([
        ["evm", ["合约或收款地址", "调用数据", "授权或撤销", "Gas 上限"]],
        ["bitcoin", ["收款列表"]],
        ["tron", ["raw_data"]],
        ["solana", ["消息字节"]],
    ])("%s 的表单带警告和生成按钮", async (chain, labels) => {
        await clearLocalData();
        const { wallet, accounts } = await pairWallet();
        const addresses = await db.addresses.toArray();
        app.current = { wallet, accounts: accounts.filter((row) => row.chain === chain), addresses, mode: "mainnet" };
        const html = page();
        for (const label of [...labels, "签名后无法撤回", "生成二维码"]) expect(html).toContain(label);
    });

    it("没有钱包时不显示表单", () => {
        app.current = { wallet: null, accounts: [], addresses: [], mode: "mainnet" };
        expect(page()).not.toContain("生成二维码");
    });
});

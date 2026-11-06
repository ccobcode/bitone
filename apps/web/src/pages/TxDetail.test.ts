import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeAll, describe, expect, it, vi } from "vitest";
import type { TxRecord } from "../data/db";
import { EVM_ADDRESS, PEER_EVM, initCore } from "../testing";
import TxDetail from "./TxDetail";

// 第一次查询取交易,按替换交易编号查的取替换交易,其余的取这笔占着的序号数.
const live = vi.hoisted(() => ({ tx: null as unknown, held: 0, replacement: null as { id: string } | null }));
vi.mock("../data/app", () => ({
    useApp: () => ({ wallet: null, accounts: [] }),
    useLive: (_query: unknown, deps: unknown[]) => {
        if (deps.length !== 1) return { tx: live.tx };
        return live.replacement && deps[0] === live.replacement.id ? live.replacement : live.held;
    },
}));
vi.mock("../gateway/provider", () => ({ useGateway: () => ({ socket: "live", down: false }) }));

beforeAll(initCore);

const NOW = 1_800_000_000_000;

const abandoned = (extra: Partial<TxRecord>) =>
    ({
        id: "TX-1",
        state: "abandoned",
        chain: "evm",
        network: "ethereum",
        kind: "transfer",
        asset: "native",
        amount: "1000",
        from: EVM_ADDRESS,
        to: PEER_EVM,
        raw: "02aa",
        hash: `0x${"ab".repeat(32)}`,
        plan: { chain: "evm", nonce: 7, gasLimit: 21000, maxFee: "1", maxPriority: "1" },
        timeline: [{ state: "abandoned", at: NOW }],
        conflicts: 0,
        confirmations: 0,
        stage: null,
        block: null,
        fee: null,
        ...extra,
    }) as TxRecord;

function page(tx: TxRecord, held: number, replacement: TxRecord | null = null): string {
    live.tx = tx;
    live.held = held;
    live.replacement = replacement;
    const route = createElement(Route, { path: "history/:id", element: createElement(TxDetail) });
    return renderToStaticMarkup(
        createElement(MemoryRouter, { initialEntries: [`/history/${tx.id}`] }, createElement(Routes, null, route)),
    );
}

describe("已放弃但已签名的交易", () => {
    it("显示网关报告的证据和释放入口", () => {
        const html = page(abandoned({ conflicts: 3, conflictAt: NOW }), 1);
        expect(html).toContain("网关报告已用掉");
        expect(html).toContain("连续报告轮数");
        expect(html).toContain("释放序号");
    });

    it("网关没有报告时如实显示,释放由用户决定", () => {
        const html = page(abandoned({}), 1);
        expect(html).toContain("网关没有报告已用掉");
        expect(html).not.toContain("连续报告轮数");
        expect(html).toContain("释放序号");
    });

    it("释放之后不再显示入口,也不能再发替换交易", () => {
        const html = page(abandoned({ conflicts: 3, conflictAt: NOW }), 0);
        expect(html).toContain("序号不再占用");
        expect(html).not.toContain("释放序号");
        expect(html).not.toContain("发替换交易");
    });
});

describe("网关报告替换交易已上链", () => {
    const replacement = abandoned({ id: "TX-2", state: "confirming", confirmations: 3, hash: `0x${"cd".repeat(32)}` });
    const replaced = (extra: Partial<TxRecord>) =>
        abandoned({ state: "replaced", replacedBy: "TX-2", timeline: [{ state: "replaced", at: NOW }], ...extra });

    it("确认之前显示证据和确认入口,不说这笔不会再上链", () => {
        const html = page(replaced({}), 0, replacement);
        expect(html).toContain("替换只有网关的说法");
        expect(html).toContain("TX-2");
        expect(html).toContain("确认中");
        expect(html).toContain("确认已被替换");
        expect(html).not.toContain("这笔不会再上链");
        expect(html).not.toContain("删除");
    });

    it("没有本地的替换交易时如实显示", () => {
        const html = page(replaced({ replacedBy: undefined }), 0);
        expect(html).toContain("没有本地记录");
        expect(html).toContain("确认已被替换");
    });

    it("用户确认之后按已替换显示,不再有确认入口", () => {
        const html = page(replaced({ replacedConfirmedAt: NOW }), 0, replacement);
        expect(html).toContain("这笔不会再上链");
        expect(html).not.toContain("确认已被替换");
    });
});

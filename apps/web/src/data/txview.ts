// 交易详情的显示文字,W4,W5 共用.各链的差别在 `chains/` 里.
import { chainOf } from "../chains";
import { amountText, nativeText, type Finality, type TxFact } from "../chains/view";
import { networkName } from "../core";
import { shortAddress } from "../core/text";
import type { TxRecord, TxState } from "./db";
import { STATE_LABELS } from "./machine";
import { t, translate } from "../core/i18n";

export { amountText, nativeText };
export type { Finality, TxFact };

/** 手续费上限或实际手续费的摘要,W4 的摘要区和核对列表用. */
export function feeFact(tx: TxRecord): TxFact {
    return chainOf(tx.chain).view.fee(tx);
}

/** 这笔的序号,W4 摘要和结果不明时要显示. */
export function sequenceFact(tx: TxRecord): TxFact {
    return chainOf(tx.chain).view.sequence(tx);
}

/** W4 收到签名后自动核对的项目,与 docs/03-system.md 第 8 节一致. */
export function verifyChecks(tx: TxRecord): TxFact[] {
    const common: TxFact = { label: t("请求 ID"), value: tx.requestId.slice(0, 8), mono: true };
    const signer: TxFact = { label: t("签名者"), value: shortAddress(tx.from), mono: true };
    return chainOf(tx.chain).view.checks(tx, common, signer);
}

/** W5 的交易详情. */
export function detailFacts(tx: TxRecord): TxFact[] {
    const rows: TxFact[] = [
        { label: t("网络"), value: networkName(tx.network) },
        { label: t("发送方"), value: tx.from, mono: true },
        { label: t("接收方"), value: tx.to, mono: true },
        { label: t("金额"), value: amountText(tx) },
    ];
    if (tx.asset !== "native") rows.push({ label: t("代币合约"), value: tx.asset, mono: true });
    rows.push(...chainOf(tx.chain).view.details(tx));
    return rows;
}

/** 最终性进度,标准见 docs/03-system.md 第 9 节. */
export function finality(tx: TxRecord): Finality {
    return chainOf(tx.chain).view.finality(tx);
}

const STEP_LABELS: Partial<Record<TxState, string>> = {
    signing: t("已构建"),
    signed: t("冷钱包已签名"),
    confirming: t("已打包"),
};

export interface TimelineStep {
    title: string;
    time: number | null;
    note: string | null;
    phase: "done" | "current" | "todo";
}

/** W5 的进度,走过的步骤来自时间线,后面还要走的按原型列出来. */
export function timelineSteps(tx: TxRecord): TimelineStep[] {
    const done = tx.timeline.map((entry, i): TimelineStep => ({
        title: STEP_LABELS[entry.state] ?? STATE_LABELS[entry.state],
        time: entry.at,
        note: entry.note ? translate(entry.note) : entry.state === "signing" ? tx.id : null,
        phase: i === tx.timeline.length - 1 ? "current" : "done",
    }));
    const todo = (titles: string[]): TimelineStep[] =>
        titles.map((title) => ({ title, time: null, note: null, phase: "todo" }));
    switch (tx.state) {
        case "signing":
            return [...done, ...todo([t("冷钱包已签名"), t("已广播"), t("已打包"), t("确认中"), t("已确认")])];
        case "signed":
            return [...done, ...todo([t("已广播"), t("已打包"), t("确认中"), t("已确认")])];
        case "broadcasting":
        case "broadcast":
        case "uncertain":
            return [...done, ...todo([t("已打包"), t("确认中"), t("已确认")])];
        case "confirming":
            return [
                ...done.map((step) => ({ ...step, phase: "done" as const })),
                { title: t("确认中"), time: null, note: finality(tx).goal, phase: "current" },
                ...todo([t("已确认")]),
            ];
        default:
            return done;
    }
}

/** 重新发起的入口.取消和创建 nonce 账户这两种不是用户的转账,不能预填内容. */
export function retryLink(tx: Pick<TxRecord, "id" | "kind" | "accountId" | "network" | "asset">): string {
    if (tx.kind === "cancel" || tx.kind === "create-nonce") {
        return `/send?account=${encodeURIComponent(tx.accountId)}&network=${encodeURIComponent(tx.network)}&asset=native`;
    }
    return `/send?from=${encodeURIComponent(tx.id)}`;
}

/** 取消时说明签名是否仍然有效,见 docs/03-system.md 第 9 节. */
export function cancelNote(tx: TxRecord): string {
    return chainOf(tx.chain).view.cancelNote(tx);
}

/** 这笔能不能发替换交易,按钮文字按链不同. */
export function replaceLabel(tx: TxRecord): string | null {
    return chainOf(tx.chain).view.replaceLabel(tx);
}

/** W4 倒计时的含义. */
export function countdownLabel(tx: TxRecord): string {
    return chainOf(tx.chain).view.countdownLabel(tx);
}

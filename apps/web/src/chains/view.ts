// 交易详情里各链共用的部分.金额和手续费都从构建记录里取.
import { formatUnits } from "../core/amount";
import { t } from "../core/i18n";
import { formatAsset, lookupAsset } from "../core/manifest";
import type { TxRecord } from "../data/db";

export interface TxFact {
    label: string;
    value: string;
    mono?: boolean;
}

export interface Finality {
    labels: string[];
    reached: number;
    count: boolean;
    goal: string;
    progress: string;
}

/** 链上失败的说明.原因不止一种时用它. */
export const EXECUTION_FAILED = "执行失败,手续费已扣除";

export function nativeText(tx: Pick<TxRecord, "network">, raw: bigint): string {
    const native = lookupAsset(tx.network, "native");
    return native ? `${formatUnits(raw, native.decimals)} ${native.symbol}` : raw.toString();
}

export function amountText(tx: TxRecord): string {
    if (tx.kind === "raw" && tx.amount === "0") return t("原始请求");
    return formatAsset(tx.network, tx.asset, BigInt(tx.amount));
}

export function finalityState(tx: Pick<TxRecord, "state">): { done: boolean; packed: boolean } {
    const done = tx.state === "confirmed";
    return { done, packed: tx.state === "confirming" || tx.state === "failed" || done };
}

/** 按阶段显示的最终性,标准见 docs/03-system.md 第 9 节. */
export function stageFinality(tx: TxRecord, labels: string[], stages: string[], goal: string): Finality {
    const { done, packed } = finalityState(tx);
    const index = stages.indexOf(tx.stage ?? "");
    const reached = done ? labels.length : packed ? Math.max(index + 1, 1) : 0;
    return {
        labels,
        reached,
        count: false,
        goal,
        progress: reached > 0 ? (labels[reached - 1] ?? "") : t("等待打包"),
    };
}

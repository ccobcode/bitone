import { t } from "../../core/i18n";
import { planOf, type TxRecord } from "../../data/db";
import type { ChainView } from "../spec";
import { finalityState, nativeText, type Finality, type TxFact } from "../view";

function fee(tx: TxRecord): TxFact {
    return { label: t("手续费"), value: nativeText(tx, BigInt(planOf(tx, "bitcoin").fee)) };
}

function finality(tx: TxRecord): Finality {
    const reached = finalityState(tx).done ? 6 : Math.min(tx.confirmations, 6);
    return {
        labels: ["1", "2", "3", "4", "5", "6"],
        reached,
        count: true,
        goal: t("达到 6 个确认后完成"),
        progress: `${reached}/6`,
    };
}

export const view: ChainView = {
    fee,
    sequence(tx) {
        const { changeIndex } = planOf(tx, "bitcoin");
        return { label: t("找零索引"), value: changeIndex === null ? t("无找零") : String(changeIndex) };
    },
    checks(tx, common) {
        const plan = planOf(tx, "bitcoin");
        return [
            common,
            { label: t("输入"), value: t("{0} 个", plan.inputs.length) },
            { label: t("输出"), value: t("{0} 个", plan.changePath && tx.kind !== "cancel" ? 2 : 1) },
            { label: t("找零路径"), value: plan.changePath ?? t("无找零"), mono: true },
        ];
    },
    details(tx) {
        const plan = planOf(tx, "bitcoin");
        return [
            { label: t("输入"), value: t("{0} 个", plan.inputs.length) },
            { label: t("找零"), value: plan.changePath ?? t("无找零"), mono: true },
            { label: t("手续费"), value: nativeText(tx, BigInt(plan.fee)) },
            { label: t("费率"), value: `${plan.feeRate} sat/vB` },
            { label: "RBF", value: t("开启") },
        ];
    },
    finality,
    cancelNote: () => t("如果已在冷钱包签名,这笔交易仍然有效,发同一组输入的替换交易才算作废."),
    replaceLabel: () => t("发替换交易"),
    countdownLabel: () => t("手续费报价还剩"),
    uncertainBody: () => t("这笔可能已经上链,先重试广播同一笔,需要重建时必须复用同一组输入."),
    failedBody: () => t("交易执行失败,手续费已扣除,资产没有转出."),
};

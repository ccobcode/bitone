import { t } from "../../core/i18n";
import { shortAddress } from "../../core/text";
import { planOf, type TxRecord } from "../../data/db";
import type { ChainView } from "../spec";
import { nativeText, stageFinality, type TxFact } from "../view";
import { SOLANA_BASE_FEE } from "./transfer";

function priorityFee(price: number, units: number): bigint {
    return (BigInt(price) * BigInt(units) + 999_999n) / 1_000_000n;
}

function fee(tx: TxRecord): TxFact {
    const plan = planOf(tx, "solana");
    return {
        label: t("手续费"),
        value: nativeText(tx, SOLANA_BASE_FEE + priorityFee(plan.priority, plan.computeUnits)),
    };
}

export const view: ChainView = {
    fee,
    sequence: (tx) => ({
        label: t("nonce 账户"),
        value: shortAddress(planOf(tx, "solana").nonceAccount),
        mono: true,
    }),
    checks: (tx, common, signer) => [
        common,
        signer,
        { label: t("消息字节"), value: t("完全一致") },
        { label: t("nonce 账户"), value: shortAddress(planOf(tx, "solana").nonceAccount), mono: true },
    ],
    details(tx) {
        const plan = planOf(tx, "solana");
        return [
            { label: t("nonce 账户"), value: plan.nonceAccount, mono: true },
            { label: t("基础费"), value: nativeText(tx, SOLANA_BASE_FEE) },
            {
                label: t("优先费"),
                value: nativeText(tx, priorityFee(plan.priority, plan.computeUnits)),
            },
        ];
    },
    finality: (tx) =>
        stageFinality(
            tx,
            [t("已处理"), t("已投票"), t("最终")],
            ["processed", "confirmed", "finalized"],
            t("到达 finalized 提交等级后完成"),
        ),
    cancelNote: (tx) =>
        tx.kind === "create-nonce"
            ? t("如果已在冷钱包签名,这笔交易在 blockhash 过期前仍然有效,过期后自动作废.")
            : t("如果已在冷钱包签名,这笔交易仍然有效,推进 nonce 才算作废."),
    // 创建 nonce 账户的 blockhash 自己会过期.
    replaceLabel: (tx) => (tx.kind === "create-nonce" ? null : t("推进 nonce")),
    countdownLabel: (tx) => (tx.kind === "create-nonce" ? t("blockhash 窗口还剩") : t("签名不过期,直到 nonce 推进")),
    uncertainBody: () => t("这笔可能已经上链,先重试广播同一笔,需要重建时必须复用同一个 nonce 值."),
    failedBody: () => t("交易执行失败,手续费已扣除,资产没有转出."),
};

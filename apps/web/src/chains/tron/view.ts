import { t } from "../../core/i18n";
import { planOf, type TxRecord } from "../../data/db";
import type { ChainView } from "../spec";
import { nativeText, stageFinality, type TxFact } from "../view";

function fee(tx: TxRecord): TxFact {
    return { label: "fee_limit", value: nativeText(tx, BigInt(planOf(tx, "tron").feeLimit)) };
}

export const view: ChainView = {
    fee,
    sequence: () => ({ label: t("过期时间"), value: t("30 分钟") }),
    checks: (tx, common, signer) => [
        common,
        signer,
        { label: t("raw_data 字节"), value: t("完全一致") },
        {
            label: t("合约类型"),
            value: tx.asset === "native" ? "TransferContract" : "TriggerSmartContract",
        },
        { label: t("过期时间"), value: t("30 分钟") },
        fee(tx),
    ],
    details: (tx) => [
        { label: t("过期时间"), value: t("30 分钟") },
        { label: "fee_limit", value: nativeText(tx, BigInt(planOf(tx, "tron").feeLimit)) },
        {
            label: t("实际手续费"),
            value: tx.fee ? nativeText(tx, BigInt(tx.fee)) : t("打包后显示"),
        },
    ],
    finality: (tx) => stageFinality(tx, [t("已打包"), t("区块确认")], ["included", "finalized"], t("区块确认后完成")),
    cancelNote: () => t("如果已在冷钱包签名,这笔交易在过期前仍然有效,无法主动作废."),
    replaceLabel: () => null,
    countdownLabel: () => t("签名过期还剩"),
    uncertainBody: () => t("这笔可能已经上链,只能重试广播同一笔,超过过期时间后才能重新发起."),
    failedBody: () => t("交易执行失败,手续费已扣除,资产没有转出."),
};

import { networkList } from "../../core";
import { t } from "../../core/i18n";
import { shortAddress } from "../../core/text";
import { planOf, type TxRecord } from "../../data/db";
import type { ChainView } from "../spec";
import { amountText, nativeText, stageFinality, type TxFact } from "../view";

function fee(tx: TxRecord): TxFact {
    const plan = planOf(tx, "evm");
    return {
        label: t("手续费上限"),
        value: nativeText(tx, BigInt(plan.gasLimit) * BigInt(plan.maxFee)),
    };
}

function chainIdOf(tx: TxRecord): string {
    return String(networkList().find((item) => item.id === tx.network)?.chainId ?? "—");
}

export const view: ChainView = {
    fee,
    sequence: (tx) => ({ label: "Nonce", value: String(planOf(tx, "evm").nonce) }),
    checks(tx, common, signer) {
        const plan = planOf(tx, "evm");
        return [
            common,
            signer,
            { label: t("收款地址"), value: shortAddress(tx.to), mono: true },
            { label: t("金额和数据"), value: amountText(tx) },
            { label: "Nonce", value: String(plan.nonce) },
            { label: "Gas Limit", value: plan.gasLimit.toLocaleString("en-US") },
            fee(tx),
            { label: "Chain ID", value: chainIdOf(tx) },
        ];
    },
    details(tx) {
        const plan = planOf(tx, "evm");
        return [
            { label: "Chain ID", value: chainIdOf(tx) },
            { label: "Nonce", value: String(plan.nonce) },
            { label: t("Gas 上限"), value: plan.gasLimit.toLocaleString("en-US") },
            fee(tx),
            {
                label: t("实际手续费"),
                value: tx.fee ? nativeText(tx, BigInt(tx.fee)) : t("打包后显示"),
            },
        ];
    },
    finality: (tx) =>
        stageFinality(
            tx,
            [t("已打包"), t("安全"), t("最终")],
            ["latest", "safe", "finalized"],
            t("到达 finalized 区块后完成"),
        ),
    cancelNote: () => t("如果已在冷钱包签名,这笔交易仍然有效,发同 nonce 的替换交易才算作废."),
    replaceLabel: () => t("发替换交易"),
    countdownLabel: () => t("手续费报价还剩"),
    uncertainBody: (sequence) => t("这笔可能已经上链,先重试广播同一笔,需要重建时必须复用 nonce {0}.", sequence.value),
    failedBody: () => t("合约回滚,手续费已扣除,资产没有转出."),
};

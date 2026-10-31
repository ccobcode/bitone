// Solana 的报价:基础费加优先费,收款方没有代币账户时还有创建租金.
import { t } from "../../core/i18n";
import { shortAddress } from "../../core/text";
import { defineQuote, errorText, settle, type QuoteInput, type QuotePlan } from "../quote";
import { nativeText } from "../view";
import { estimateSolana, fetchNonceAccount } from "./api";
import { solanaNonceAddress } from "./core";
import { SOLANA_BASE_FEE, solanaComputeUnits } from "./transfer";

export function quote(input: QuoteInput): QuotePlan {
    const { network, from, to, asset } = input;
    const nonceAccount = solanaNonceAddress(from);
    return defineQuote(
        {
            nonceInfo: {
                key: ["chain", network, "nonce-account", nonceAccount],
                run: () => fetchNonceAccount(network, nonceAccount),
                retry: 1,
            },
            estimate: {
                key: ["chain", network, "estimate", to, asset],
                run: () => estimateSolana(network, to ?? "", asset),
                enabled: asset !== "native" && to !== null,
                retry: false,
            },
        },
        ({ nonceInfo, estimate: estimated }, base, feeData) => {
            const token = asset !== "native";
            const estimate = estimated.data;
            const createAta = token && estimate !== undefined && !estimate.recipientTokenAccount;
            const units = BigInt(solanaComputeUnits(token, createAta));
            const priority = feeData?.kind === "solana" ? (feeData.medium * units + 999_999n) / 1_000_000n : null;
            const rent = estimate?.rent ?? 0n;
            const fee = priority === null ? null : SOLANA_BASE_FEE + priority + rent;
            const missing = nonceInfo.data?.exists === false;
            const busy = input.reservations.some((row) => row.type === "solana-nonce");
            return settle(
                {
                    ...base,
                    failed: base.failed || nonceInfo.isError,
                    feeTag: t("基础费加优先费"),
                    feeText: fee === null ? "—" : nativeText({ network }, fee),
                    rows: [
                        { label: t("基础费"), value: nativeText({ network }, SOLANA_BASE_FEE) },
                        { label: t("优先费"), value: priority === null ? "—" : nativeText({ network }, priority) },
                        {
                            label: t("收款方代币账户"),
                            value: !token ? t("不需要") : estimate ? (createAta ? t("需要创建") : t("已存在")) : "—",
                        },
                        { label: t("创建租金"), value: createAta ? nativeText({ network }, rent) : t("无") },
                    ],
                    fee,
                    problem: errorText(estimated.error),
                    advancedLabel: t("nonce 账户"),
                    advanced: [
                        {
                            id: "adv-nonce",
                            label: t("nonce 账户"),
                            value: shortAddress(nonceAccount),
                            editable: false,
                        },
                        {
                            id: "adv-prio",
                            label: t("优先费"),
                            value: priority === null ? "—" : nativeText({ network }, priority),
                            editable: false,
                        },
                    ],
                    validity: t("使用 durable nonce,签名不过期"),
                    setup: missing
                        ? {
                              title: t("创建 nonce 账户"),
                              helper: t("首次 Solana 转账前要先创建,需要少量可赎回的 SOL."),
                              action: t("创建账户"),
                          }
                        : null,
                    busyNotice: busy ? t("上一笔 Solana 交易还没有结束,完成后才能转账.") : null,
                },
                input,
            );
        },
    );
}

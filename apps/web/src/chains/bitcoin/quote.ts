// Bitcoin 的报价.手续费按所选的币算,没有单独的手续费余额.
import { t } from "../../core/i18n";
import type { Account } from "../../data/db";
import { firstFree } from "../../data/claims";
import type { FeeTier } from "../../gateway/client";
import { TIERS, defineQuote, type QuoteInput, type QuotePlan } from "../quote";
import { nativeText } from "../view";
import { selectCoins } from "./core";
import { spendableCoins } from "./transfer";

/** 三档的目标区块数,和网关估算费率用的目标相同. */
const TARGETS: Record<FeeTier, string> = { slow: t("约 144 块"), medium: t("约 6 块"), fast: t("约 1 块") };

function maxAmount(coins: Parameters<typeof selectCoins>[0], rate: bigint, kind: Account["kind"]): bigint {
    const total = coins.reduce((sum, item) => sum + BigInt(item.amount), 0n);
    const fits = (amount: bigint) => {
        try {
            selectCoins(coins, amount, rate, kind);
            return true;
        } catch {
            return false;
        }
    };
    // 手续费由核心库估算,这里只二分,不在 TS 里算交易大小.
    let low = 0n;
    let high = total;
    while (low < high) {
        const middle = (low + high + 1n) >> 1n;
        if (fits(middle)) low = middle;
        else high = middle - 1n;
    }
    return low;
}

export function quote(input: QuoteInput): QuotePlan {
    const { account, network, amount, tier } = input;
    return defineQuote(
        {
            coins: {
                key: ["chain", network, "coins", account.id, input.reservations.length],
                run: () => spendableCoins(account, network),
                retry: 1,
            },
        },
        ({ coins }, base, feeData) => {
            const list = coins.data ?? [];
            const available = coins.data ? list.reduce((sum, item) => sum + BigInt(item.amount), 0n) : null;
            const quoteTier = (item: FeeTier) => {
                if (feeData?.kind !== "bitcoin") return null;
                try {
                    return selectCoins(list, amount && amount > 0n ? amount : 1n, feeData[item], account.kind);
                } catch {
                    return null;
                }
            };
            const tiers =
                feeData?.kind === "bitcoin"
                    ? TIERS.map((item) => {
                          const picked = quoteTier(item);
                          return {
                              tier: item,
                              cost: picked ? nativeText({ network }, BigInt(picked.fee)) : t("余额不足"),
                              rate: `${feeData[item]} sat/vB`,
                              eta: TARGETS[item],
                          };
                      })
                    : null;
            const chosen = quoteTier(tier);
            const change = chosen && chosen.change > 0 ? firstFree(input.reservations, "change", 0) : null;
            const enough = !coins.data || !feeData ? null : chosen !== null;
            return {
                ...base,
                failed: base.failed || coins.isError,
                feeTag: "sat/vB",
                feeText: chosen ? nativeText({ network }, BigInt(chosen.fee)) : "—",
                tiers,
                fee: chosen ? BigInt(chosen.fee) : null,
                available,
                max: feeData?.kind === "bitcoin" && coins.data ? maxAmount(list, feeData[tier], account.kind) : null,
                enough,
                enoughText:
                    enough === null ? t("等待报价") : enough ? t("余额足够支付手续费") : t("余额不足以支付手续费"),
                advancedLabel: change === null ? t("RBF 开启") : t("找零索引 {0}", change),
                advanced: [
                    {
                        id: "adv-change",
                        label: t("找零索引"),
                        value: change === null ? t("无找零") : String(change),
                        editable: false,
                    },
                    { id: "adv-rbf", label: "RBF", value: t("开启"), editable: false },
                ],
            };
        },
    );
}

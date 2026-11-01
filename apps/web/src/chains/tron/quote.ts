// TRON 的报价.带宽和能量够用时不花 TRX,不够时燃烧.
import { t } from "../../core/i18n";
import { cleanText } from "../../core/text";
import { defineQuote, errorText, isReady, settle, type QuoteInput, type QuotePlan, type QuoteRow } from "../quote";
import { nativeText } from "../view";
import { estimateTron, fetchResources } from "./api";
import { TRON_DEFAULT_FEE_LIMIT, tronCallData, tronCost, tronFeeLimit } from "./transfer";

export function quote(input: QuoteInput): QuotePlan {
    const { network, from, to, amount, asset } = input;
    return defineQuote(
        {
            resources: {
                key: ["chain", network, "resources", from],
                run: () => fetchResources(network, from),
                retry: 1,
            },
            estimate: {
                key: ["chain", network, "estimate", from, to, asset, amount?.toString()],
                run: () =>
                    estimateTron(
                        network,
                        asset === "native"
                            ? { from, to: to ?? "" }
                            : {
                                  from,
                                  to: to ?? "",
                                  contract: asset,
                                  data: tronCallData({ ...input, to: to ?? "", amount: amount ?? 0n }),
                              },
                    ),
                enabled: isReady(input),
                retry: false,
            },
        },
        ({ resources, estimate: estimated }, base, feeData) => {
            const token = asset !== "native";
            const estimate = estimated.data;
            const resource = resources.data;
            const prices = feeData?.kind === "tron" ? feeData : null;
            const cost =
                estimate && resource && prices
                    ? tronCost(estimate, resource, prices.energyPrice, prices.bandwidthPrice)
                    : null;
            const feeLimit =
                input.feeLimitOverride ??
                (estimate && prices
                    ? tronFeeLimit(estimate.energy, prices.energyPrice)
                    : estimated.isError
                      ? TRON_DEFAULT_FEE_LIMIT
                      : null);
            const rows: QuoteRow[] = [
                { label: t("带宽"), value: estimate?.bandwidth.toString() ?? "—" },
                { label: t("能量"), value: estimate?.energy.toString() ?? "—" },
                {
                    label: t("不足时燃烧"),
                    value: cost ? nativeText({ network }, cost.energyBurn + cost.bandwidthBurn) : "—",
                },
                {
                    label: t("收款方激活费"),
                    value: cost
                        ? cost.activationFee > 0n
                            ? nativeText({ network }, cost.activationFee)
                            : t("无")
                        : "—",
                },
            ];
            const problem =
                resource && !resource.activated ? t("付款账户还没有激活,先转入一笔 TRX.") : errorText(estimated.error);
            return settle(
                {
                    ...base,
                    failed: base.failed || resources.isError,
                    feeTag: t("带宽和能量"),
                    feeLabel: token ? "fee_limit" : t("预计燃烧"),
                    feeText: token
                        ? feeLimit === null
                            ? "—"
                            : nativeText({ network }, feeLimit)
                        : cost
                          ? nativeText({ network }, cost.total)
                          : "—",
                    rows,
                    fee: cost?.total ?? null,
                    problem,
                    problemNote:
                        token && problem && estimated.isError
                            ? t("估算失败:{0},会用默认值生成请求,可以在高级设置里修改.", cleanText(problem, 160))
                            : null,
                    advancedLabel: t("过期 30 分钟"),
                    advanced: [
                        { id: "adv-exp", label: t("过期时间"), value: t("30 分钟"), editable: false },
                        ...(token
                            ? [
                                  {
                                      id: "adv-limit",
                                      label: "fee_limit(sun)",
                                      value: feeLimit?.toString() ?? "",
                                      editable: true,
                                  },
                              ]
                            : []),
                    ],
                    validity: t("签名 30 分钟后过期"),
                },
                input,
            );
        },
    );
}

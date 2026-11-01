// EVM 的报价.手续费上限等于 Gas 上限乘以最高单价.
import { formatUnits } from "../../core/amount";
import { t } from "../../core/i18n";
import { cleanText } from "../../core/text";
import { abandonedHolder, firstFree } from "../../data/claims";
import { TIERS, defineQuote, errorText, isReady, settle, type QuoteInput, type QuotePlan } from "../quote";
import { nativeText } from "../view";
import { fetchNonces } from "./api";
import { evmDefaultGas, evmGasLimit } from "./transfer";

function gwei(raw: bigint): string {
    return `${formatUnits(raw, 9, { max: 4 })} Gwei`;
}

export function quote(input: QuoteInput): QuotePlan {
    const { network, from, to, amount, asset, tier } = input;
    return defineQuote(
        {
            nonces: {
                key: ["chain", network, "nonce", from],
                run: () => fetchNonces(network, [from]),
                retry: 1,
            },
            gas: {
                key: ["chain", network, "gas", from, to, asset, amount?.toString()],
                run: () => evmGasLimit({ ...input, to: to ?? "", amount: amount ?? 0n, gasLimit: undefined }),
                enabled: isReady(input) && input.gasOverride === null,
                retry: false,
            },
        },
        ({ nonces, gas }, base, feeData) => {
            const gasLimit = input.gasOverride ?? gas.data ?? evmDefaultGas(asset);
            const chainNonce = nonces.data?.get(from);
            const floor = chainNonce ? Math.max(chainNonce.latest, chainNonce.pending) : null;
            const nonce = floor === null ? null : firstFree(input.reservations, "nonce", floor);
            const tiers =
                feeData?.kind === "evm"
                    ? TIERS.map((item) => ({
                          tier: item,
                          cost: nativeText({ network }, gasLimit * feeData[item].maxFee),
                          rate: gwei(feeData[item].maxFee),
                          eta: null,
                      }))
                    : null;
            const selected = feeData?.kind === "evm" ? gasLimit * feeData[tier].maxFee : null;
            const problem = input.gasOverride === null ? errorText(gas.error) : null;
            return settle(
                {
                    ...base,
                    failed: base.failed || nonces.isError,
                    feeTag: "EIP-1559",
                    feeLabel: t("手续费上限"),
                    feeText: selected === null ? "—" : nativeText({ network }, selected),
                    tiers,
                    fee: selected,
                    problem,
                    problemNote:
                        problem === null
                            ? null
                            : t("估算失败:{0},会用默认值生成请求,可以在高级设置里修改.", cleanText(problem, 160)),
                    held: floor === null ? null : abandonedHolder(input.reservations, input.txs, floor),
                    advancedLabel: nonce === null ? "" : `Nonce ${nonce}`,
                    advanced: [
                        {
                            id: "adv-nonce",
                            label: "Nonce",
                            value: nonce === null ? "—" : String(nonce),
                            editable: false,
                        },
                        { id: "adv-gas", label: t("Gas 上限"), value: gasLimit.toString(), editable: true },
                    ],
                },
                input,
            );
        },
    );
}

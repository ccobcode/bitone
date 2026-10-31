// W3 的报价。手续费的形式按链不同，见 docs/03-system.md 第 10 节。
// 这里只用于显示和预检查，真正构建时在账户锁里重新查询。
import { useQuery } from "@tanstack/react-query";
import { selectCoins, solanaNonceAddress } from "../core";
import { formatUnits } from "../core/amount";
import type { AssetEntry } from "../core/manifest";
import { lookupAsset } from "../core/manifest";
import { shortAddress } from "../core/text";
import { nativeText } from "../data/txview";
import type { Account, Reservation, TxRecord, Wallet } from "../data/db";
import {
    SOLANA_BASE_FEE,
    TransferError,
    evmGasLimit,
    solanaComputeUnits,
    spendableCoins,
    tronCallData,
    tronCost,
    tronFeeLimit,
} from "../data/transfer";
import { abandonedHolder, firstFree } from "../data/txs";
import {
    estimateSolana,
    estimateTron,
    fetchFees,
    fetchNonceAccount,
    fetchNonces,
    fetchResources,
    GatewayError,
    type FeeTier,
} from "./client";
import { t, translate } from "../core/i18n";

export const TIERS: FeeTier[] = ["slow", "medium", "fast"];

/** Bitcoin 三档的目标区块数，见 docs/09。 */
const BITCOIN_TARGETS: Record<FeeTier, string> = { slow: t("约 144 块"), medium: t("约 6 块"), fast: t("约 1 块") };

export interface TierQuote {
    tier: FeeTier;
    cost: string;
    rate: string;
    eta: string | null;
}

export interface QuoteRow {
    label: string;
    value: string;
}

export interface AdvancedField {
    id: string;
    label: string;
    value: string;
    /** 可以手动修改的字段。序号都在账户锁里分配，不能手填。 */
    editable: boolean;
}

export interface Quote {
    /** 网关查询失败，不能下结论。 */
    failed: boolean;
    feeTag: string;
    feeLabel: string;
    feeText: string;
    tiers: TierQuote[] | null;
    rows: QuoteRow[] | null;
    /** 这笔要付的手续费，单位是原生币的最小单位。不知道时是 null。 */
    fee: bigint | null;
    /** 资产的可用余额。 */
    available: bigint | null;
    /** 「全部」对应的金额。 */
    max: bigint | null;
    /** 余额够不够。不知道时是 null。 */
    enough: boolean | null;
    enoughText: string;
    /** 估算失败的原因，比如合约回滚。 */
    problem: string | null;
    advancedLabel: string;
    advanced: AdvancedField[];
    validity: string;
    needsNonceAccount: boolean;
    nonceBusy: boolean;
    /** 已取消但签名仍然有效的交易占着 nonce，新交易打包不了。 */
    held: TxRecord | null;
}

export interface QuoteInput {
    wallet: Wallet;
    accounts: Account[];
    account: Account;
    from: string;
    network: string;
    asset: string;
    entry: AssetEntry;
    /** 校验过的收款地址，还没填好时是 null。 */
    to: string | null;
    amount: bigint | null;
    tier: FeeTier;
    gasOverride: bigint | null;
    feeLimitOverride: bigint | null;
    /** 这个账户在这个网络上的资产余额，扣掉待转出之后的。 */
    balance: bigint | null;
    /** 原生币余额，扣掉待转出之后的。 */
    nativeBalance: bigint | null;
    reservations: readonly Reservation[];
    txs: readonly TxRecord[];
}

function gwei(raw: bigint): string {
    return `${formatUnits(raw, 9, { max: 4 })} Gwei`;
}

function errorText(error: unknown): string | null {
    if (!error) return null;
    if (error instanceof TransferError) return translate(error.message);
    if (error instanceof GatewayError) return translate(error.detail) || t("网关暂时无法估算。");
    return t("无法估算。");
}

function maxBitcoin(coins: Parameters<typeof selectCoins>[0], rate: bigint, kind: Account["kind"]): bigint {
    const total = coins.reduce((sum, item) => sum + BigInt(item.amount), 0n);
    const fits = (amount: bigint) => {
        try {
            selectCoins(coins, amount, rate, kind);
            return true;
        } catch {
            return false;
        }
    };
    // 手续费由核心库估算，这里只二分，不在 TS 里算交易大小。
    let low = 0n;
    let high = total;
    while (low < high) {
        const middle = (low + high + 1n) >> 1n;
        if (fits(middle)) low = middle;
        else high = middle - 1n;
    }
    return low;
}

function balanceText(native: AssetEntry | null, enough: boolean | null): string {
    const symbol = native?.symbol ?? "";
    if (enough === null) return t("等待报价");
    return enough ? t("{0} 余额足够支付手续费", symbol) : t("{0} 余额不足以支付手续费", symbol);
}

export function useQuote(input: QuoteInput): Quote {
    const { account, network, from, to, amount, asset, tier, entry } = input;
    const chain = account.chain;
    const fees = useQuery({
        queryKey: ["chain", network, "fees"],
        queryFn: () => fetchFees(network),
        refetchInterval: 15_000,
        retry: 1,
    });
    const ready = to !== null && amount !== null && amount > 0n;
    const nonces = useQuery({
        queryKey: ["chain", network, "nonce", from],
        queryFn: () => fetchNonces(network, [from]),
        enabled: chain === "evm",
        retry: 1,
    });
    const gas = useQuery({
        queryKey: ["chain", network, "gas", from, to, asset, amount?.toString()],
        queryFn: () => evmGasLimit({ ...input, to: to ?? "", amount: amount ?? 0n, gasLimit: undefined }),
        enabled: chain === "evm" && ready && input.gasOverride === null,
        retry: false,
    });
    const coins = useQuery({
        queryKey: ["chain", network, "coins", account.id, input.reservations.length],
        queryFn: () => spendableCoins(account, network),
        enabled: chain === "bitcoin",
        retry: 1,
    });
    const resources = useQuery({
        queryKey: ["chain", network, "resources", from],
        queryFn: () => fetchResources(network, from),
        enabled: chain === "tron",
        retry: 1,
    });
    const tronEstimate = useQuery({
        queryKey: ["chain", network, "estimate", from, to, asset, amount?.toString()],
        queryFn: () =>
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
        enabled: chain === "tron" && ready,
        retry: false,
    });
    const nonceAccount = chain === "solana" ? solanaNonceAddress(from) : "";
    const nonceInfo = useQuery({
        queryKey: ["chain", network, "nonce-account", nonceAccount],
        queryFn: () => fetchNonceAccount(network, nonceAccount),
        enabled: chain === "solana",
        retry: 1,
    });
    const solanaEstimate = useQuery({
        queryKey: ["chain", network, "estimate", to, asset],
        queryFn: () => estimateSolana(network, to ?? "", asset),
        enabled: chain === "solana" && asset !== "native" && to !== null,
        retry: false,
    });

    const native = lookupAsset(network, "native");
    const feeData = fees.data;
    const base: Quote = {
        failed: fees.isError,
        feeTag: "",
        feeLabel: t("手续费"),
        feeText: "—",
        tiers: null,
        rows: null,
        fee: null,
        available: input.balance,
        max: null,
        enough: null,
        enoughText: t("等待报价"),
        problem: null,
        advancedLabel: "",
        advanced: [],
        validity: t("手续费报价 30 分钟内有效"),
        needsNonceAccount: false,
        nonceBusy: false,
        held: null,
    };

    const settle = (quote: Quote): Quote => {
        const { fee, available } = quote;
        let enough: boolean | null = null;
        let max: bigint | null = null;
        if (fee !== null && available !== null && input.nativeBalance !== null) {
            if (entry.native) {
                enough = (amount ?? 0n) + fee <= available;
                max = available > fee ? available - fee : 0n;
            } else {
                enough = fee <= input.nativeBalance;
                max = available;
            }
        }
        return { ...quote, enough, max, enoughText: balanceText(native, enough) };
    };

    if (chain === "evm") {
        const gasLimit = input.gasOverride ?? gas.data ?? (asset === "native" ? 21_000n : 65_000n);
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
        return settle({
            ...base,
            failed: fees.isError || nonces.isError,
            feeTag: "EIP-1559",
            feeLabel: t("手续费上限"),
            feeText: selected === null ? "—" : nativeText({ network }, selected),
            tiers,
            fee: selected,
            problem: input.gasOverride === null ? errorText(gas.error) : null,
            held: floor === null ? null : abandonedHolder(input.reservations, input.txs, floor),
            advancedLabel: nonce === null ? "" : `Nonce ${nonce}`,
            advanced: [
                { id: "adv-nonce", label: "Nonce", value: nonce === null ? "—" : String(nonce), editable: false },
                { id: "adv-gas", label: t("Gas 上限"), value: gasLimit.toString(), editable: true },
            ],
        });
    }

    if (chain === "bitcoin") {
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
                          eta: BITCOIN_TARGETS[item],
                      };
                  })
                : null;
        const chosen = quoteTier(tier);
        const change = chosen && chosen.change > 0 ? firstFree(input.reservations, "change", 0) : null;
        const enough = !coins.data || !feeData ? null : chosen !== null;
        return {
            ...base,
            failed: fees.isError || coins.isError,
            feeTag: "sat/vB",
            feeText: chosen ? nativeText({ network }, BigInt(chosen.fee)) : "—",
            tiers,
            fee: chosen ? BigInt(chosen.fee) : null,
            available,
            max: feeData?.kind === "bitcoin" && coins.data ? maxBitcoin(list, feeData[tier], account.kind) : null,
            enough,
            enoughText: enough === null ? t("等待报价") : enough ? t("余额足够支付手续费") : t("余额不足以支付手续费"),
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
    }

    if (chain === "tron") {
        const token = asset !== "native";
        const estimate = tronEstimate.data;
        const resource = resources.data;
        const prices = feeData?.kind === "tron" ? feeData : null;
        const cost =
            estimate && resource && prices
                ? tronCost(estimate, resource, prices.energyPrice, prices.bandwidthPrice)
                : null;
        const feeLimit =
            input.feeLimitOverride ?? (estimate && prices ? tronFeeLimit(estimate.energy, prices.energyPrice) : null);
        const rows: QuoteRow[] = [
            { label: t("带宽"), value: estimate?.bandwidth.toString() ?? "—" },
            { label: t("能量"), value: estimate?.energy.toString() ?? "—" },
            {
                label: t("不足时燃烧"),
                value: cost ? nativeText({ network }, cost.energyBurn + cost.bandwidthBurn) : "—",
            },
            {
                label: t("收款方激活费"),
                value: cost ? (cost.activationFee > 0n ? nativeText({ network }, cost.activationFee) : t("无")) : "—",
            },
        ];
        const problem =
            resource && !resource.activated ? t("付款账户还没有激活，先转入一笔 TRX。") : errorText(tronEstimate.error);
        return settle({
            ...base,
            failed: fees.isError || resources.isError,
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
            advancedLabel: t("过期 30 分钟"),
            advanced: [
                { id: "adv-exp", label: t("过期时间"), value: t("30 分钟"), editable: false },
                ...(token
                    ? [
                          {
                              id: "adv-limit",
                              label: "fee_limit（sun）",
                              value: feeLimit?.toString() ?? "",
                              editable: true,
                          },
                      ]
                    : []),
            ],
            validity: t("签名 30 分钟后过期"),
        });
    }

    // Solana：基础费加优先费，收款方没有代币账户时还有创建租金。
    const token = asset !== "native";
    const estimate = solanaEstimate.data;
    const createAta = token && estimate !== undefined && !estimate.recipientTokenAccount;
    const units = BigInt(solanaComputeUnits(token, createAta));
    const priority = feeData?.kind === "solana" ? (feeData.medium * units + 999_999n) / 1_000_000n : null;
    const rent = estimate?.rent ?? 0n;
    const fee = priority === null ? null : SOLANA_BASE_FEE + priority + rent;
    const missing = nonceInfo.data?.exists === false;
    return settle({
        ...base,
        failed: fees.isError || nonceInfo.isError,
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
        problem: errorText(solanaEstimate.error),
        advancedLabel: t("nonce 账户"),
        advanced: [
            { id: "adv-nonce", label: t("nonce 账户"), value: shortAddress(nonceAccount), editable: false },
            {
                id: "adv-prio",
                label: t("优先费"),
                value: priority === null ? "—" : nativeText({ network }, priority),
                editable: false,
            },
        ],
        validity: t("使用 durable nonce，签名不过期"),
        needsNonceAccount: missing,
        nonceBusy: input.reservations.some((row) => row.type === "solana-nonce"),
    });
}

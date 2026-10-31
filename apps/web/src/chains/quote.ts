// W3 的报价.手续费的形式按链不同,见 docs/03-system.md 第 10 节.
// 这里只用于显示和预检查,真正构建时在账户锁里重新查询.
import { t, translate } from "../core/i18n";
import { lookupAsset, type AssetEntry } from "../core/manifest";
import type { Account, Reservation, TxRecord, Wallet } from "../data/db";
import { GatewayError, type FeeTier } from "../gateway/client";
import type { Fees } from "./index";
import { TransferError } from "./transfer";

export const TIERS: FeeTier[] = ["slow", "medium", "fast"];

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
    /** 可以手动修改的字段.序号都在账户锁里分配,不能手填. */
    editable: boolean;
}

/** 转账前要先完成的准备步骤. */
export interface SetupStep {
    title: string;
    helper: string;
    action: string;
}

export interface Quote {
    /** 网关查询失败,不能下结论. */
    failed: boolean;
    feeTag: string;
    feeLabel: string;
    feeText: string;
    tiers: TierQuote[] | null;
    rows: QuoteRow[] | null;
    /** 这笔要付的手续费,单位是原生币的最小单位.不知道时是 null. */
    fee: bigint | null;
    available: bigint | null;
    /** "全部"对应的金额. */
    max: bigint | null;
    /** 余额够不够.不知道时是 null. */
    enough: boolean | null;
    enoughText: string;
    /** 估算失败的原因,比如合约回滚. */
    problem: string | null;
    /** 给用户看的说明,缺省时直接显示 `problem`. */
    problemNote: string | null;
    advancedLabel: string;
    advanced: AdvancedField[];
    validity: string;
    setup: SetupStep | null;
    /** 这个账户现在不能转账的原因. */
    busyNotice: string | null;
    /** 已取消但签名仍然有效的交易占着 nonce,新交易打包不了. */
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
    /** 校验过的收款地址,还没填好时是 null. */
    to: string | null;
    amount: bigint | null;
    tier: FeeTier;
    gasOverride: bigint | null;
    feeLimitOverride: bigint | null;
    /** 这个账户在这个网络上的资产余额,扣掉待转出之后的. */
    balance: bigint | null;
    /** 原生币余额,扣掉待转出之后的. */
    nativeBalance: bigint | null;
    reservations: readonly Reservation[];
    txs: readonly TxRecord[];
}

/** 报价要跑的一个查询.只描述,由 `useQuote` 执行,换链时查询的数量可以不同. */
export interface QuoteQuery<T = unknown> {
    key: readonly unknown[];
    run(): Promise<T>;
    enabled?: boolean;
    retry: number | false;
}

export interface QueryView<T = unknown> {
    data: T | undefined;
    error: unknown;
    isError: boolean;
}

type Queries = Record<string, QuoteQuery>;
type Views<Q> = { [K in keyof Q]: Q[K] extends QuoteQuery<infer T> ? QueryView<T> : never };

export interface QuotePlan {
    queries: Queries;
    build(results: Record<string, QueryView>, base: Quote, fees: Fees | undefined): Quote;
}

/** 把带类型的查询和结果对应起来,只在这里擦除类型. */
export function defineQuote<Q extends Queries>(
    queries: Q,
    build: (results: Views<Q>, base: Quote, fees: Fees | undefined) => Quote,
): QuotePlan {
    return { queries, build: (results, base, fees) => build(results as Views<Q>, base, fees) };
}

export function isReady({ to, amount }: Pick<QuoteInput, "to" | "amount">): boolean {
    return to !== null && amount !== null && amount > 0n;
}

export function baseQuote(input: QuoteInput, feesFailed: boolean): Quote {
    return {
        failed: feesFailed,
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
        problemNote: null,
        advancedLabel: "",
        advanced: [],
        validity: t("手续费报价 30 分钟内有效"),
        setup: null,
        busyNotice: null,
        held: null,
    };
}

export function errorText(error: unknown): string | null {
    if (!error) return null;
    if (error instanceof TransferError) return translate(error.message);
    if (error instanceof GatewayError) return translate(error.detail) || t("网关暂时无法估算.");
    return t("无法估算.");
}

function balanceText(native: AssetEntry | null, enough: boolean | null): string {
    const symbol = native?.symbol ?? "";
    if (enough === null) return t("等待报价");
    return enough ? t("{0} 余额足够支付手续费", symbol) : t("{0} 余额不足以支付手续费", symbol);
}

/** 按手续费和余额算出够不够,以及"全部"对应的金额. */
export function settle(quote: Quote, input: QuoteInput): Quote {
    const { fee, available } = quote;
    let enough: boolean | null = null;
    let max: bigint | null = null;
    if (fee !== null && available !== null && input.nativeBalance !== null) {
        if (input.entry.native) {
            enough = (input.amount ?? 0n) + fee <= available;
            max = available > fee ? available - fee : 0n;
        } else {
            enough = fee <= input.nativeBalance;
            max = available;
        }
    }
    return { ...quote, enough, max, enoughText: balanceText(lookupAsset(input.network, "native"), enough) };
}

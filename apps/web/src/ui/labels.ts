// 账户在界面上的名称。
import { networkName, networksFor, type AccountKind, type Chain, type Mode } from "../core";
import type { NetworkIcon } from "./TokenIcon";
import { t } from "../core/i18n";

const CHAIN_TITLES: Record<Chain, string> = {
    bitcoin: "Bitcoin",
    evm: "EVM",
    tron: "TRON",
    solana: "Solana",
};

const CHAIN_ICONS: Record<Chain, NetworkIcon> = {
    bitcoin: "bitcoin",
    evm: "ethereum",
    tron: "tron",
    solana: "solana",
};

const KIND_LABELS: Record<Exclude<AccountKind, "single">, string> = {
    segwit: "Native SegWit",
    taproot: "Taproot",
    "nested-segwit": "Nested SegWit",
    legacy: "Legacy",
};

const SINGLE_LABELS: Record<Exclude<Chain, "bitcoin">, string> = {
    evm: "Ethereum、BSC、Polygon",
    tron: "TRC-20",
    solana: "SPL",
};

interface AccountLike {
    chain: Chain;
    kind: AccountKind;
}

export function accountTitle(account: AccountLike): string {
    return CHAIN_TITLES[account.chain];
}

export function accountIcon(account: AccountLike): NetworkIcon {
    return CHAIN_ICONS[account.chain];
}

export function accountKind({ chain, kind }: AccountLike): string {
    if (kind !== "single") return KIND_LABELS[kind];
    return chain === "bitcoin" ? "" : SINGLE_LABELS[chain];
}

/** 这个账户可用的全部网络名称。 */
export function accountNetworks(chain: Chain, mode: Mode): string {
    return networksFor(chain, mode)
        .map((network) => networkName(network.id))
        .join(t("、"));
}

export function modeLabel(mode: Mode): string {
    return mode === "testnet" ? t("测试网") : t("主网");
}

const TIER_LABELS = { slow: t("慢"), medium: t("中"), fast: t("快") } as const;

export function tierLabel(tier: keyof typeof TIER_LABELS): string {
    return TIER_LABELS[tier];
}

/** 时间线和列表里的时间。 */
export function timeLabel(time: number, now = Date.now()): string {
    const date = new Date(time);
    const pad = (value: number) => String(value).padStart(2, "0");
    const clock = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
    const today = new Date(now);
    if (date.toDateString() === today.toDateString()) return t("今天 {0}", clock);
    const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
    if (date.toDateString() === yesterday.toDateString()) return t("昨天 {0}", clock);
    return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${clock}`;
}

export function clockLabel(time: number): string {
    return new Date(time).toLocaleTimeString("zh-CN", { hour12: false });
}

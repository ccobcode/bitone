// 账户在界面上的名称.
import { chainOf } from "../chains";
import { networkName, networksFor, type AccountKind, type Chain, type Mode } from "../core";
import { t } from "../core/i18n";

interface AccountLike {
    chain: Chain;
    kind: AccountKind;
}

export function accountTitle(account: AccountLike): string {
    return chainOf(account.chain).title;
}

export function accountIcon(account: AccountLike): string {
    return chainOf(account.chain).icon;
}

export function accountKind({ chain, kind }: AccountLike): string {
    return chainOf(chain).kindLabels[kind] ?? "";
}

/** 列表里的账户名称.有多种账户的链要带上类型. */
export function accountName(account: AccountLike): string {
    return chainOf(account.chain).namedByKind
        ? `${accountTitle(account)} ${accountKind(account)}`
        : accountTitle(account);
}

/** 这个账户可用的全部网络名称. */
export function accountNetworks(chain: Chain, mode: Mode): string {
    return networksFor(chain, mode)
        .map((network) => networkName(network.id))
        .join(t(","));
}

export function modeLabel(mode: Mode): string {
    return mode === "testnet" ? t("测试网") : t("主网");
}

const TIER_LABELS = { slow: t("慢"), medium: t("中"), fast: t("快") } as const;

export function tierLabel(tier: keyof typeof TIER_LABELS): string {
    return TIER_LABELS[tier];
}

const pad = (value: number) => String(value).padStart(2, "0");

/** 时间线和列表里的时间. */
export function timeLabel(time: number, now = Date.now()): string {
    return `${dayLabel(time, now)} ${minuteLabel(time)}`;
}

/** 本地日期 YYYY-MM-DD. */
export function dateLabel(time: number): string {
    const date = new Date(time);
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 列表里按天分组的标题.今年的不写年份. */
export function dayLabel(time: number, now = Date.now()): string {
    const date = new Date(time);
    const today = new Date(now);
    if (date.toDateString() === today.toDateString()) return t("今天");
    const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
    if (date.toDateString() === yesterday.toDateString()) return t("昨天");
    const full = dateLabel(time);
    return date.getFullYear() === today.getFullYear() ? full.slice(5) : full;
}

export function minuteLabel(time: number): string {
    const date = new Date(time);
    return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function clockLabel(time: number): string {
    return new Date(time).toLocaleTimeString("zh-CN", { hour12: false });
}

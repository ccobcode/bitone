import { useState, type ReactNode } from "react";
import { Link } from "react-router";
import { isLegacyKind, networkName } from "../core";
import { formatUnits, formatUsd } from "../core/amount";
import { shortAddress } from "../core/text";
import { saveSetting, useApp, useLive } from "../data/app";
import { db } from "../data/db";
import {
    assetShares,
    buildRows,
    groupPending,
    groupRows,
    largestRow,
    sendLink,
    type AssetGroup,
    type AssetRow,
    type Share,
} from "../gateway/assets";
import { useAttention, useFeed, useWalletTxs } from "../gateway/feed";
import { useGateway } from "../gateway/provider";
import { FeedItem } from "../ui/FeedList";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon } from "../ui/Icon";
import { dateLabel } from "../ui/labels";
import {
    Chip,
    Collapse,
    Disclosure,
    Empty,
    LinkButton,
    Notice,
    PageTitle,
    Section,
    SectionTitle,
    Skeleton,
    SkeletonRows,
} from "../ui/primitives";
import { overlayLink, receiveLink } from "../ui/Sheet";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
import { t } from "../core/i18n";

/** 表格最多显示 8 位小数，完整金额放在 title 里。 */
const TABLE_FRACTION = 8;
const RECENT = 5;

/** 颜色跟着资产走，不跟名次，资产排序变了颜色也不变。 */
const SHARE_COLORS: Record<Share["key"], string> = {
    btc: "var(--chart-2)",
    eth: "var(--chart-1)",
    usdt: "var(--chart-3)",
    bnb: "var(--chart-4)",
    sol: "var(--chart-5)",
    pol: "var(--chart-7)",
    trx: "var(--chart-8)",
    usdc: "var(--chart-6)",
    other: "var(--text-3)",
};

function GroupAmount({ group, loading }: { group: AssetGroup; loading: boolean }) {
    if (!group.loaded) return loading ? <Skeleton className="my-1 h-4 w-20" /> : <span className="text-text-3">—</span>;
    const { amount, decimals, entry } = group;
    const text = formatUnits(amount, decimals, { min: entry.minFraction, max: TABLE_FRACTION });
    return (
        <span key={text} className="fade-in" title={`${formatUnits(amount, decimals)} ${entry.symbol}`}>
            {text}
        </span>
    );
}

/** 第二行：在哪些网络上。一个网络时写名称，多个时写数量，详情里才拆开。 */
function whereLabel(group: AssetGroup): string {
    const held = group.rows.filter((row) => row.amount > 0n);
    const rows = held.length > 0 ? held : group.rows;
    const networks = new Set(rows.map((row) => row.network));
    if (networks.size > 1) return t("{0} 个网络", networks.size);
    if (rows.length > 1) return t("{0} 个账户", rows.length);
    return rows[0] ? networkName(rows[0].network) : "";
}

function Row({ group, pending, loading }: { group: AssetGroup; pending: bigint; loading: boolean }) {
    const { entry } = group;
    const top = largestRow(group);
    const receivable = !isLegacyKind(top.account.kind);
    return (
        <li className="row relative flex min-h-row items-center gap-s3 py-s3">
            <Link
                to={`/assets/${group.key}`}
                aria-label={t("查看 {0}", entry.symbol)}
                className="absolute inset-0 rounded-r2"
            />
            <TokenIcon asset={entry} size={36} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-t3 font-semibold">{entry.symbol}</span>
                <span className="truncate text-t2 text-text-3">{whereLabel(group)}</span>
            </span>
            <span className="row-actions relative hidden shrink-0 gap-s1 @split:flex">
                {receivable ? (
                    <LinkButton
                        {...receiveLink(top.account.id, { asset: group.key })}
                        variant="ghost"
                        size="sm"
                        aria-label={t("收款 {0}", entry.symbol)}
                    >
                        {t("收款")}
                    </LinkButton>
                ) : null}
                {group.amount > 0n ? (
                    <LinkButton to={sendLink(top)} variant="ghost" size="sm" aria-label={t("转账 {0}", entry.symbol)}>
                        {t("转账")}
                    </LinkButton>
                ) : null}
            </span>
            <span className="flex min-w-0 flex-col items-end text-right">
                <span className="text-t3 font-medium">
                    <GroupAmount group={group} loading={loading} />
                </span>
                {pending > 0n ? (
                    <span className="text-t2 text-warn">
                        {t("待转出")} {formatUnits(pending, group.decimals, { min: entry.minFraction })}
                    </span>
                ) : group.cents !== null ? (
                    <span className="text-t2 text-text-3">{formatUsd(group.cents)}</span>
                ) : !group.loaded && loading ? (
                    <Skeleton className="my-1 h-3 w-16" />
                ) : null}
            </span>
            <Icon name="caret" size={16} className="shrink-0 text-text-3" />
        </li>
    );
}

/** 清单外的代币。没有可信的符号和小数位，只显示原始整数，也没有详情可进。 */
function UnknownRow({ row }: { row: AssetRow }) {
    return (
        <li className="row flex min-h-row items-center gap-s3 py-s3">
            <TokenIcon asset={null} size={36} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-t3 font-semibold text-warn">{t("未知代币")}</span>
                <span className="truncate text-t2 text-text-3">
                    {networkName(row.network)} <span className="font-mono text-t1">{shortAddress(row.asset)}</span>
                </span>
            </span>
            <span className="min-w-0 text-right text-t3 font-medium [overflow-wrap:anywhere]">
                {row.amount.toString()}
            </span>
        </li>
    );
}

/** 资产分布。估值只用于展示。 */
function Allocation({ shares }: { shares: Share[] }) {
    const name = (share: Share) => share.label ?? t("其他");
    const percent = (share: Share) => (share.percent < 1 ? "<1%" : `${Math.round(share.percent)}%`);
    return (
        <div className="flex flex-col gap-s2">
            <div
                role="img"
                aria-label={shares.map((share) => `${name(share)} ${percent(share)}`).join(t("，"))}
                className="flex h-2 gap-0.5"
            >
                {shares.map((share) => (
                    <span
                        key={share.key}
                        title={`${name(share)} ${formatUsd(share.cents)} ${percent(share)}`}
                        className="min-w-1 rounded-[2px] first:rounded-l-rf last:rounded-r-rf"
                        style={{ flex: `${share.percent} 1 0`, background: SHARE_COLORS[share.key] }}
                    />
                ))}
            </div>
            <ul className="flex flex-wrap gap-x-s4 gap-y-s1 text-t2 text-text-2">
                {shares.map((share) => (
                    <li key={share.key} className="flex items-center gap-s2">
                        <span
                            aria-hidden="true"
                            className="size-2 rounded-[2px]"
                            style={{ background: SHARE_COLORS[share.key] }}
                        />
                        {name(share)} {percent(share)}
                    </li>
                ))}
            </ul>
        </div>
    );
}

function Hero({ groups, total, loading }: { groups: AssetGroup[]; total: bigint | null; loading: boolean }) {
    const shares = assetShares(groups);
    return (
        <section aria-label={t("总资产")} className="card">
            <div className="flex flex-wrap items-end justify-between gap-s4">
                <div className="flex min-h-14 min-w-0 flex-col justify-center gap-s1">
                    <span className="text-t2 text-text-3">{t("总资产")}</span>
                    {total === null && loading ? (
                        <Skeleton className="h-10 w-48" />
                    ) : (
                        <span
                            key={total === null ? "" : total.toString()}
                            className="fade-in text-t7 font-semibold [overflow-wrap:anywhere]"
                        >
                            {total === null ? "—" : formatUsd(total)}
                        </span>
                    )}
                    {total === null && !loading ? <span className="text-t2 text-text-3">{t("暂无价格")}</span> : null}
                </div>
                <div className="grid w-full grid-cols-2 gap-s2 @split:flex @split:w-auto">
                    <LinkButton {...overlayLink("?receive=")} size="lg">
                        <Icon name="receive" />
                        {t("收款")}
                    </LinkButton>
                    <LinkButton to="/send" variant="primary" size="lg">
                        <Icon name="send" />
                        {t("转账")}
                    </LinkButton>
                </div>
            </div>
            {shares.length > 0 ? <Allocation shares={shares} /> : null}
        </section>
    );
}

/** 只有等着用户动手的时候才出现。 */
function TodoStrip() {
    const rows = useAttention();
    if (rows.length === 0) return null;
    return (
        <section aria-label={t("待处理")} className="tint rise rounded-r3 bg-accent-soft px-s4 py-s2 @split:px-s6">
            <ul>
                {rows.map((row) => (
                    <li key={row.key} className="flex min-h-row items-center gap-s3 py-s1">
                        <TokenIcon
                            asset={row.entry}
                            chain={row.entry && !row.entry.native ? networkIcon(row.network) : undefined}
                            size={32}
                        />
                        <span className="flex min-w-0 flex-1 flex-col">
                            <span className="truncate text-t3 font-medium">{row.title}</span>
                            <span className="hidden truncate text-t1 text-text-3 @split:block">{row.party}</span>
                            {/* 手机上放不下标签，状态代替第二行。 */}
                            <span className="truncate text-t1 text-text-2 @split:hidden">{row.status}</span>
                        </span>
                        <span className="hidden shrink-0 @split:block">
                            <Chip tone={row.tone}>{row.status}</Chip>
                        </span>
                        <LinkButton to={row.href} variant="primary" size="sm">
                            {row.action}
                        </LinkButton>
                    </li>
                ))}
            </ul>
        </section>
    );
}

/** 收起的一组。展开后仍然排在持仓后面，不和有余额的混在一起。 */
function Folded({ label, children }: { label: string; children: ReactNode }) {
    const [open, setOpen] = useState(false);
    return (
        <div className="flex flex-col">
            <Disclosure open={open} onToggle={() => setOpen(!open)}>
                {label}
            </Disclosure>
            <Collapse open={open}>{children}</Collapse>
        </div>
    );
}

/** 零余额的展开状态存在本地，下次打开保持原样。 */
function ZeroBalances({ children }: { children: ReactNode }) {
    const saved = useLive(async () => (await db.settings.get("showZeroBalances"))?.value, []);
    const open = saved === true;
    return (
        <div className="flex flex-col">
            <Disclosure open={open} onToggle={() => void saveSetting("showZeroBalances", !open)}>
                {open ? t("收起") : t("展开所有")}
            </Disclosure>
            <Collapse open={open}>{children}</Collapse>
        </div>
    );
}

function Recent() {
    const { ledger } = useFeed();
    return (
        <Section aria-labelledby="h-recent" className="gap-s1">
            <div className="flex items-center justify-between gap-s3 pb-s1">
                <SectionTitle id="h-recent">{t("最近记录")}</SectionTitle>
                <Link to="/history" className="link">
                    {t("全部记录")}
                </Link>
            </div>
            {ledger.length > 0 ? (
                <ul className="stagger">
                    {ledger.slice(0, RECENT).map((row) => (
                        <FeedItem key={row.key} row={row} compact />
                    ))}
                </ul>
            ) : (
                <Empty icon="history">{t("还没有交易记录。")}</Empty>
            )}
        </Section>
    );
}

function StatusRow({ to, label, children }: { to: string; label: string; children: ReactNode }) {
    return (
        <Link
            to={to}
            className="row flex min-h-11 items-center justify-between gap-s3 text-t2 text-text-2 no-underline"
        >
            {label}
            {children}
        </Link>
    );
}

/** 这个浏览器里的数据靠不靠得住。 */
function WalletStatus() {
    const { accounts, addresses, persisted } = useApp();
    const last = useLive(async () => (await db.settings.get("lastBackup"))?.value, []);
    const list = accounts.filter((account) => !isLegacyKind(account.kind));
    const verified = list.filter((account) =>
        addresses.some(
            (item) => item.accountId === account.id && item.change === 0 && item.index === 0 && item.verified,
        ),
    ).length;
    return (
        <Section aria-labelledby="h-status" className="gap-s1">
            <SectionTitle id="h-status" className="pb-s1">
                {t("钱包状态")}
            </SectionTitle>
            <StatusRow to="/device" label={t("账户核对")}>
                <Chip tone={verified === list.length ? "ok" : "warn"}>
                    {verified} / {list.length}
                </Chip>
            </StatusRow>
            <StatusRow to="/settings" label={t("上次备份")}>
                {typeof last === "number" ? dateLabel(last) : <Chip tone="warn">{t("暂无")}</Chip>}
            </StatusRow>
            <StatusRow to="/settings" label={t("本地存储")}>
                <Chip tone={persisted ? "ok" : "warn"}>{persisted ? t("已持久化") : t("未持久化")}</Chip>
            </StatusRow>
        </Section>
    );
}

export default function Assets() {
    const { wallet, accounts, addresses, mode } = useApp();
    const gateway = useGateway();
    const txs = useWalletTxs();

    if (!wallet) {
        return (
            <>
                <PageTitle>{t("资产")}</PageTitle>
                <NoWallet />
            </>
        );
    }

    const rows = buildRows(accounts, addresses, mode, gateway.supported, gateway.balances);
    const groups = groupRows(rows, gateway.prices);
    const held = groups.filter((group) => !group.empty);
    const zero = groups.filter((group) => group.empty);
    // 全是零余额时列表不能是空的，新钱包要看得到自己有哪些币。
    const shown = held.length > 0 ? held : groups;
    const unknown = rows.filter((row) => row.entry === null && row.amount > 0n);
    const failed = gateway.down ? [] : gateway.failures.map((failure) => networkName(failure.network));
    const priced = groups.flatMap((group) => (group.cents === null ? [] : [group.cents]));
    const total = priced.length > 0 ? priced.reduce((sum, cents) => sum + cents, 0n) : null;
    const line = (group: AssetGroup) => (
        <Row key={group.key} group={group} pending={groupPending(group, txs ?? [])} loading={gateway.loading} />
    );

    return (
        <>
            <PageTitle>{t("资产")}</PageTitle>
            {gateway.down ? <GatewayDown /> : null}
            <Hero groups={groups} total={total} loading={gateway.loading} />
            <TodoStrip />

            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <section aria-labelledby="h-held" className="card col-span-12 gap-s1 @4xl:col-span-8">
                    <SectionTitle id="h-held" className="pb-s1">
                        {t("持仓")}
                    </SectionTitle>
                    {failed.length > 0 ? (
                        <Notice tone="warn">
                            {failed.join(t("、"))} {t("余额暂时不可用。")}
                        </Notice>
                    ) : null}
                    {gateway.discoveryFailed ? <Notice tone="warn">{t("Bitcoin 余额可能不全。")}</Notice> : null}
                    {shown.length > 0 ? (
                        <ul className="stagger">{shown.map(line)}</ul>
                    ) : gateway.loading ? (
                        <SkeletonRows />
                    ) : (
                        <Empty icon={gateway.down ? "offline" : "assets"}>
                            {gateway.down ? t("余额暂时不可用。") : t("还没有资产。")}
                        </Empty>
                    )}
                    {zero.length > 0 && held.length > 0 ? (
                        <ZeroBalances>
                            <ul>{zero.map(line)}</ul>
                        </ZeroBalances>
                    ) : null}
                    {unknown.length > 0 ? (
                        <Folded label={t("{0} 个未识别代币", unknown.length)}>
                            <Notice tone="warn">{t("不在内置清单里，不能转账，也不计入总资产。")}</Notice>
                            <ul>
                                {unknown.map((row) => (
                                    <UnknownRow key={row.id} row={row} />
                                ))}
                            </ul>
                        </Folded>
                    ) : null}
                </section>

                <div className="col-span-12 flex min-w-0 flex-col gap-s4 @split:gap-s5 @4xl:col-span-4">
                    <Recent />
                    <WalletStatus />
                </div>
            </div>
        </>
    );
}

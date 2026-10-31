import { Link, useParams } from "react-router";
import { isLegacyKind, networkName } from "../core";
import { formatUnits, formatUsd } from "../core/amount";
import { shortAddress } from "../core/text";
import { useApp } from "../data/app";
import {
    buildRows,
    groupAmount,
    groupRows,
    largestRow,
    rowCents,
    sendLink,
    type AssetGroup,
    type ManifestRow,
} from "../gateway/assets";
import { useFeed } from "../gateway/feed";
import type { PriceMap } from "../gateway/client";
import { useGateway } from "../gateway/provider";
import { FeedItem } from "../ui/FeedList";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon } from "../ui/Icon";
import { accountKind } from "../ui/labels";
import { ChildTitle, Empty, LinkButton, Section, SectionTitle, SkeletonRows } from "../ui/primitives";
import { receiveLink } from "../ui/Sheet";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
import { t } from "../core/i18n";

const RECORDS = 5;

function rowTitle({ account, network }: ManifestRow): string {
    return account.kind === "single" ? networkName(network) : `${networkName(network)} ${accountKind(account)}`;
}

function Position({ row, group, prices }: { row: ManifestRow; group: AssetGroup; prices: PriceMap | null }) {
    const { entry } = group;
    const cents = rowCents(row, prices);
    return (
        <li className="row flex min-h-row flex-wrap items-center gap-x-s3 gap-y-s1 py-s2">
            <TokenIcon network={networkIcon(row.network)} size={32} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-t3 font-medium">{rowTitle(row)}</span>
                <span className="truncate font-mono text-t1 text-text-3">{shortAddress(row.address)}</span>
            </span>
            <span className="flex min-w-0 flex-col items-end text-right">
                <span className="text-t3">
                    {row.loaded ? (
                        formatUnits(row.amount, entry.decimals, { min: entry.minFraction })
                    ) : (
                        <span className="text-text-3">—</span>
                    )}
                </span>
                {cents === null ? null : <span className="text-t2 text-text-3">{formatUsd(cents)}</span>}
            </span>
            <span className="flex shrink-0 gap-s1">
                {isLegacyKind(row.account.kind) ? null : (
                    <LinkButton
                        {...receiveLink(row.account.id, { asset: group.key })}
                        variant="ghost"
                        size="sm"
                        aria-label={t("从 {0} 收款", rowTitle(row))}
                    >
                        {t("收款")}
                    </LinkButton>
                )}
                <LinkButton to={sendLink(row)} variant="ghost" size="sm" aria-label={t("从 {0} 转账", rowTitle(row))}>
                    {t("转账")}
                </LinkButton>
            </span>
        </li>
    );
}

function Detail({ group, prices }: { group: AssetGroup; prices: PriceMap | null }) {
    const { ledger } = useFeed();
    const { entry } = group;
    const top = largestRow(group);
    const held = group.rows
        .filter((row) => !row.loaded || row.amount > 0n)
        .sort((a, b) => (groupAmount(group, b) > groupAmount(group, a) ? 1 : -1));
    const records = ledger.filter((row) => row.entry?.icon === group.key);
    return (
        <>
            <section aria-label={entry.symbol} className="card">
                <div className="flex flex-wrap items-center gap-s4">
                    <TokenIcon asset={entry} size={48} />
                    <div className="flex min-w-0 flex-1 flex-col">
                        <span className="text-t7 font-semibold [overflow-wrap:anywhere]">
                            {formatUnits(group.amount, group.decimals, { min: entry.minFraction })} {entry.symbol}
                        </span>
                        <span className="text-t3 text-text-2">
                            {group.cents === null ? t("暂无价格") : formatUsd(group.cents)}
                        </span>
                    </div>
                    <div className="grid w-full grid-cols-2 gap-s2 @split:flex @split:w-auto">
                        {isLegacyKind(top.account.kind) ? null : (
                            <LinkButton {...receiveLink(top.account.id, { asset: group.key })} size="lg">
                                <Icon name="receive" />
                                {t("收款")}
                            </LinkButton>
                        )}
                        {group.amount > 0n ? (
                            <LinkButton to={sendLink(top)} variant="primary" size="lg">
                                <Icon name="send" />
                                {t("转账")}
                            </LinkButton>
                        ) : null}
                    </div>
                </div>
            </section>

            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <Section aria-labelledby="h-where" className="col-span-12 gap-s1 @4xl:col-span-7">
                    <SectionTitle id="h-where" className="pb-s1">
                        {t("分布")}
                    </SectionTitle>
                    {held.length > 0 ? (
                        <ul className="stagger">
                            {held.map((row) => (
                                <Position key={row.id} row={row} group={group} prices={prices} />
                            ))}
                        </ul>
                    ) : (
                        <Empty icon="assets">{t("还没有余额。")}</Empty>
                    )}
                </Section>
                <Section aria-labelledby="h-records" className="col-span-12 gap-s1 @4xl:col-span-5">
                    <div className="flex items-center justify-between gap-s3 pb-s1">
                        <SectionTitle id="h-records">{t("记录")}</SectionTitle>
                        <Link to="/history" className="link">
                            {t("全部记录")}
                        </Link>
                    </div>
                    {records.length > 0 ? (
                        <ul className="stagger">
                            {records.slice(0, RECORDS).map((row) => (
                                <FeedItem key={row.key} row={row} compact />
                            ))}
                        </ul>
                    ) : (
                        <Empty icon="history">{t("还没有交易记录。")}</Empty>
                    )}
                </Section>
            </div>
        </>
    );
}

export default function AssetDetail() {
    const { asset } = useParams();
    const { accounts, addresses, mode } = useApp();
    const gateway = useGateway();
    const rows = buildRows(accounts, addresses, mode, gateway.supported, gateway.balances);
    const group = groupRows(rows, gateway.prices).find((item) => item.key === asset);
    const parent = { to: "/", label: t("资产") };

    return (
        <>
            <ChildTitle parent={parent}>{group?.entry.symbol ?? t("资产")}</ChildTitle>
            {gateway.down ? <GatewayDown /> : null}
            {group ? (
                <Detail group={group} prices={gateway.prices} />
            ) : gateway.loading ? (
                <Section>
                    <SkeletonRows />
                </Section>
            ) : (
                <Empty icon="assets">{t("没有找到这个资产。")}</Empty>
            )}
        </>
    );
}

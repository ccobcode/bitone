import { useState } from "react";
import { Link } from "react-router";
import { networkName } from "../core";
import { formatUnits, formatUsd, usdCents } from "../core/amount";
import { formatAsset, priceKey } from "../core/manifest";
import { useApp, useLive } from "../data/app";
import { db, type TxRecord } from "../data/db";
import { buildRows, pendingAmount, type AssetRow } from "../gateway/assets";
import type { PriceMap } from "../gateway/client";
import { useGateway } from "../gateway/provider";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon } from "../ui/Icon";
import { accountKind } from "../ui/labels";
import { Button, Empty, Helper, LinkButton, PageTitle, Skeleton, SkeletonRows } from "../ui/primitives";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
import { t } from "../core/i18n";

/** 表格最多显示 8 位小数，完整金额放在 title 里。 */
const TABLE_FRACTION = 8;

function Amount({ row, loading }: { row: AssetRow; loading: boolean }) {
    if (!row.loaded) return loading ? <Skeleton className="my-1 h-4 w-20" /> : <span className="text-text-3">—</span>;
    if (!row.entry) {
        return (
            <span className="[overflow-wrap:anywhere]" title={t("原始整数，小数位未知")}>
                {row.amount.toString()}
            </span>
        );
    }
    const { decimals, minFraction, symbol } = row.entry;
    const text = formatUnits(row.amount, decimals, { min: minFraction, max: TABLE_FRACTION });
    return (
        <span key={text} className="fade-in" title={`${formatUnits(row.amount, decimals)} ${symbol}`}>
            {text}
        </span>
    );
}

/** 这一行的美元估值，单位是分。价格只来自网关，只用于展示。 */
function rowCents(row: AssetRow, prices: PriceMap | null): bigint | null {
    if (!row.entry || !row.loaded) return null;
    const price = prices?.get(priceKey(row.network, row.asset));
    return price ? usdCents(row.amount, row.entry.decimals, price) : null;
}

function Row({
    row,
    prices,
    pending,
    loading,
}: {
    row: AssetRow;
    prices: PriceMap | null;
    pending: bigint;
    loading: boolean;
}) {
    const { entry, account, network } = row;
    const cents = rowCents(row, prices);
    const kind = entry?.native && account.chain === "bitcoin" ? ` ${accountKind(account)}` : "";
    const symbol = entry?.symbol ?? t("未知代币");
    const send = `/send?account=${encodeURIComponent(account.id)}&network=${encodeURIComponent(network)}&asset=${encodeURIComponent(row.asset)}`;
    return (
        <li className="row relative flex min-h-row items-center gap-s3 py-s3">
            <TokenIcon asset={entry} chain={entry && !entry.native ? networkIcon(network) : undefined} size={36} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className={`truncate text-t3 font-semibold ${entry ? "" : "text-warn"}`}>{symbol}</span>
                <span className="truncate text-t2 text-text-3">
                    {networkName(network)}
                    {kind}
                </span>
            </span>
            <span className="flex min-w-0 flex-col items-end text-right">
                <span className="text-t3 font-medium">
                    <Amount row={row} loading={loading} />
                </span>
                {pending > 0n ? (
                    <span className="text-t2 text-warn">
                        {t("待转出")} {formatAsset(network, row.asset, pending)}
                    </span>
                ) : cents !== null ? (
                    <span className="text-t2 text-text-3">{formatUsd(cents)}</span>
                ) : !row.loaded && loading ? (
                    <Skeleton className="my-1 h-3 w-16" />
                ) : null}
            </span>
            <Link to={send} aria-label={t("转账 {0}", symbol)} className="absolute inset-0" />
        </li>
    );
}

/** 只有等着用户动手的时候才出现。 */
function PendingBar({ txs }: { txs: readonly TxRecord[] }) {
    const pending = txs.filter((tx) => tx.state === "signing");
    const latest = pending.at(-1);
    if (!latest) return null;
    return (
        <Link
            to={`/sign?id=${encodeURIComponent(latest.id)}`}
            className="press flex min-h-11 items-center justify-between gap-s3 rounded-r3 bg-accent-soft px-s4 text-t3 font-medium text-accent-text no-underline"
        >
            {pending.length} {t("笔待签名")}
            <Icon name="chevron" size={16} className="-rotate-90" />
        </Link>
    );
}

export default function Assets() {
    const { wallet, accounts, addresses, mode } = useApp();
    const gateway = useGateway();
    const [showAll, setShowAll] = useState(false);
    const txs = useLive(async () => (wallet ? db.txs.where("walletId").equals(wallet.id).toArray() : []), [wallet]);

    if (!wallet) {
        return (
            <>
                <PageTitle>{t("资产")}</PageTitle>
                <NoWallet />
            </>
        );
    }

    const rows = buildRows(accounts, addresses, mode, gateway.supported, gateway.balances);
    const held = rows.filter((row) => !row.loaded || row.amount > 0n);
    const zero = rows.length - held.length;
    // 全是零余额时列表不能是空的，新钱包要看得到自己有哪些币。
    const shown = showAll || held.length === 0 ? rows : held;
    const failed = gateway.down ? [] : gateway.failures.map((failure) => networkName(failure.network));
    const priced = rows.map((row) => rowCents(row, gateway.prices)).filter((cents) => cents !== null);
    const total = priced.length > 0 ? priced.reduce((sum, cents) => sum + cents, 0n) : null;

    return (
        <>
            <h1 className="sr-only">{t("资产")}</h1>
            {gateway.down ? <GatewayDown /> : null}
            <PendingBar txs={txs ?? []} />

            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <section
                    aria-label={t("总资产")}
                    className="card col-span-12 @4xl:sticky @4xl:top-[calc(var(--topbar)+var(--safe-top)+var(--s5))] @4xl:col-span-4"
                >
                    <div className="flex min-h-14 flex-col justify-center gap-s1">
                        {total === null && gateway.loading ? (
                            <Skeleton className="h-10 w-48" />
                        ) : (
                            <span
                                key={total === null ? "" : total.toString()}
                                className="fade-in text-t7 font-semibold [overflow-wrap:anywhere]"
                            >
                                {total === null ? "—" : formatUsd(total)}
                            </span>
                        )}
                        {total === null && !gateway.loading ? (
                            <span className="text-t2 text-text-3">{t("暂无价格")}</span>
                        ) : null}
                    </div>
                    <div className="grid grid-cols-2 gap-s2">
                        <LinkButton to="/send" variant="primary" size="lg">
                            <Icon name="send" />
                            {t("转账")}
                        </LinkButton>
                        <LinkButton to="/receive" size="lg">
                            <Icon name="receive" />
                            {t("收款")}
                        </LinkButton>
                    </div>
                </section>

                <section aria-label={t("持仓")} className="card col-span-12 gap-s1 @4xl:col-span-8">
                    {failed.length > 0 ? (
                        <Helper tone="text-warn">
                            {failed.join(t("、"))} {t("余额暂时不可用。")}
                        </Helper>
                    ) : null}
                    {gateway.discoveryFailed ? <Helper tone="text-warn">{t("Bitcoin 余额可能不全。")}</Helper> : null}
                    {shown.length > 0 ? (
                        <ul className="stagger">
                            {shown.map((row) => (
                                <Row
                                    key={row.id}
                                    row={row}
                                    prices={gateway.prices}
                                    pending={pendingAmount(txs ?? [], row.account.id, row.network, row.asset)}
                                    loading={gateway.loading}
                                />
                            ))}
                        </ul>
                    ) : gateway.loading ? (
                        <SkeletonRows />
                    ) : (
                        <Empty icon={gateway.down ? "offline" : "assets"}>
                            {gateway.down ? t("余额暂时不可用。") : t("还没有资产。")}
                        </Empty>
                    )}
                    {zero > 0 && held.length > 0 ? (
                        <Button
                            variant="ghost"
                            size="sm"
                            className="mt-s1 self-center"
                            onClick={() => setShowAll(!showAll)}
                        >
                            {showAll ? t("收起") : t("显示 {0} 项零余额", zero)}
                        </Button>
                    ) : null}
                </section>
            </div>
        </>
    );
}

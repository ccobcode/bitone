import { useEffect, useState } from "react";
import { Link } from "react-router";
import { saveSetting, useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { streamId } from "../data/feed";
import { useFeed } from "../gateway/feed";
import { failedNames, type FeedRow, type HistoryStream } from "../gateway/history";
import { useGateway } from "../gateway/provider";
import { networkList } from "../core";
import { FeedItem } from "../ui/FeedItem";
import { DownloadingIcon, Icon } from "../ui/Icon";
import { dayLabel } from "../ui/labels";
import { Button, ChildTitle, Empty, Notice, PageTitle, Section, SectionTitle, Segmented } from "../ui/primitives";
import { Picker } from "../ui/Picker";
import { TokenIcon, networkIcon, type NetworkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
import { t } from "../core/i18n";

/** 列表每次多显示这么多行，记录都在本地，不用请求网关。 */
const PAGE_SIZE = 50;

const NETWORKS: { id: "all" | NetworkIcon; label: string }[] = [
    { id: "all", label: t("全部网络") },
    { id: "bitcoin", label: "Bitcoin" },
    { id: "ethereum", label: "Ethereum" },
    { id: "bsc", label: "BSC" },
    { id: "polygon", label: "Polygon" },
    { id: "tron", label: "TRON" },
    { id: "solana", label: "Solana" },
];

type Direction = "all" | FeedRow["direction"];

const DIRECTIONS: { value: Direction; label: string }[] = [
    { value: "all", label: t("全部") },
    { value: "in", label: t("转入") },
    { value: "out", label: t("转出") },
];

/** 按天分组。没有时间的是内存池里的交易，已经在进行中那一组。 */
function byDay(rows: readonly FeedRow[]): { day: string; rows: FeedRow[] }[] {
    const days: { day: string; rows: FeedRow[] }[] = [];
    for (const row of rows) {
        const day = row.time === null ? t("等待打包") : dayLabel(row.time);
        const last = days.at(-1);
        if (last?.day === day) last.rows.push(row);
        else days.push({ day, rows: [row] });
    }
    return days;
}

/** 后台正在取新记录。放在标题旁边不占内容的位置，开始和结束时列表都不跳。 */
function Downloading() {
    return (
        <span role="status" className="fade-in flex shrink-0 items-center gap-s1 text-t2 text-text-3">
            <DownloadingIcon />
            {t("下载中")}
        </span>
    );
}

export default function History() {
    const { wallet, mode } = useApp();
    const gateway = useGateway();
    const { ledger, unsent, downloaded } = useFeed();
    const [network, setNetwork] = useState<"all" | NetworkIcon>("all");
    const [direction, setDirection] = useState<Direction>("all");
    const [limit, setLimit] = useState(PAGE_SIZE);
    const [loadingOlder, setLoadingOlder] = useState(false);

    // 看过交易记录后，顶栏和首页不再提示这些链上失败。
    useEffect(() => void saveSetting("historySeenAt", Date.now()), []);

    const cursors = useLive(() => db.feedCursors.toArray(), []);
    const shown = ledger.filter(
        (row) =>
            (direction === "all" || row.direction === direction) &&
            (network === "all" || networkIcon(row.network) === network),
    );
    const moving = shown.filter((row) => row.moving);
    const settled = shown.filter((row) => !row.moving);
    const visible = settled.slice(0, limit);
    const saved = new Map((cursors ?? []).map((cursor) => [cursor.id, cursor]));
    const cursorOf = (stream: HistoryStream) => (wallet ? saved.get(streamId(wallet.id, stream)) : undefined);
    // 更早的记录还在链上，翻到时才取。
    const older = gateway.feedStreams.filter(
        (stream) => cursorOf(stream)?.older && (network === "all" || networkIcon(stream.network) === network),
    );
    // 第一次下载且没有内容可看时才盖住列表。其余情况只是一个小提示，上游慢也不挡已有的记录。
    const downloading = shown.length === 0 && gateway.syncing && gateway.feedStreams.some((s) => !cursorOf(s));
    const never = gateway.syncFailed.filter(
        (id) => !gateway.feedStreams.some((stream) => stream.network === id && cursorOf(stream)),
    );
    const stale = gateway.syncFailed.filter((id) => !never.includes(id));
    const localOnly = gateway.noHistory.filter((id) =>
        networkList().some((n) => n.id === id && n.testnet === (mode === "testnet")),
    );
    const available = NETWORKS.filter(
        (item) =>
            item.id === "all" ||
            networkList().some((n) => networkIcon(n.id) === item.id && n.testnet === (mode === "testnet")),
    );

    if (!wallet) {
        return (
            <>
                <PageTitle>{t("交易记录")}</PageTitle>
                <NoWallet />
            </>
        );
    }

    const picked = available.find((item) => item.id === network);

    return (
        <>
            <PageTitle aside={gateway.syncing && !downloading ? <Downloading /> : null}>{t("交易记录")}</PageTitle>
            <div className="flex flex-col gap-s4">
                <div className="flex flex-wrap items-center gap-s3">
                    <Segmented<Direction>
                        label={t("按方向筛选")}
                        size="md"
                        value={direction}
                        onChange={setDirection}
                        options={DIRECTIONS}
                    />
                    <Picker
                        id="network-filter"
                        look="filter"
                        label={t("按网络筛选")}
                        value={
                            <span className="flex items-center gap-s2">
                                {picked && picked.id !== "all" ? <TokenIcon network={picked.id} size={16} /> : null}
                                {picked?.label}
                            </span>
                        }
                        items={available.map((item) => ({
                            key: item.id,
                            node: (
                                <span className="flex items-center gap-s2">
                                    {item.id === "all" ? null : <TokenIcon network={item.id} size={16} />}
                                    {item.label}
                                </span>
                            ),
                        }))}
                        onPick={(id) => setNetwork(id as "all" | NetworkIcon)}
                    />
                    {unsent.length > 0 ? (
                        <Link to="/history/unsent" className="link ml-auto inline-flex items-center gap-s1">
                            {t("未上链的请求")} {unsent.length}
                            <Icon name="caret" size={14} />
                        </Link>
                    ) : null}
                </div>

                {stale.length > 0 ? (
                    <Notice tone="warn">{t("{0} 暂时无法更新，显示的是上次同步的记录。", failedNames(stale))}</Notice>
                ) : null}
                {never.length > 0 ? (
                    <Notice tone="warn">{t("{0} 的记录暂时下载不到。", failedNames(never))}</Notice>
                ) : null}
                {localOnly.length > 0 ? (
                    <Notice>{t("{0} 只显示从这里发出的交易。", failedNames(localOnly))}</Notice>
                ) : null}
            </div>

            {moving.length > 0 ? (
                <Section aria-labelledby="h-moving" className="gap-s2">
                    <SectionTitle id="h-moving">{t("进行中")}</SectionTitle>
                    <ul className="stagger">
                        {moving.map((row) => (
                            <FeedItem key={row.key} row={row} />
                        ))}
                    </ul>
                </Section>
            ) : null}

            {moving.length === 0 || visible.length > 0 ? (
                <Section aria-label={t("交易列表")} className="relative min-h-40 gap-s4">
                    {visible.length > 0 ? (
                        byDay(visible).map(({ day, rows }) => (
                            <section key={day} aria-label={day} className="flex flex-col gap-s2">
                                <h2 className="text-t2 text-text-3">{day}</h2>
                                <ul key={`${network}-${direction}`} className="stagger">
                                    {rows.map((row) => (
                                        <FeedItem key={row.key} row={row} />
                                    ))}
                                </ul>
                            </section>
                        ))
                    ) : downloading ? null : (
                        <Empty icon="history">{t("没有符合条件的交易。")}</Empty>
                    )}
                    {downloading ? (
                        <div
                            role="status"
                            className="absolute inset-0 flex flex-col items-center justify-center gap-s1 rounded-r3 bg-surface/90 text-center"
                        >
                            <span className="text-t3 font-medium">{t("正在下载交易记录")}</span>
                            <span className="text-t2 text-text-3">{t("已下载 {0} 条", downloaded)}</span>
                        </div>
                    ) : null}
                </Section>
            ) : null}

            {settled.length > limit || older.length > 0 ? (
                <div className="flex justify-end">
                    <Button
                        disabled={loadingOlder}
                        onClick={async () => {
                            setLoadingOlder(true);
                            if (settled.length <= limit) await gateway.loadOlder(older).catch(() => undefined);
                            setLimit(limit + PAGE_SIZE);
                            setLoadingOlder(false);
                        }}
                    >
                        {t("加载更多")}
                    </Button>
                </div>
            ) : null}
        </>
    );
}

/** 没上链就结束的请求。不是资金流动，单独放，已放弃但签名有效的也在这里一直看得到。 */
export function Unsent() {
    const { unsent } = useFeed();
    return (
        <>
            <ChildTitle parent={{ to: "/history", label: t("交易记录") }}>{t("未上链的请求")}</ChildTitle>
            {unsent.length > 0 ? (
                <Section aria-label={t("未上链的请求")} className="gap-0">
                    <ul className="stagger">
                        {unsent.map((row) => (
                            <FeedItem key={row.key} row={row} compact />
                        ))}
                    </ul>
                </Section>
            ) : (
                <Empty icon="history">{t("没有未上链的请求。")}</Empty>
            )}
        </>
    );
}

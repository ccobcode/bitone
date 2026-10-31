import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router";
import { addressKey } from "../gateway/client";
import { saveSetting, useApp, useLive } from "../data/app";
import { db } from "../data/db";
import type { StateGroup } from "../data/machine";
import { streamId } from "../data/feed";
import { failedNames, mergeFeed, remoteItem, type FeedRow, type HistoryStream } from "../gateway/history";
import { useGateway } from "../gateway/provider";
import { networkName, networkList } from "../core";
import { timeLabel } from "../ui/labels";
import { Button, Chip, Empty, Helper, ListHead, PageTitle, Party, Section, Segmented } from "../ui/primitives";
import { Picker } from "../ui/Picker";
import { TokenIcon, networkIcon, type NetworkIcon } from "../ui/TokenIcon";
import { NoWallet } from "./NoWallet";
import { t, translate } from "../core/i18n";

/** 列表每次多显示这么多行，记录都在本地，不用请求网关。 */
const PAGE_SIZE = 50;
const GRID = "grid grid-cols-[minmax(0,1fr)_auto] gap-x-s4 @split:grid-cols-12 @split:gap-x-s6";

const NETWORKS: { id: "all" | NetworkIcon; label: string }[] = [
    { id: "all", label: t("全部网络") },
    { id: "bitcoin", label: "Bitcoin" },
    { id: "ethereum", label: "Ethereum" },
    { id: "bsc", label: "BSC" },
    { id: "polygon", label: "Polygon" },
    { id: "tron", label: "TRON" },
    { id: "solana", label: "Solana" },
];

const GROUPS: { value: "全部" | StateGroup; label: string }[] = [
    { value: "全部", label: t("全部") },
    { value: "待处理", label: t("待处理") },
    { value: "进行中", label: t("进行中") },
    { value: "已完成", label: t("已完成") },
    { value: "未完成", label: t("未完成") },
];
/** 窄屏放不下五段，改成下拉。下拉的按钮没有标题，「全部」要写明是什么。 */
const GROUP_MENU = [{ value: "全部", label: t("全部状态") }, ...GROUPS.slice(1)];

const NOTE_TONES = { warn: "text-warn", danger: "text-danger", muted: "text-text-3" };

/** 宽屏是操作列里的文字链接，窄屏放不下操作列，盖住整行。 */
function RowAction({ row, className, label }: { row: FeedRow; className: string; label?: string }) {
    if (!row.href) return null;
    return row.external ? (
        <a href={row.href} target="_blank" rel="noreferrer noopener" className={className} aria-label={label}>
            {label ? null : row.action}
        </a>
    ) : (
        <Link to={row.href} className={className} aria-label={label}>
            {label ? null : row.action}
        </Link>
    );
}

function Row({ row }: { row: FeedRow }) {
    return (
        <li className={`${GRID} row min-h-row items-center py-s2`}>
            <span className="row-span-2 flex min-w-0 items-center gap-s3 @split:col-span-3 @split:row-span-1">
                <TokenIcon network={networkIcon(row.network)} size={32} />
                <span className="flex min-w-0 flex-col">
                    <span className="truncate text-t3 font-medium">{row.title}</span>
                    <span className="flex min-w-0 gap-s2 text-t1 text-text-3">
                        <span className="truncate">
                            <Party text={row.party} />
                        </span>
                        {row.note ? (
                            <span className={`truncate ${NOTE_TONES[row.noteTone]}`}>{translate(row.note)}</span>
                        ) : null}
                    </span>
                    {row.remark ? <span className="truncate text-t1 text-text-2">{row.remark}</span> : null}
                </span>
            </span>
            <span className="hidden truncate text-t2 text-text-2 @split:col-span-2 @split:block">
                {networkName(row.network)}
            </span>
            <span className="text-right text-t3 [overflow-wrap:anywhere] @split:col-span-2">{row.amount}</span>
            <span className="flex justify-end @split:col-span-2 @split:justify-start">
                <Chip tone={row.tone}>{row.status}</Chip>
            </span>
            <span className="hidden text-t2 text-text-3 @split:col-span-2 @split:block">
                {row.time === null ? t("等待打包") : timeLabel(row.time)}
            </span>
            <span className="hidden justify-end @split:col-span-1 @split:flex">
                <RowAction row={row} className="link" />
            </span>
            <RowAction row={row} className="absolute inset-0 @split:hidden" label={row.action} />
        </li>
    );
}

export default function History() {
    const { wallet, addresses, mode } = useApp();
    const gateway = useGateway();
    const [network, setNetwork] = useState<"all" | NetworkIcon>("all");
    const [group, setGroup] = useState<"全部" | StateGroup>("全部");
    const [limit, setLimit] = useState(PAGE_SIZE);
    const [loadingOlder, setLoadingOlder] = useState(false);

    // 看过交易记录后，顶栏不再提示这些链上失败。
    useEffect(() => void saveSetting("historySeenAt", Date.now()), []);

    const local = useLive(async () => (wallet ? db.txs.where("walletId").equals(wallet.id).toArray() : []), [wallet]);
    const notes = useLive(() => db.notes.toArray(), []);
    const chain = useLive(
        async () => (wallet ? db.chainTxs.where("walletId").equals(wallet.id).toArray() : []),
        [wallet],
    );
    const cursors = useLive(() => db.feedCursors.toArray(), []);
    const remote = useMemo(() => (chain ?? []).map(remoteItem), [chain]);
    const own = useMemo(() => new Set(addresses.map((row) => addressKey(row.address))), [addresses]);
    const rows = useMemo(() => mergeFeed(local ?? [], remote, own, notes ?? []), [local, remote, own, notes]);
    const shown = rows.filter((row) => {
        if (group !== "全部" && row.group !== group) return false;
        return network === "all" || networkIcon(row.network) === network;
    });
    const visible = shown.slice(0, limit);
    const saved = new Map((cursors ?? []).map((cursor) => [cursor.id, cursor]));
    const cursorOf = (stream: HistoryStream) => (wallet ? saved.get(streamId(wallet.id, stream)) : undefined);
    // 更早的记录还在链上，翻到时才取。
    const older = gateway.feedStreams.filter(
        (stream) => cursorOf(stream)?.older && (network === "all" || networkIcon(stream.network) === network),
    );
    // 第一次下载且没有内容可看时才盖住列表。其余情况只是一个小提示，上游慢也不挡已有的记录。
    const downloading = visible.length === 0 && gateway.syncing && gateway.feedStreams.some((s) => !cursorOf(s));
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
            <PageTitle>{t("交易记录")}</PageTitle>
            <div className="flex flex-col gap-s4">
                <div className="flex flex-wrap items-center justify-between gap-s3">
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
                    <div className="nav:hidden">
                        <Picker
                            id="status-filter"
                            look="filter"
                            label={t("按状态筛选")}
                            value={GROUP_MENU.find((item) => item.value === group)?.label}
                            items={GROUP_MENU.map((item) => ({ key: item.value, node: item.label }))}
                            onPick={(value) => setGroup(value as "全部" | StateGroup)}
                        />
                    </div>
                    <div className="hidden nav:block">
                        <Segmented<"全部" | StateGroup>
                            label={t("按状态筛选")}
                            size="md"
                            value={group}
                            onChange={setGroup}
                            options={GROUPS}
                        />
                    </div>
                </div>

                {gateway.syncing && !downloading ? <Helper>{t("同步中")}</Helper> : null}
                {stale.length > 0 ? (
                    <Helper tone="text-warn">
                        {t("{0} 暂时无法更新，显示的是上次同步的记录。", failedNames(stale))}
                    </Helper>
                ) : null}
                {never.length > 0 ? (
                    <Helper tone="text-warn">{t("{0} 的记录暂时下载不到。", failedNames(never))}</Helper>
                ) : null}
                {localOnly.length > 0 ? (
                    <Helper>{t("{0} 只显示从这里发出的交易。", failedNames(localOnly))}</Helper>
                ) : null}
            </div>

            <Section aria-label={t("交易列表")} className="relative min-h-40 gap-0">
                {visible.length > 0 ? (
                    <div>
                        <ListHead grid={GRID}>
                            <span className="col-span-3">{t("交易")}</span>
                            <span className="col-span-2">{t("网络")}</span>
                            <span className="col-span-2 text-right">{t("金额")}</span>
                            <span className="col-span-2">{t("状态")}</span>
                            <span className="col-span-2">{t("时间")}</span>
                            <span className="col-span-1 text-right">{t("操作")}</span>
                        </ListHead>
                        <ul key={`${network}-${group}`} className="stagger">
                            {visible.map((row) => (
                                <Row key={row.key} row={row} />
                            ))}
                        </ul>
                    </div>
                ) : downloading ? null : (
                    <Empty icon="history">{t("没有符合条件的交易。")}</Empty>
                )}
                {downloading ? (
                    <div
                        role="status"
                        className="absolute inset-0 flex flex-col items-center justify-center gap-s1 rounded-r3 bg-surface/90 text-center"
                    >
                        <span className="text-t3 font-medium">{t("正在下载交易记录")}</span>
                        <span className="text-t2 text-text-3">{t("已下载 {0} 条", (chain ?? []).length)}</span>
                    </div>
                ) : null}
            </Section>

            {shown.length > limit || older.length > 0 ? (
                <div className="flex justify-end">
                    <Button
                        disabled={loadingOlder}
                        onClick={async () => {
                            setLoadingOlder(true);
                            if (shown.length <= limit) await gateway.loadOlder(older).catch(() => undefined);
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

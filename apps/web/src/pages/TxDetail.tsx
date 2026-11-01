import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router";
import { chainOf, explorerLink } from "../chains";
import { TransferError, type ReplaceMode } from "../chains/transfer";
import { networkName } from "../core";
import { lookupAsset } from "../core/manifest";
import { cleanText, shortAddress } from "../core/text";
import { useApp, useLive } from "../data/app";
import { db, txKey, type ChainEntry, type TxRecord } from "../data/db";
import { STATE_LABELS, isOverdue } from "../data/machine";
import { broadcastTx, createReplacement, resendTx } from "../data/transfer";
import {
    amountText,
    detailFacts,
    finality,
    nativeText,
    replaceLabel,
    retryLink,
    sequenceFact,
    timelineSteps,
} from "../data/txview";
import { deleteRequest, findTx, isDeletable, releaseAbandoned } from "../data/txs";
import { addressKey, fetchTxs } from "../gateway/client";
import { markSeen } from "../gateway/feed";
import { TONES, ledgerRows, localTitle, recordHref, remoteItem, txHref } from "../gateway/history";
import { useGateway } from "../gateway/provider";
import { ActionBlock } from "../ui/ActionBlock";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon, type IconName } from "../ui/Icon";
import { clockLabel, timeLabel } from "../ui/labels";
import {
    BigAmount,
    Button,
    ChildTitle,
    Chip,
    Empty,
    Fact,
    FullAddress,
    Helper,
    LABEL,
    LinkButton,
    ListRow,
    Notice,
    Party,
    Section,
    SectionTitle,
    SkeletonRows,
} from "../ui/primitives";
import { Confirm } from "../ui/Sheet";
import { SignedTx } from "../ui/SignedTx";
import { StateCard } from "../ui/StateCard";
import { AssetIcon } from "../ui/TokenIcon";
import { t, translate } from "../core/i18n";

const MAX_REMARK = 200;

const HEADS: Record<TxRecord["state"], { icon: IconName; tone: string }> = {
    signing: { icon: "qr", tone: "bg-accent-soft text-accent-text" },
    signed: { icon: "check", tone: "bg-accent-soft text-accent-text" },
    broadcasting: { icon: "send", tone: "bg-accent-soft text-accent-text" },
    broadcast: { icon: "send", tone: "bg-accent-soft text-accent-text" },
    confirming: { icon: "clock", tone: "bg-accent-soft text-accent-text" },
    confirmed: { icon: "check", tone: "bg-ok-soft text-ok" },
    uncertain: { icon: "unknown", tone: "bg-warn-soft text-warn" },
    failed: { icon: "close", tone: "bg-danger-soft text-danger" },
    replaced: { icon: "close", tone: "bg-surface-3 text-text-2" },
    expired: { icon: "clock", tone: "bg-surface-3 text-text-2" },
    abandoned: { icon: "close", tone: "bg-surface-3 text-text-2" },
};

function LiveState() {
    const { socket, down } = useGateway();
    if (down) return <Chip tone="danger">{t("已断开")}</Chip>;
    if (socket !== "live") return <Chip tone="warn">{t("定时刷新")}</Chip>;
    return (
        <Chip tone="ok">
            <span aria-hidden="true" className="size-2 rounded-full bg-ok" />
            {t("实时更新")}
        </Chip>
    );
}

/** 一条分段进度.走到的段填满,正在等的那段呼吸. */
function FinalityMeter({ tx }: { tx: TxRecord }) {
    const { labels, reached, count } = finality(tx);
    const waiting = tx.state !== "confirmed" && tx.state !== "failed";
    return (
        <ol aria-label={t("最终性阶段")} className="flex gap-s2">
            {labels.map((label, i) => (
                <li key={label} className="flex min-w-0 flex-1 flex-col gap-s2">
                    <span aria-hidden="true" className="block h-1 overflow-hidden rounded-rf bg-surface-3">
                        <span
                            className={`meter-fill block h-full rounded-rf ${
                                i < reached
                                    ? "w-full bg-accent"
                                    : waiting && i === reached
                                      ? "meter-live w-full"
                                      : "w-0"
                            }`}
                        />
                    </span>
                    {count ? null : (
                        <span className={`text-t2 transition-colors ${i < reached ? "text-text" : "text-text-3"}`}>
                            {label}
                        </span>
                    )}
                </li>
            ))}
        </ol>
    );
}

interface AmountHeadProps {
    icon: ReactNode;
    amount: string;
    party: ReactNode;
    /** 状态,放在金额右边. */
    children: ReactNode;
}

/** 金额,对方和状态.窄屏时金额独占一行,和冷钱包的核对页一样,数字不会被状态挤断. */
function AmountHead({ icon, amount, party, children }: AmountHeadProps) {
    return (
        <div className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-s4 gap-y-s3 @split:grid-cols-[auto_minmax(0,1fr)_auto]">
            {icon}
            <div className="flex items-center justify-end gap-s3 @split:col-start-3 @split:row-start-1">{children}</div>
            <div className="col-span-2 flex min-w-0 flex-col @split:col-span-1 @split:col-start-2 @split:row-start-1">
                <BigAmount>{amount}</BigAmount>
                <span className="truncate text-t3 text-text-2">{party}</span>
            </div>
        </div>
    );
}

/** 这一屏的主角:这笔转账和它走到哪了. */
function Hero({ tx, tracking }: { tx: TxRecord; tracking: boolean }) {
    const { icon, tone } = HEADS[tx.state];
    const entry = lookupAsset(tx.network, tx.asset);
    const { progress, goal } = finality(tx);
    return (
        <Section aria-labelledby={tracking ? "h-conf" : undefined} className="flex flex-col gap-s5">
            <AmountHead
                icon={<AssetIcon entry={entry} network={tx.network} size={48} />}
                amount={amountText(tx)}
                party={
                    <>
                        {t("至")} <span className="font-mono">{shortAddress(tx.to)}</span>
                    </>
                }
            >
                <Chip tone={tx.reverted ? "danger" : TONES[tx.state]}>{STATE_LABELS[tx.state]}</Chip>
                <span
                    key={tx.state}
                    className={`flex size-12 items-center justify-center rounded-full ${tone} ${
                        tx.state === "confirmed" || tx.state === "failed" ? "ring-once" : "rise"
                    }`}
                >
                    <Icon name={icon} size={24} draw={tx.state === "confirmed"} />
                </span>
            </AmountHead>
            {tracking ? (
                <div className="flex flex-col gap-s3">
                    <div className="flex flex-wrap items-center justify-between gap-x-s4 gap-y-s1">
                        <h2 id="h-conf" className="flex items-baseline gap-s3 text-t3 font-medium">
                            {t("最终性")}
                            <span key={progress} className="fade-in font-normal text-text-2">
                                {progress}
                            </span>
                        </h2>
                        <LiveState />
                    </div>
                    <FinalityMeter tx={tx} />
                    {tx.state === "confirmed" ? null : <Helper>{goal}</Helper>}
                </div>
            ) : null}
            {tx.hash ? <HashLine network={tx.network} hash={tx.hash} /> : null}
        </Section>
    );
}

const STEP_DOTS = {
    done: "bg-text-3",
    current: "bg-accent",
    todo: "bg-line-strong",
};

/** 时间线只留一行一步,说明跟在标题后面. */
function Timeline({ tx }: { tx: TxRecord }) {
    const steps = timelineSteps(tx);
    const { goal } = finality(tx);
    const live = tx.state === "broadcasting" || tx.state === "broadcast" || tx.state === "confirming";
    const end = tx.state === "confirmed" ? "bg-ok" : STEP_DOTS.current;
    return (
        <Section aria-labelledby="h-time" className="col-span-12 @split:col-span-5">
            <SectionTitle id="h-time">{t("进度")}</SectionTitle>
            <ol className="stagger flex flex-col">
                {steps.map((step) => (
                    <li
                        key={`${step.title}-${step.time}`}
                        className="flex min-h-8 flex-wrap items-center gap-x-s3 py-s1 text-t2"
                    >
                        <span
                            aria-hidden="true"
                            className={`size-2 shrink-0 rounded-full ${step.phase === "current" ? end : STEP_DOTS[step.phase]} ${
                                live && step.phase === "current" ? "step-live" : ""
                            }`}
                        />
                        <span
                            className={
                                step.phase === "todo" ? "text-text-3" : step.phase === "current" ? "font-medium" : ""
                            }
                        >
                            {step.title}
                        </span>
                        {step.note && step.note !== goal ? (
                            <span className="min-w-0 text-text-3 [overflow-wrap:anywhere]">
                                {cleanText(step.note, 200)}
                            </span>
                        ) : null}
                        {step.time === null ? null : (
                            <span className="ml-auto text-text-3">{clockLabel(step.time)}</span>
                        )}
                    </li>
                ))}
            </ol>
        </Section>
    );
}

function Details({ tx }: { tx: TxRecord }) {
    return (
        <Section aria-labelledby="h-detail" className="col-span-12 @split:col-span-7">
            <SectionTitle id="h-detail">{t("交易详情")}</SectionTitle>
            <dl className="flex flex-col text-t2">
                {detailFacts(tx).map((fact) => (
                    <Fact key={fact.label} label={fact.label} mono={fact.mono}>
                        {fact.value}
                    </Fact>
                ))}
            </dl>
            {tx.raw ? <SignedTx raw={tx.raw} /> : null}
            {tx.hash ? <Remark id={txKey(tx.network, tx.hash)} /> : null}
        </Section>
    );
}

/** 备注只保存在浏览器,随备份导出. */
function Remark({ id }: { id: string }) {
    const note = useLive(async () => (await db.notes.get(id)) ?? null, [id]);
    const [text, setText] = useState<string | null>(null);
    const value = text ?? note?.text ?? "";

    async function save() {
        const clean = cleanText(value.trim(), MAX_REMARK);
        if (clean) await db.notes.put({ id, text: clean, updatedAt: Date.now() });
        else await db.notes.delete(id);
    }

    return (
        <div className="flex flex-col gap-s2">
            <label htmlFor="remark" className={LABEL}>
                {t("备注")}
            </label>
            <input
                id="remark"
                type="text"
                maxLength={MAX_REMARK}
                value={value}
                onChange={(event) => setText(event.target.value)}
                onBlur={() => void save()}
                className="field text-t3"
            />
        </div>
    );
}

function useReplace(tx: TxRecord) {
    const { wallet, accounts } = useApp();
    const navigate = useNavigate();
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");

    async function replace(mode: ReplaceMode) {
        const account = accounts.find((item) => item.id === tx.accountId);
        if (!wallet || !account) return;
        setBusy(true);
        setFailure("");
        try {
            const next = await createReplacement(tx, mode, { wallet, accounts, account });
            await navigate(`/sign/${encodeURIComponent(next.id)}`);
        } catch (error) {
            setFailure(
                error instanceof TransferError ? translate(error.message) : t("暂时无法生成新的请求,请稍后重试."),
            );
            setBusy(false);
        }
    }

    return { busy, failure, replace };
}

/** 已广播还没打包时能做什么,由这条链决定. */
function BroadcastActions({ tx }: { tx: TxRecord }) {
    const actions = useReplace(tx);
    const { BroadcastCard } = chainOf(tx.chain);
    return <BroadcastCard {...actions} tx={tx} overdue={isOverdue(tx, Date.now())} resend={() => resendTx(tx)} />;
}

function UncertainCard({ tx }: { tx: TxRecord }) {
    const { down } = useGateway();
    const { busy, failure, replace } = useReplace(tx);
    const [sending, setSending] = useState(false);
    const sequence = sequenceFact(tx);
    const { replace: replacement, view } = chainOf(tx.chain);
    const rebuild = "build" in replacement;
    const body = view.uncertainBody(sequence);

    async function retry() {
        setSending(true);
        await broadcastTx(tx.id).catch(() => undefined);
        setSending(false);
    }

    return (
        <StateCard
            tone="warn"
            icon="unknown"
            title={t("广播结果不明")}
            body={body}
            facts={[
                { label: t("网关返回"), value: cleanText(translate(tx.lastError ?? "超时"), 80) },
                sequence,
                { label: t("链上记录"), value: down ? t("暂时无法查询") : t("未查到") },
            ]}
            actions={
                <>
                    {failure ? (
                        <Notice tone="danger" className="mr-auto">
                            {failure}
                        </Notice>
                    ) : null}
                    {rebuild ? (
                        <>
                            <Button disabled={busy || sending} onClick={() => void replace("cancel")}>
                                {t("取消")}
                            </Button>
                            <Button disabled={busy || sending} onClick={() => void replace("rebuild")}>
                                {t("重建交易")}
                            </Button>
                        </>
                    ) : null}
                    <Button variant="primary" disabled={busy || sending} onClick={() => void retry()}>
                        {t("重试广播")}
                    </Button>
                </>
            }
        />
    );
}

function feeText(tx: TxRecord): string {
    return tx.fee ? nativeText(tx, BigInt(tx.fee)) : "—";
}

/** 执行失败但还没到最终阶段.重组后结果可能变,这时不给重新发起的入口. */
function RevertingCard({ tx }: { tx: TxRecord }) {
    return (
        <StateCard
            tone="danger"
            icon="close"
            title={t("执行失败")}
            body={t("手续费已扣除,等待最终确认后再决定是否重新发起.")}
            facts={[
                { label: t("手续费"), value: feeText(tx) },
                { label: t("区块"), value: tx.block === null ? "—" : tx.block.toLocaleString("en-US") },
            ]}
        />
    );
}

function FailedCard({ tx }: { tx: TxRecord }) {
    const fee = feeText(tx);
    return (
        <StateCard
            tone="danger"
            icon="close"
            title={t("链上失败")}
            body={chainOf(tx.chain).view.failedBody()}
            facts={[
                { label: t("手续费"), value: fee },
                { label: t("区块"), value: tx.block === null ? "—" : tx.block.toLocaleString("en-US") },
            ]}
            actions={
                <LinkButton variant="primary" to={retryLink(tx)}>
                    {t("重新发起")}
                </LinkButton>
            }
        />
    );
}

/** 轮询记下的序号证据.数据来自网关,可能是假的,所以只显示,释放由用户决定. */
function spentFacts({ conflicts, conflictAt }: Pick<TxRecord, "conflicts" | "conflictAt">) {
    if (conflicts === 0 || conflictAt === undefined) {
        return [{ label: t("序号状态"), value: t("网关没有报告已用掉") }];
    }
    return [
        { label: t("序号状态"), value: t("网关报告已用掉") },
        { label: t("连续报告轮数"), value: String(conflicts) },
        { label: t("最近报告"), value: timeLabel(conflictAt) },
    ];
}

/** 确认前再看一遍证据. */
function ReleaseSequence({ tx }: { tx: TxRecord }) {
    return (
        <Confirm
            trigger={t("释放序号")}
            title={t("确认序号已经用掉")}
            cancel={t("取消")}
            actions={
                <Button variant="danger" onClick={() => void releaseAbandoned(tx.id)}>
                    {t("释放序号")}
                </Button>
            }
        >
            <dl className="flex flex-col text-t2">
                {spentFacts(tx).map((fact) => (
                    <Fact key={fact.label} label={fact.label}>
                        {fact.value}
                    </Fact>
                ))}
            </dl>
            {tx.conflicts === 0 ? <Notice tone="warn">{t("网关没有报告已用掉,这笔仍可能上链.")}</Notice> : null}
            <Helper>
                {t("这些数据来自网关,不一定可靠,请先在区块浏览器核对.释放后这笔签名如果还有效,可能抢在新交易之前上链.")}
            </Helper>
        </Confirm>
    );
}

/** 已取消但签名仍然有效,提示发替换交易才算作废.序号被用掉时由用户确认释放. */
function AbandonedCard({ tx }: { tx: TxRecord }) {
    const { busy, failure, replace } = useReplace(tx);
    const held = useLive(() => db.reservations.where("txIds").equals(tx.id).count(), [tx.id]);
    const label = replaceLabel(tx);
    if (held === 0) {
        return (
            <StateCard
                tone="warn"
                icon="alert"
                title={t("取消只影响本地记录")}
                body={t("这笔交易已经签名,签名可能仍然有效,序号不再占用.")}
                facts={[]}
            />
        );
    }
    return (
        <StateCard
            tone="warn"
            icon="alert"
            title={t("取消只影响本地记录")}
            body={t("这笔交易已经签名,签名仍然有效,序号会一直占用.")}
            facts={spentFacts(tx)}
            actions={
                <>
                    {failure ? (
                        <Notice tone="danger" className="mr-auto">
                            {failure}
                        </Notice>
                    ) : null}
                    {label ? (
                        <Button disabled={busy} onClick={() => void replace("cancel")}>
                            {label}
                        </Button>
                    ) : null}
                    <ReleaseSequence tx={tx} />
                </>
            }
        />
    );
}

const CLOSED: Partial<Record<TxRecord["state"], { title: string; body: string }>> = {
    replaced: { title: t("已替换"), body: t("同一序号的另一笔交易已经上链,这笔不会再上链.") },
    expired: { title: t("已过期"), body: t("超过过期时间,这笔没有上链,可以重新发起.") },
    abandoned: { title: t("已放弃"), body: t("这笔交易已从本地取消.") },
};

function HashLine({ network, hash }: { network: string; hash: string }) {
    const link = explorerLink(network, hash);
    const [copied, setCopied] = useState(false);

    return (
        <div className="flex flex-wrap items-center gap-x-s2 gap-y-s1">
            <span className="min-w-0 font-mono text-t2 text-text-2 [overflow-wrap:anywhere]">{hash}</span>
            <Button
                variant="ghost"
                size="iconSm"
                aria-label={t("复制交易哈希")}
                onClick={() => {
                    void navigator.clipboard.writeText(hash).then(
                        () => {
                            setCopied(true);
                            setTimeout(() => setCopied(false), 2000);
                        },
                        () => undefined,
                    );
                }}
            >
                <Icon key={String(copied)} name={copied ? "check" : "copy"} size={16} draw={copied} />
            </Button>
            {link ? (
                <a
                    href={link}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="link inline-flex items-center gap-s1"
                >
                    {t("在区块浏览器打开")}
                    <Icon name="external" size={14} />
                </a>
            ) : null}
        </div>
    );
}

/** 没上链就结束的请求可以删,还占着序号的不出现这个入口. */
function DeleteRequest({ tx }: { tx: TxRecord }) {
    const navigate = useNavigate();
    const held = useLive(() => db.reservations.where("txIds").equals(tx.id).count(), [tx.id]);
    if (held === undefined || !isDeletable(tx, held, Date.now())) return null;

    async function remove() {
        await deleteRequest(tx.id);
        await navigate("/history/unsent", { replace: true });
    }

    return (
        <div className="flex justify-end">
            <Confirm
                trigger={t("删除")}
                title={t("确认删除这条请求")}
                cancel={t("取消")}
                actions={
                    <Button variant="danger" onClick={() => void remove()}>
                        {t("确认删除")}
                    </Button>
                }
            >
                {t("这笔没有上链,删除后这个浏览器不再显示它.")}
            </Confirm>
        </div>
    );
}

const HISTORY = { to: "/history", label: t("交易记录") };
const UNSENT = { to: "/history/unsent", label: t("未上链的请求") };

/** 这个浏览器发起的交易.上过链的和没上链的都在这里看,操作按状态出现. */
function View({ tx }: { tx: TxRecord }) {
    const { down } = useGateway();
    const closed = CLOSED[tx.state];
    const tracking = tx.state === "broadcast" || tx.state === "confirming" || tx.state === "confirmed";

    // 看过这笔失败,顶栏和首页不再提示.
    useEffect(() => {
        if (tx.state === "failed") void markSeen(tx.updatedAt);
    }, [tx.state, tx.updatedAt]);

    // 还在签名的请求不在这里处理,回到签名流程.
    if (tx.state === "signing" || tx.state === "signed") {
        return <Navigate to={`/sign/${encodeURIComponent(tx.id)}`} replace />;
    }

    return (
        <>
            <ChildTitle parent={closed ? UNSENT : HISTORY}>{localTitle(tx)}</ChildTitle>
            <Hero tx={tx} tracking={tracking} />
            {down && tx.state !== "confirmed" ? <GatewayDown /> : null}
            {tx.state === "uncertain" ? <UncertainCard tx={tx} /> : null}
            {tx.state === "failed" ? <FailedCard tx={tx} /> : null}
            {tx.state === "confirming" && tx.reverted ? <RevertingCard tx={tx} /> : null}
            {tx.state === "broadcast" ? <BroadcastActions tx={tx} /> : null}
            {tx.state === "abandoned" && tx.raw ? <AbandonedCard tx={tx} /> : null}
            {closed && !(tx.state === "abandoned" && tx.raw) ? (
                <ActionBlock id="h-closed" title={closed.title} helper={<Helper>{closed.body}</Helper>} />
            ) : null}
            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <Timeline tx={tx} />
                <Details tx={tx} />
            </div>
            <DeleteRequest tx={tx} />
        </>
    );
}

/** 别处发出的交易和转入的记录.只有链上事实,确认数和手续费现查. */
function ChainView({ entries }: { entries: ChainEntry[] }) {
    const { wallet, addresses } = useApp();
    const rows = useMemo(() => {
        const own = new Set(addresses.map((item) => addressKey(item.address)));
        const remote = entries.map((entry) => remoteItem(entry, own));
        return ledgerRows([], remote, own, []);
    }, [entries, addresses]);
    const [row, ...others] = rows;
    const { network, hash, txKey: key } = entries[0] as ChainEntry;
    const entry = entries.find((item) => item.id === row?.key);
    const seen = useQuery({
        queryKey: ["chain", "tx", network, hash],
        queryFn: async () => (await fetchTxs(network, [hash]))[0] ?? null,
    });
    // 这个浏览器曾经发起过这笔,后来判定没上链.
    const stale = useLive(async () => {
        if (!wallet) return null;
        const rows = await db.txs.where("walletId").equals(wallet.id).toArray();
        return rows.find((tx) => tx.hash && txKey(tx.network, tx.hash) === key) ?? null;
    }, [wallet, key]);

    if (!row || !entry) return <Empty icon="search">{t("没有找到这笔交易.")}</Empty>;
    const { confirmations, block, fee } = seen.data ?? { confirmations: 0, block: null, fee: null };
    const party = row.direction === "in" ? entry.from : entry.to;

    return (
        <>
            <ChildTitle parent={HISTORY}>{row.title}</ChildTitle>
            <Section className="flex flex-col gap-s4">
                <AmountHead
                    icon={<AssetIcon entry={row.entry} network={row.network} size={48} />}
                    amount={row.amount}
                    party={
                        <>
                            {row.direction === "in" ? t("来自") : t("至")}{" "}
                            <span className="font-mono">{party ? shortAddress(party) : "—"}</span>
                        </>
                    }
                >
                    <Chip tone={row.tone}>{row.status}</Chip>
                </AmountHead>
                <HashLine network={entry.network} hash={entry.hash} />
            </Section>
            {stale ? (
                <Notice tone="warn">
                    {t("链上查到这笔,本地记录已过时")}{" "}
                    <Link to={`/history/${encodeURIComponent(stale.id)}`} className="link">
                        {t("查看本地记录")}
                    </Link>
                </Notice>
            ) : null}
            {row.note && !stale ? <Notice tone="danger">{row.note}</Notice> : null}
            {row.entry ? null : <Notice tone="warn">{t("这个代币不在内置清单里.")}</Notice>}
            {row.direction === "in" ? <Notice>{t("大额收款请在区块浏览器核对入账.")}</Notice> : null}
            {others.length > 0 ? (
                <Section aria-labelledby="h-others" className="gap-s2">
                    <SectionTitle id="h-others">{t("同一笔交易的其他转账")}</SectionTitle>
                    <ul>
                        {others.map((item) => (
                            <ListRow
                                key={item.key}
                                icon={<AssetIcon entry={item.entry} network={item.network} size={32} />}
                                title={item.title}
                                sub={<Party text={item.party} />}
                                value={
                                    <span className={item.direction === "in" ? "text-t3 text-ok" : "text-t3"}>
                                        {item.amount}
                                    </span>
                                }
                            />
                        ))}
                    </ul>
                </Section>
            ) : null}
            <Section aria-labelledby="h-detail">
                <SectionTitle id="h-detail">{t("交易详情")}</SectionTitle>
                <dl className="flex flex-col text-t2">
                    <Fact label={t("网络")}>{networkName(entry.network)}</Fact>
                    <Fact label={t("时间")}>{entry.time === null ? t("等待打包") : timeLabel(entry.time * 1000)}</Fact>
                    {entry.from ? (
                        <Fact label={t("发送方")}>
                            <FullAddress address={entry.from} className="justify-end" />
                        </Fact>
                    ) : null}
                    {entry.to ? (
                        <Fact label={t("接收方")}>
                            <FullAddress address={entry.to} className="justify-end" />
                        </Fact>
                    ) : null}
                    {entry.asset === "native" ? null : (
                        <Fact label={t("代币合约")} mono>
                            {entry.asset}
                        </Fact>
                    )}
                    {confirmations ? <Fact label={t("确认数")}>{confirmations.toLocaleString("en-US")}</Fact> : null}
                    {block === null ? null : <Fact label={t("区块")}>{block.toLocaleString("en-US")}</Fact>}
                    {/* 转入的手续费是对方付的. */}
                    {fee === null || row.direction === "in" ? null : (
                        <Fact label={t("手续费")}>{nativeText(entry, BigInt(fee))}</Fact>
                    )}
                </dl>
                <Remark id={entry.txKey} />
            </Section>
        </>
    );
}

export default function TxDetail() {
    const { id, network, hash } = useParams();
    const { wallet } = useApp();
    const record = useLive(async (): Promise<{ tx?: TxRecord; entries?: ChainEntry[] } | null> => {
        if (network && hash) {
            const tx = await findTx(network, hash);
            if (tx && txHref(tx) === recordHref(network, hash)) return { tx };
            const key = txKey(network, hash);
            const rows = wallet ? await db.chainTxs.where("walletId").equals(wallet.id).toArray() : [];
            const entries = rows.filter((row) => row.txKey === key);
            if (entries.length > 0) return { entries };
            return tx ? { tx } : null;
        }
        if (!id) return null;
        const tx = await db.txs.get(id);
        if (tx) return { tx };
        const entry = await db.chainTxs.get(id);
        return entry ? { entries: [entry] } : null;
    }, [id, network, hash, wallet]);

    if (record === undefined) {
        return (
            <>
                <ChildTitle parent={HISTORY}>{t("交易详情")}</ChildTitle>
                <Section>
                    <SkeletonRows />
                </Section>
            </>
        );
    }
    if (record === null) {
        return (
            <>
                <ChildTitle parent={HISTORY}>{t("交易详情")}</ChildTitle>
                <Empty icon="search">{t("没有找到这笔交易.")}</Empty>
            </>
        );
    }
    // 旧链接用的是本地编号或者整条记录的标识,都跳到统一的地址.
    if (id) {
        const [entry] = record.entries ?? [];
        const href = entry ? recordHref(entry.network, entry.hash) : record.tx ? txHref(record.tx) : null;
        if (href && href !== `/history/${encodeURIComponent(id)}`) return <Navigate to={href} replace />;
    }
    return record.tx ? <View tx={record.tx} /> : <ChainView entries={record.entries ?? []} />;
}

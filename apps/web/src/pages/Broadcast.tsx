import { useEffect, useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { formatUnits } from "../core/amount";
import { explorerLink } from "../core/explorer";
import { lookupAsset } from "../core/manifest";
import { cleanText, shortAddress } from "../core/text";
import { useApp, useLive } from "../data/app";
import { db, txKey, type TxRecord } from "../data/db";
import { STATE_LABELS, isOverdue } from "../data/machine";
import { TransferError, broadcastTx, createReplacement, resendTx, type ReplaceMode } from "../data/transfer";
import {
    amountText,
    detailFacts,
    finality,
    replaceLabel,
    retryLink,
    sequenceFact,
    timelineSteps,
} from "../data/txview";
import { cancelTx, planOf } from "../data/txs";
import { useGateway } from "../gateway/provider";
import { GatewayDown } from "../ui/GatewayDown";
import { Icon, type IconName } from "../ui/Icon";
import { clockLabel } from "../ui/labels";
import {
    Alert,
    Button,
    Chip,
    Empty,
    Fact,
    Helper,
    LABEL,
    LinkButton,
    PageTitle,
    Section,
    SectionTitle,
    SkeletonRows,
} from "../ui/primitives";
import { StateCard } from "../ui/StateCard";
import { TokenIcon, networkIcon } from "../ui/TokenIcon";
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

/** 一条分段进度。走到的段填满，正在等的那段呼吸。 */
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
                                      ? "meter-live w-full bg-line-strong"
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

/** 这一屏的主角：这笔转账和它走到哪了。 */
function Hero({ tx, tracking }: { tx: TxRecord; tracking: boolean }) {
    const { icon, tone } = HEADS[tx.state];
    const entry = lookupAsset(tx.network, tx.asset);
    const { progress, goal } = finality(tx);
    return (
        <Section aria-labelledby={tracking ? "h-conf" : undefined} className="flex flex-col gap-s5">
            <div className="flex min-w-0 items-center gap-s4">
                <TokenIcon
                    asset={entry}
                    chain={entry && !entry.native ? networkIcon(tx.network) : undefined}
                    size={48}
                />
                <div className="flex min-w-0 flex-1 flex-col">
                    <span className="text-t7 font-semibold [overflow-wrap:anywhere]">{amountText(tx)}</span>
                    <span className="truncate text-t3 text-text-2">
                        {t("至")} <span className="font-mono">{shortAddress(tx.to)}</span>
                    </span>
                </div>
                <span
                    key={tx.state}
                    className={`rise flex size-12 shrink-0 items-center justify-center rounded-full ${tone}`}
                >
                    <Icon name={icon} size={24} draw={tx.state === "confirmed"} />
                </span>
            </div>
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
            <HashLine tx={tx} />
        </Section>
    );
}

const STEP_DOTS = {
    done: "bg-text-3",
    current: "bg-accent",
    todo: "bg-line-strong",
};

/** 时间线只留一行一步，说明跟在标题后面。 */
function Timeline({ tx }: { tx: TxRecord }) {
    const steps = timelineSteps(tx);
    const { goal } = finality(tx);
    return (
        <Section aria-labelledby="h-time" className="col-span-12 @split:col-span-5">
            <SectionTitle id="h-time">{t("进度")}</SectionTitle>
            <ol className="stagger flex flex-col">
                {steps.map((step) => (
                    <li
                        key={`${step.title}-${step.time}`}
                        className="flex min-h-8 flex-wrap items-center gap-x-s3 py-s1 text-t2"
                    >
                        <span aria-hidden="true" className={`size-2 shrink-0 rounded-full ${STEP_DOTS[step.phase]}`} />
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
            {tx.hash ? <Remark id={txKey(tx.network, tx.hash)} /> : null}
        </Section>
    );
}

/** 备注只保存在浏览器，随备份导出。 */
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

/** 需要用户决定的一件事：左边说明，右边按钮。 */
function ActionBlock({
    id,
    title,
    helper,
    tone,
    children,
}: {
    id: string;
    title: string;
    helper: ReactNode;
    tone?: string;
    children?: ReactNode;
}) {
    return (
        <Section aria-labelledby={id} className="rise flex-row flex-wrap items-center justify-end gap-s4">
            <div className="flex min-w-0 flex-[1_1_320px] flex-col gap-s1">
                <SectionTitle id={id} className={tone}>
                    {title}
                </SectionTitle>
                {helper}
            </div>
            {children}
        </Section>
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
            await navigate(`/sign?id=${encodeURIComponent(next.id)}`);
        } catch (error) {
            setFailure(
                error instanceof TransferError ? translate(error.message) : t("暂时无法生成新的请求，请稍后重试。"),
            );
            setBusy(false);
        }
    }

    return { busy, failure, replace };
}

/** Solana 不提供加速。节点丢弃时可以重发同一笔，推进 nonce 可以作废它。 */
function SolanaCard({ tx }: { tx: TxRecord }) {
    const { busy, failure, replace } = useReplace(tx);
    const [notice, setNotice] = useState("");

    async function resend() {
        setNotice("");
        try {
            await resendTx(tx);
            setNotice(t("已重新提交。"));
        } catch {
            setNotice(t("提交失败，请稍后重试。"));
        }
    }

    return (
        <ActionBlock
            id="h-act"
            title={t("不能加速")}
            helper={
                <>
                    <Helper>{t("Solana 不提供加速，推进 nonce 可以作废这笔交易。")}</Helper>
                    {notice ? (
                        <p role="status" className="text-t2 text-text-2">
                            {notice}
                        </p>
                    ) : null}
                    {failure ? <Alert>{failure}</Alert> : null}
                </>
            }
        >
            {planOf(tx, "solana").nonceValue === null ? null : (
                <div className="flex gap-s2">
                    <Button disabled={busy} onClick={() => void replace("cancel")}>
                        {t("推进 nonce")}
                    </Button>
                    <Button variant="primary" onClick={() => void resend()}>
                        {t("重新广播")}
                    </Button>
                </div>
            )}
        </ActionBlock>
    );
}

/** 已广播还没打包时可以加速或取消。TRON 和 Solana 不提供。 */
function SpeedCard({ tx }: { tx: TxRecord }) {
    const { busy, failure, replace } = useReplace(tx);
    const overdue = isOverdue(tx, Date.now());
    if (tx.chain === "tron") {
        return (
            <ActionBlock
                id="h-act"
                title={t("不能加速")}
                helper={<Helper>{t("TRON 不提供加速，超过过期时间后这笔交易自动失效。")}</Helper>}
            />
        );
    }
    return (
        <ActionBlock
            id="h-act"
            title={overdue ? t("等待时间过长") : t("加速或取消")}
            helper={
                <>
                    <Helper tone={overdue ? "text-warn" : "text-text-3"}>
                        {t("两种操作都会生成新的待签名请求，要回到冷钱包签名。")}
                    </Helper>
                    {failure ? <Alert>{failure}</Alert> : null}
                </>
            }
        >
            <div className="flex gap-s2">
                <Button disabled={busy} onClick={() => void replace("cancel")}>
                    {t("取消")}
                </Button>
                <Button variant="primary" disabled={busy} onClick={() => void replace("speedup")}>
                    {t("加速")}
                </Button>
            </div>
        </ActionBlock>
    );
}

function SignedCard({ tx }: { tx: TxRecord }) {
    const [busy, setBusy] = useState(false);

    async function send() {
        setBusy(true);
        await broadcastTx(tx.id).catch(() => undefined);
        setBusy(false);
    }

    return (
        <ActionBlock id="h-send" title={t("核验通过")} helper={<Helper>{t("签名与构建记录一致，可以提交。")}</Helper>}>
            <div className="flex gap-s2">
                <Button disabled={busy} onClick={() => void cancelTx(tx.id, t("用户取消，签名仍然有效"))}>
                    {t("放弃")}
                </Button>
                <Button variant="primary" disabled={busy} onClick={() => void send()}>
                    {t("广播交易")}
                </Button>
            </div>
        </ActionBlock>
    );
}

function UncertainCard({ tx }: { tx: TxRecord }) {
    const { down } = useGateway();
    const { busy, failure, replace } = useReplace(tx);
    const [sending, setSending] = useState(false);
    const sequence = sequenceFact(tx);
    const rebuild = tx.chain !== "tron";
    const body =
        tx.chain === "evm"
            ? t("这笔可能已经上链，先重试广播同一笔，需要重建时必须复用 nonce {0}。", sequence.value)
            : tx.chain === "bitcoin"
              ? t("这笔可能已经上链，先重试广播同一笔，需要重建时必须复用同一组输入。")
              : tx.chain === "solana"
                ? t("这笔可能已经上链，先重试广播同一笔，需要重建时必须复用同一个 nonce 值。")
                : t("这笔可能已经上链，只能重试广播同一笔，超过过期时间后才能重新发起。");

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
                    {failure ? <Alert className="mr-auto">{failure}</Alert> : null}
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
    const native = lookupAsset(tx.network, "native");
    return tx.fee && native ? `${formatUnits(BigInt(tx.fee), native.decimals)} ${native.symbol}` : "—";
}

/** 执行失败但还没到最终阶段。重组后结果可能变，这时不给重新发起的入口。 */
function RevertingCard({ tx }: { tx: TxRecord }) {
    return (
        <StateCard
            tone="danger"
            icon="close"
            title={t("执行失败")}
            body={t("手续费已扣除，等待最终确认后再决定是否重新发起。")}
            facts={[
                { label: t("手续费"), value: feeText(tx) },
                { label: t("区块"), value: tx.block === null ? "—" : tx.block.toLocaleString("en-US") },
            ]}
            actions={<LinkButton to="/history">{t("查看记录")}</LinkButton>}
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
            body={
                tx.chain === "evm"
                    ? t("合约回滚，手续费已扣除，资产没有转出。")
                    : t("交易执行失败，手续费已扣除，资产没有转出。")
            }
            facts={[
                { label: t("手续费"), value: fee },
                { label: t("区块"), value: tx.block === null ? "—" : tx.block.toLocaleString("en-US") },
            ]}
            actions={
                <>
                    <LinkButton to="/history">{t("查看记录")}</LinkButton>
                    <LinkButton variant="primary" to={retryLink(tx)}>
                        {t("重新发起")}
                    </LinkButton>
                </>
            }
        />
    );
}

/** 已取消但签名仍然有效，提示发替换交易才算作废。 */
function AbandonedCard({ tx }: { tx: TxRecord }) {
    const { busy, failure, replace } = useReplace(tx);
    const label = replaceLabel(tx);
    return (
        <section
            aria-labelledby="h-off"
            className="tint rise flex flex-col gap-s3 rounded-r3 bg-warn-soft p-s4 @split:p-s6"
        >
            <SectionTitle id="h-off" className="text-warn">
                {t("取消只影响本地记录")}
            </SectionTitle>
            <Helper tone="text-text-2">{t("这笔交易已经签名，签名仍然有效，序号会一直占用。")}</Helper>
            {failure ? <Alert>{failure}</Alert> : null}
            {label ? (
                <div className="flex justify-end">
                    <Button disabled={busy} onClick={() => void replace("cancel")}>
                        {label}
                    </Button>
                </div>
            ) : null}
        </section>
    );
}

const CLOSED: Partial<Record<TxRecord["state"], { title: string; body: string }>> = {
    replaced: { title: t("已替换"), body: t("同一序号的另一笔交易已经上链，这笔不会再上链。") },
    expired: { title: t("已过期"), body: t("超过过期时间，这笔没有上链，可以重新发起。") },
    abandoned: { title: t("已放弃"), body: t("这笔交易已从本地取消。") },
};

function HashLine({ tx }: { tx: TxRecord }) {
    const link = tx.hash ? explorerLink(tx.network, tx.hash) : null;
    const [copied, setCopied] = useState(false);

    useEffect(() => {
        if (!copied) return;
        const timer = setTimeout(() => setCopied(false), 2000);
        return () => clearTimeout(timer);
    }, [copied]);

    if (!tx.hash) return null;
    return (
        <div className="flex flex-wrap items-center gap-x-s2 gap-y-s1">
            <span className="min-w-0 font-mono text-t2 text-text-2 [overflow-wrap:anywhere]">{tx.hash}</span>
            <Button
                variant="ghost"
                size="iconSm"
                aria-label={t("复制交易哈希")}
                onClick={() => {
                    if (tx.hash)
                        void navigator.clipboard.writeText(tx.hash).then(
                            () => setCopied(true),
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
                    {t("区块浏览器")}
                    <Icon name="external" size={14} />
                </a>
            ) : null}
        </div>
    );
}

function View({ tx }: { tx: TxRecord }) {
    const { down } = useGateway();
    const closed = CLOSED[tx.state];
    const tracking = tx.state === "broadcast" || tx.state === "confirming" || tx.state === "confirmed";
    return (
        <>
            <PageTitle>{STATE_LABELS[tx.state]}</PageTitle>
            <Hero tx={tx} tracking={tracking} />
            {down && tx.state !== "confirmed" ? <GatewayDown /> : null}
            {tx.state === "signed" ? <SignedCard tx={tx} /> : null}
            {tx.state === "uncertain" ? <UncertainCard tx={tx} /> : null}
            {tx.state === "failed" ? <FailedCard tx={tx} /> : null}
            {tx.state === "confirming" && tx.reverted ? <RevertingCard tx={tx} /> : null}
            {tx.state === "broadcast" && tx.chain === "solana" ? <SolanaCard tx={tx} /> : null}
            {tx.state === "broadcast" && tx.chain !== "solana" ? <SpeedCard tx={tx} /> : null}
            {tx.state === "abandoned" && tx.raw ? <AbandonedCard tx={tx} /> : null}
            {closed && !(tx.state === "abandoned" && tx.raw) ? (
                <ActionBlock id="h-closed" title={closed.title} helper={<Helper>{closed.body}</Helper>} />
            ) : null}
            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <Timeline tx={tx} />
                <Details tx={tx} />
            </div>
            <div className="flex flex-wrap justify-end gap-s2">
                <LinkButton to="/">{t("返回资产")}</LinkButton>
                <LinkButton variant="primary" to="/send">
                    {t("再转一笔")}
                </LinkButton>
            </div>
        </>
    );
}

export default function Broadcast() {
    const [params] = useSearchParams();
    const id = params.get("id");
    const tx = useLive(async () => (id ? ((await db.txs.get(id)) ?? null) : null), [id]);
    if (tx === undefined) {
        return (
            <>
                <PageTitle>{t("广播")}</PageTitle>
                <SkeletonRows />
            </>
        );
    }
    if (tx === null) {
        return (
            <>
                <PageTitle>{t("广播")}</PageTitle>
                <Empty icon="search" action={<LinkButton to="/history">{t("查看记录")}</LinkButton>}>
                    {t("没有找到这笔交易。")}
                </Empty>
            </>
        );
    }
    return <View tx={tx} />;
}

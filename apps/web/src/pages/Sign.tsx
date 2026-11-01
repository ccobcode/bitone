import { chainOf } from "../chains";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useNavigate, useParams } from "react-router";
import { UrReceiver, accountKeyList, previewRequest, verifyResponse, type Review } from "../core";
import { useApp, useLive } from "../data/app";
import { db, type TxRecord } from "../data/db";
import { txHref } from "../gateway/history";
import { TransferError } from "../chains/transfer";
import { broadcastTx, replaceAndCancel } from "../data/transfer";
import {
    amountText,
    cancelNote,
    countdownLabel,
    feeFact,
    replaceLabel,
    retryLink,
    sequenceFact,
    verifyChecks,
} from "../data/txview";
import { STATE_LABELS } from "../data/machine";
import { cancelTx, markSigned } from "../data/txs";
import { shortAddress } from "../core/text";
import { lookupAsset } from "../core/manifest";
import { AnimatedQr } from "../ui/AnimatedQr";
import { buzz, CameraPanel, useImageScan, type CameraFailure } from "../ui/Camera";
import { Icon } from "../ui/Icon";
import {
    Button,
    ChildTitle,
    Chip,
    Collapse,
    Disclosure,
    Empty,
    Fact,
    FullAddress,
    LinkButton,
    Notice,
    Section,
    SectionTitle,
    SkeletonRows,
} from "../ui/primitives";
import { ScanProgress } from "../ui/ScanProgress";
import { Confirm } from "../ui/Sheet";
import { SignedTx } from "../ui/SignedTx";
import { StateCard } from "../ui/StateCard";
import { AssetIcon } from "../ui/TokenIcon";
import { t, translate } from "../core/i18n";

const ASSETS = { to: "/", label: t("资产") };

/** 561 是 33 的整数倍,见 docs/06-ui.md 第 4.5 节. */
const QR_SIZE = 561;

function remaining(expiresAt: number, now: number): string {
    const seconds = Math.max(Math.ceil((expiresAt - now) / 1000), 0);
    const pad = (value: number) => String(value).padStart(2, "0");
    return `${pad(Math.floor(seconds / 60))}:${pad(seconds % 60)}`;
}

function useNow(): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, []);
    return now;
}

function verifyFailure(error: unknown): { item: string; recorded: string } {
    const message = error instanceof Error ? error.message : "";
    if (message.includes("不匹配")) return { item: t("请求 ID"), recorded: t("与这笔交易不同") };
    if (message.includes("签名无效") || message.includes("公钥"))
        return { item: t("签名"), recorded: t("与付款账户公钥不符") };
    return { item: t("签名数据"), recorded: t("无法解析") };
}

function Countdown({ tx }: { tx: TxRecord }) {
    const now = useNow();
    return (
        <Chip tone="warn">
            <Icon name="clock" size={14} />
            {countdownLabel(tx)}
            {tx.expiresAt !== null ? <span>{remaining(tx.expiresAt, now)}</span> : null}
        </Chip>
    );
}

function ScanPanel({
    tx,
    onVerified,
    children,
}: {
    tx: TxRecord;
    onVerified(raw: string, hash: string): void;
    children: ReactNode;
}) {
    const { accounts } = useApp();
    const navigate = useNavigate();
    const receiver = useRef<UrReceiver | null>(null);
    const [scanning, setScanning] = useState(false);
    const [failure, setFailure] = useState<CameraFailure | null>(null);
    const [progress, setProgress] = useState({ value: 0, frames: 0 });
    const [notice, setNotice] = useState("");
    const [rejected, setRejected] = useState<{ item: string; recorded: string } | null>(null);
    const [showChecks, setShowChecks] = useState(false);

    const reset = useCallback(() => {
        receiver.current?.free();
        receiver.current = null;
        setProgress({ value: 0, frames: 0 });
        setRejected(null);
        setNotice("");
    }, []);

    useEffect(
        () => () => {
            receiver.current?.free();
            receiver.current = null;
        },
        [],
    );

    const finish = useCallback(
        (message: Uint8Array) => {
            const account = accounts.find((item) => item.id === tx.accountId);
            if (!account) return setNotice(t("找不到付款账户."));
            try {
                const verified = verifyResponse(tx.request, message, chainOf(tx.chain).signerKey(account));
                setScanning(false);
                buzz();
                onVerified(verified.raw, verified.hash);
            } catch (error) {
                setScanning(false);
                setRejected(verifyFailure(error));
            }
        },
        [accounts, onVerified, tx.accountId, tx.chain, tx.request],
    );

    const feed = useCallback(
        (text: string) => {
            receiver.current ??= new UrReceiver("bitone-response");
            const current = receiver.current;
            if (rejected || current.complete) return;
            const result = current.feed(text);
            if (result === "repeated") return;
            if (result === "foreign") return setNotice(t("这不是签名结果二维码."));
            if (result === "invalid") return setNotice(t("这一帧与已收到的数据不一致,可以重新扫描."));
            setNotice("");
            setProgress({ value: current.progress, frames: current.frames });
            if (current.complete) finish(current.message());
        },
        [finish, rejected],
    );

    const images = useImageScan(t("上传签名二维码图片"), feed, setNotice);

    async function abandon() {
        await cancelTx(tx.id, t("核验失败,用户放弃"));
        await navigate(txHref(tx), { replace: true });
    }

    return (
        <Section aria-labelledby="h-scan" className="col-span-12 @split:col-span-5">
            <SectionTitle id="h-scan">{t("扫描签名结果")}</SectionTitle>
            {images.element}

            {rejected ? (
                <StateCard
                    tone="danger"
                    icon="alert"
                    title={t("核验失败")}
                    body={t("签名与构建记录不一致,已拒绝广播.")}
                    facts={[
                        { label: t("不一致项"), value: rejected.item },
                        { label: t("结果"), value: rejected.recorded },
                    ]}
                    actions={
                        <>
                            <Button onClick={() => void abandon()}>{t("放弃交易")}</Button>
                            <Button variant="primary" onClick={reset}>
                                {t("重新签名")}
                            </Button>
                        </>
                    }
                />
            ) : scanning || failure ? (
                <CameraPanel active failure={failure} onFailure={setFailure} onText={feed} onUpload={images.open} />
            ) : (
                <div className="flex aspect-[4/3] flex-col items-center justify-center gap-s2 rounded-r3 bg-camera text-on-camera">
                    <Icon name="camera" size={24} />
                    <span className="text-t2">{t("等待冷钱包签名")}</span>
                </div>
            )}

            {rejected || failure ? null : (
                <div className="flex flex-wrap justify-end gap-s2">
                    <Button onClick={images.open}>{t("上传图片")}</Button>
                    {scanning ? null : (
                        <Button variant="primary" onClick={() => setScanning(true)}>
                            <Icon name="scan" />
                            {t("开启摄像头")}
                        </Button>
                    )}
                </div>
            )}
            <ScanProgress value={progress.value} frames={progress.frames} />
            {notice ? <Notice tone="warn">{notice}</Notice> : null}

            {children}

            <div className="flex flex-col">
                <Disclosure open={showChecks} onToggle={() => setShowChecks(!showChecks)}>
                    {t("收到后自动核对")}
                </Disclosure>
                <Collapse open={showChecks}>
                    <ul className="flex flex-col pt-s2">
                        {verifyChecks(tx).map((check) => (
                            <li
                                key={check.label}
                                className="flex min-h-8 items-baseline justify-between gap-s3 text-t2"
                            >
                                <span className="text-text-3">{check.label}</span>
                                <span className={check.mono ? "font-mono" : ""}>{check.value}</span>
                            </li>
                        ))}
                    </ul>
                </Collapse>
            </div>
        </Section>
    );
}

function CancelPanel({ tx }: { tx: TxRecord }) {
    const { wallet, accounts } = useApp();
    const navigate = useNavigate();
    const [busy, setBusy] = useState(false);
    const [failure, setFailure] = useState("");
    const replace = replaceLabel(tx);

    async function confirm() {
        setBusy(true);
        await cancelTx(tx.id, t("用户取消"));
        await navigate(txHref(tx), { replace: true });
    }

    async function sendReplacement() {
        const account = accounts.find((item) => item.id === tx.accountId);
        if (!wallet || !account) return;
        setBusy(true);
        setFailure("");
        try {
            const next = await replaceAndCancel(tx, { wallet, accounts, account });
            await navigate(`/sign/${encodeURIComponent(next.id)}`);
        } catch (error) {
            setFailure(error instanceof TransferError ? translate(error.message) : t("无法生成替换交易."));
            setBusy(false);
        }
    }

    return (
        <div className="flex justify-end">
            <Confirm
                trigger={t("取消交易")}
                title={t("取消只影响本地记录")}
                cancel={t("继续签名")}
                actions={
                    <>
                        {replace ? (
                            <Button disabled={busy} onClick={() => void sendReplacement()}>
                                {replace}
                            </Button>
                        ) : null}
                        <Button variant="danger" disabled={busy} onClick={() => void confirm()}>
                            {t("确认取消")}
                        </Button>
                    </>
                }
            >
                {cancelNote(tx)}
                {failure ? <Notice tone="danger">{failure}</Notice> : null}
            </Confirm>
        </div>
    );
}

/** 用和冷钱包相同的代码再看一遍.结果只用于提示,判断以冷钱包屏幕为准. */
function usePreview(tx: TxRecord): Review | null | undefined {
    const { accounts } = useApp();
    return useMemo(() => {
        // 账户还没读出来时不下结论.
        if (accounts.length === 0) return undefined;
        try {
            return previewRequest(tx.request, accountKeyList(accounts));
        } catch {
            return null;
        }
    }, [tx.request, accounts]);
}

function SigningView({ tx }: { tx: TxRecord }) {
    const [qrError, setQrError] = useState("");
    const entry = lookupAsset(tx.network, tx.asset);
    const review = usePreview(tx);
    const refused = review?.confirm === "refuse";

    const onVerified = useCallback(
        (raw: string, hash: string) => {
            // 页面跟着状态走,下一步是广播.扫描期间交易可能已经到期,这时切到已结束的状态.
            void markSigned(tx.id, { raw, hash }).catch(() => undefined);
        },
        [tx.id],
    );

    return (
        <>
            <ChildTitle parent={ASSETS}>{t("冷钱包签名")}</ChildTitle>
            <div className="grid grid-cols-12 items-start gap-s4 @split:gap-s5">
                <Section aria-labelledby="h-qr" className="col-span-12 flex min-w-0 flex-col gap-s4 @split:col-span-7">
                    <div className="flex flex-wrap items-center justify-between gap-s2">
                        <SectionTitle id="h-qr">{t("待签名二维码")}</SectionTitle>
                        <div className="flex flex-wrap items-center gap-s2">
                            <Countdown tx={tx} />
                            {review ? (
                                <Chip tone={refused ? "danger" : review.confirm === "tap" ? "ok" : "warn"}>
                                    {review.title}
                                </Chip>
                            ) : null}
                        </div>
                    </div>
                    {review === null ? (
                        <Notice tone="warn">{t("这个请求本机解析不了,判断以冷钱包屏幕为准.")}</Notice>
                    ) : null}
                    {refused ? (
                        <Notice tone="danger">{t("本机预览认为冷钱包会拒绝这个请求,判断以冷钱包屏幕为准.")}</Notice>
                    ) : null}
                    {review?.alerts.map((alert) => (
                        <Notice key={alert} tone="warn">
                            {t("冷钱包会提示:")}
                            {alert}
                        </Notice>
                    ))}
                    {qrError ? (
                        <Notice tone="danger">{qrError}</Notice>
                    ) : (
                        <AnimatedQr
                            message={tx.request}
                            type="request"
                            size={QR_SIZE}
                            label={t("待签名交易动态二维码")}
                            onError={setQrError}
                        />
                    )}
                </Section>
                <ScanPanel tx={tx} onVerified={onVerified}>
                    <dl aria-label={t("交易摘要")} className="flex flex-col text-t2">
                        <Fact label={t("发送")}>
                            <span className="inline-flex items-center gap-s2">
                                <AssetIcon entry={entry} network={tx.network} size={16} />
                                {amountText(tx)}
                            </span>
                        </Fact>
                        {[
                            { label: t("收款"), value: shortAddress(tx.to), mono: true },
                            feeFact(tx),
                            sequenceFact(tx),
                        ].map((fact) => (
                            <Fact key={fact.label} label={fact.label} mono={fact.mono}>
                                {fact.value}
                            </Fact>
                        ))}
                    </dl>
                </ScanPanel>
            </div>

            <CancelPanel tx={tx} />
        </>
    );
}

/** 签名已经核验,等用户确认广播.广播后这笔才算发出去,页面落到交易详情. */
function SignedView({ tx }: { tx: TxRecord }) {
    const [busy, setBusy] = useState(false);

    async function send() {
        setBusy(true);
        await broadcastTx(tx.id).catch(() => undefined);
        setBusy(false);
    }

    const fee = feeFact(tx);
    return (
        <>
            <ChildTitle parent={ASSETS}>{t("广播")}</ChildTitle>
            <Section aria-labelledby="h-send" className="gap-s4">
                <SectionTitle id="h-send">{t("核验通过")}</SectionTitle>
                <dl aria-label={t("交易摘要")} className="flex flex-col text-t2">
                    <Fact label={t("发送")}>{amountText(tx)}</Fact>
                    <Fact label={t("收款")}>
                        {tx.to ? <FullAddress address={tx.to} className="justify-end" /> : "—"}
                    </Fact>
                    <Fact label={fee.label} mono={fee.mono}>
                        {fee.value}
                    </Fact>
                </dl>
                {tx.raw ? <SignedTx raw={tx.raw} /> : null}
                <div className="flex justify-end gap-s2">
                    <Button disabled={busy} onClick={() => void cancelTx(tx.id, t("用户取消,签名仍然有效"))}>
                        {t("放弃")}
                    </Button>
                    <Button variant="primary" disabled={busy} onClick={() => void send()}>
                        {busy ? t("广播中") : t("广播交易")}
                    </Button>
                </div>
            </Section>
        </>
    );
}

function ClosedView({ tx }: { tx: TxRecord }) {
    const expired = tx.timeline.at(-1)?.note === "手续费报价已过期" || tx.timeline.at(-1)?.note === "超过有效期";
    return (
        <>
            <ChildTitle parent={ASSETS}>{t("冷钱包签名")}</ChildTitle>
            <StateCard
                tone="warn"
                icon="clock"
                title={expired ? t("已过期") : STATE_LABELS[tx.state]}
                body={expired ? t("这个请求已经失效,重新构建后用新的二维码签名.") : t("这个请求已经结束,不能继续签名.")}
                facts={[
                    { label: t("编号"), value: tx.id },
                    { label: t("状态"), value: <Chip tone="neutral">{STATE_LABELS[tx.state]}</Chip> },
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
        </>
    );
}

export default function Sign() {
    const { id } = useParams();
    const tx = useLive(async () => (id ? ((await db.txs.get(id)) ?? null) : null), [id]);

    if (tx === undefined) {
        return (
            <>
                <ChildTitle parent={ASSETS}>{t("冷钱包签名")}</ChildTitle>
                <SkeletonRows />
            </>
        );
    }
    if (tx === null) {
        return (
            <>
                <ChildTitle parent={ASSETS}>{t("冷钱包签名")}</ChildTitle>
                <Empty icon="qr" action={<LinkButton to="/history">{t("查看记录")}</LinkButton>}>
                    {t("没有找到这笔交易.")}
                </Empty>
            </>
        );
    }
    if (tx.state === "signing") return <SigningView key={tx.id} tx={tx} />;
    if (tx.state === "signed") return <SignedView key={tx.id} tx={tx} />;
    if ((tx.state === "abandoned" && !tx.raw) || tx.state === "expired") return <ClosedView tx={tx} />;
    return <Navigate to={txHref(tx)} replace />;
}

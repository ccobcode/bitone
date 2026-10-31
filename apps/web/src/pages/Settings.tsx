import { AlertDialog } from "@base-ui/react/alert-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { saveSetting, useApp, useLive } from "../data/app";
import { BackupError, MIN_PASSPHRASE, exportBackup, importBackup, passphraseLongEnough } from "../data/backup";
import { clearLocalData, db } from "../data/db";
import { requestPersist } from "../data/pairing";
import { useGateway } from "../gateway/provider";
import type { SocketStatus } from "../gateway/socket";
import { Icon } from "../ui/Icon";
import { Alert, Button, Chip, PageTitle, SectionTitle, Segmented, buttonClass } from "../ui/primitives";
import { lang, setLang, t, type Lang } from "../core/i18n";

const SOCKET_LABELS: Record<SocketStatus, string> = {
    connecting: t("连接中"),
    live: t("已连接"),
    retrying: t("重连中"),
    degraded: t("定时刷新"),
};

function day(time: number): string {
    const date = new Date(time);
    const pad = (value: number) => String(value).padStart(2, "0");
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function download(text: string, name: string): void {
    const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    link.click();
    URL.revokeObjectURL(url);
}

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
}

function useCounts() {
    return useLive(async () => {
        const [wallets, accounts, txs, contacts] = await Promise.all([
            db.wallets.count(),
            db.accounts.count(),
            db.txs.count(),
            db.contacts.count(),
        ]);
        return [
            { label: t("钱包"), value: wallets },
            { label: t("账户"), value: accounts },
            { label: t("交易"), value: txs },
            { label: t("地址簿"), value: contacts },
        ];
    }, []);
}

/** 本地数据占用的空间。读一次钱包表，数据变化时重新估算。 */
function useUsage(): number | null {
    const usage = useLive(async () => {
        await db.wallets.count();
        return (await navigator.storage?.estimate?.().catch(() => null))?.usage ?? null;
    }, []);
    return usage ?? null;
}

/** 一组设置是一张卡片，标题在上，下面一行一项。 */
function Group({ id, title, tone, children }: { id: string; title: string; tone?: string; children: ReactNode }) {
    return (
        <section aria-labelledby={id} className="card gap-s2">
            <SectionTitle id={id} className={tone}>
                {title}
            </SectionTitle>
            <div className="flex flex-col">{children}</div>
        </section>
    );
}

/** 一行设置。左边的标题贴卡片左内边距，右边的值或操作贴右内边距。 */
function Setting({
    label,
    hint,
    wide,
    children,
}: {
    label: ReactNode;
    hint?: string;
    wide?: boolean;
    children: ReactNode;
}) {
    return (
        <div className="setting">
            <div className="setting-main">
                <span className="text-t3">{label}</span>
                {hint ? <span className="text-t2 text-text-3">{hint}</span> : null}
            </div>
            <div className={`setting-side ${wide ? "setting-wide" : ""}`}>{children}</div>
        </div>
    );
}

function ClearData() {
    const client = useQueryClient();
    const navigate = useNavigate();
    const [busy, setBusy] = useState(false);

    async function clear() {
        setBusy(true);
        await clearLocalData();
        client.clear();
        await navigate("/welcome", { replace: true });
    }

    return (
        <AlertDialog.Root>
            <AlertDialog.Trigger className={buttonClass("danger")}>{t("清除")}</AlertDialog.Trigger>
            <AlertDialog.Portal>
                <AlertDialog.Backdrop className="scrim fixed inset-0 z-30" />
                <AlertDialog.Popup className="dialog fixed top-1/2 left-1/2 z-30 flex w-[min(440px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-s4">
                    <AlertDialog.Title className="text-t5 font-semibold">{t("确认清除本地数据")}</AlertDialog.Title>
                    <AlertDialog.Description className="text-t3 text-text-2">
                        {t("这个浏览器里的钱包、交易记录和地址簿都会删除。")}
                    </AlertDialog.Description>
                    <div className="flex justify-end gap-s2">
                        <AlertDialog.Close className={buttonClass("secondary")}>{t("取消")}</AlertDialog.Close>
                        <Button variant="danger" disabled={busy} onClick={() => void clear()}>
                            {t("确认清除")}
                        </Button>
                    </div>
                </AlertDialog.Popup>
            </AlertDialog.Portal>
        </AlertDialog.Root>
    );
}

function BackupCard() {
    const { selectWallet, wallet } = useApp();
    const navigate = useNavigate();
    const upload = useRef<HTMLInputElement>(null);
    const [restored, setRestored] = useState("");
    const [passphrase, setPassphrase] = useState("");
    const [busy, setBusy] = useState(false);
    const [message, setMessage] = useState("");
    const [failure, setFailure] = useState("");
    const last = useLive(async () => (await db.settings.get("lastBackup"))?.value, []);
    const ready = passphraseLongEnough(passphrase);

    // 等全局状态读到导入的钱包再跳转，否则路由守卫看到的还是没有钱包。
    useEffect(() => {
        if (restored && wallet?.id === restored) void navigate("/device");
    }, [restored, wallet, navigate]);

    async function work(task: () => Promise<void>) {
        setBusy(true);
        setMessage("");
        setFailure("");
        try {
            await task();
            setPassphrase("");
        } catch (error) {
            setFailure(error instanceof BackupError ? error.message : t("操作失败，没有写入任何数据。"));
        } finally {
            setBusy(false);
        }
    }

    const save = () =>
        work(async () => {
            const text = await exportBackup(passphrase);
            download(text, `bitone-backup-${day(Date.now())}.json`);
            await saveSetting("lastBackup", Date.now());
            setMessage(t("备份已导出。"));
        });

    const restore = (file: File) =>
        work(async () => {
            const summary = await importBackup(await file.text(), passphrase);
            await requestPersist();
            if (summary.first) await selectWallet(summary.first);
            const conflict =
                summary.conflicts > 0 ? t("{0} 个账户与本地公钥不同，已保留本地的。", summary.conflicts) : "";
            setMessage(t("已导入 {0} 个账户、{1} 个联系人。{2}", summary.accounts, summary.contacts, conflict));
            if (summary.first) setRestored(summary.first.id);
        });

    return (
        <Group id="h-bak" title={t("备份")}>
            <Setting
                wide
                label={<label htmlFor="bakpw">{t("备份口令")}</label>}
                hint={t("导入的联系人和账户都要重新确认")}
            >
                <input
                    id="bakpw"
                    type="password"
                    autoComplete="new-password"
                    placeholder={t("至少 {0} 个字符", MIN_PASSPHRASE)}
                    value={passphrase}
                    onChange={(event) => setPassphrase(event.target.value)}
                    className="field w-full text-t3 nav:w-72"
                />
            </Setting>
            <input
                ref={upload}
                type="file"
                accept="application/json,.json"
                hidden
                aria-label={t("选择备份文件")}
                onChange={(event) => {
                    const [file] = event.target.files ?? [];
                    event.target.value = "";
                    if (file) void restore(file);
                }}
            />
            <Setting
                wide
                label={t("备份文件")}
                hint={t("上次备份 {0}", typeof last === "number" ? day(last) : t("暂无"))}
            >
                <Button disabled={!ready || busy} onClick={() => upload.current?.click()}>
                    <Icon name="upload" />
                    {t("导入备份")}
                </Button>
                <Button disabled={!ready || busy} onClick={() => void save()}>
                    <Icon name="download" />
                    {t("导出加密备份")}
                </Button>
            </Setting>
            {message ? (
                <p role="status" className="pt-s2 text-t2 text-ok">
                    {message}
                </p>
            ) : null}
            {failure ? <Alert className="pt-s2">{failure}</Alert> : null}
        </Group>
    );
}

export default function Settings() {
    const { persisted, refreshPersisted } = useApp();
    const gateway = useGateway();
    const counts = useCounts();
    const usage = useUsage();
    const [denied, setDenied] = useState(false);

    async function persist() {
        setDenied(!(await requestPersist()));
        await refreshPersisted();
    }

    const socketLive = gateway.socket === "live";

    return (
        <>
            <PageTitle>{t("设置")}</PageTitle>

            <Group id="h-lang" title={t("语言")}>
                <Setting label={t("界面语言")} hint={t("默认跟随系统语言")}>
                    <Segmented<Lang>
                        label={t("界面语言")}
                        value={lang()}
                        options={[
                            { value: "zh", label: "中文" },
                            { value: "en", label: "English" },
                        ]}
                        onChange={(next) => {
                            setLang(next);
                            location.reload();
                        }}
                    />
                </Setting>
            </Group>

            <Group id="h-gw" title={t("网关")}>
                <Setting label={t("网关地址")} hint={t("和网页端同域部署，不能修改")}>
                    <span className="font-mono text-t2 [overflow-wrap:anywhere]">{location.origin}</span>
                </Setting>
                <Setting label="HTTP">
                    {gateway.httpOk && gateway.latency !== null ? (
                        <span className="text-t2 text-text-3">{gateway.latency} ms</span>
                    ) : null}
                    <Chip tone={gateway.httpOk ? "ok" : "danger"}>{gateway.httpOk ? t("正常") : t("不可用")}</Chip>
                </Setting>
                <Setting label="WebSocket">
                    <Chip tone={socketLive ? "ok" : "warn"}>{SOCKET_LABELS[gateway.socket]}</Chip>
                </Setting>
            </Group>

            <Group id="h-local" title={t("本地数据")}>
                <Setting
                    label={t("存储")}
                    hint={
                        persisted
                            ? undefined
                            : denied
                              ? t("浏览器拒绝了申请，请导出备份。")
                              : t("浏览器可能清理本地数据，地址簿和备注只能从备份恢复。")
                    }
                >
                    {usage !== null ? <span className="text-t2 text-text-3">{formatBytes(usage)}</span> : null}
                    <Chip tone={persisted ? "ok" : "warn"}>{persisted ? t("已持久化") : t("未持久化")}</Chip>
                    {persisted ? null : (
                        <Button size="sm" onClick={() => void persist()}>
                            {t("再次申请")}
                        </Button>
                    )}
                </Setting>
                <Setting label={t("记录")}>
                    <span className="text-t2 text-text-2">
                        {(counts ?? []).map((count) => `${count.label} ${count.value}`).join(t("，"))}
                    </span>
                </Setting>
            </Group>

            <BackupCard />

            <Group id="h-danger" title={t("危险操作")} tone="text-danger">
                <Setting label={t("清除本地数据")} hint={t("钱包可重新扫码恢复，地址簿和备注只能从备份恢复")}>
                    <ClearData />
                </Setting>
            </Group>
        </>
    );
}

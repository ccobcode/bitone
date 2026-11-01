import { useQueryClient } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { playChime, unlockChime } from "../core/chime";
import { saveSetting, useApp, useLive } from "../data/app";
import { clearLocalData, db } from "../data/db";
import { requestPersist } from "../data/pairing";
import { useGateway } from "../gateway/provider";
import type { SocketStatus } from "../gateway/socket";
import { Icon } from "../ui/Icon";
import { dateLabel } from "../ui/labels";
import { LanguageSwitch } from "../ui/LanguageSwitch";
import { requestBackground, useNoticeSettings } from "../ui/Notices";
import { Button, Chip, LinkButton, PageTitle, SectionTitle, Switch } from "../ui/primitives";
import { Confirm, overlayLink } from "../ui/Sheet";
import { t } from "../core/i18n";

const SOCKET_LABELS: Record<SocketStatus, string> = {
    connecting: t("连接中"),
    live: t("已连接"),
    retrying: t("重连中"),
    degraded: t("定时刷新"),
};

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

/** 本地数据占用的空间.读一次钱包表,数据变化时重新估算. */
function useUsage(): number | null {
    const usage = useLive(async () => {
        await db.wallets.count();
        return (await navigator.storage?.estimate?.().catch(() => null))?.usage ?? null;
    }, []);
    return usage ?? null;
}

/** 一组设置是一张卡片,标题在上,下面一行一项. */
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

/** 一行设置.左边的标题贴卡片左内边距,右边的值或操作贴右内边距. */
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
        <Confirm
            trigger={t("清除")}
            variant="danger"
            title={t("确认清除本地数据")}
            cancel={t("取消")}
            actions={
                <Button variant="danger" disabled={busy} onClick={() => void clear()}>
                    {t("确认清除")}
                </Button>
            }
        >
            {t("这个浏览器里的钱包,交易记录和地址簿都会删除.")}
        </Confirm>
    );
}

function BackupCard() {
    const last = useLive(async () => (await db.settings.get("lastBackup"))?.value, []);
    return (
        <Group id="h-bak" title={t("备份")}>
            <Setting
                wide
                label={t("备份文件")}
                hint={t("上次备份 {0}", typeof last === "number" ? dateLabel(last) : t("暂无"))}
            >
                <LinkButton {...overlayLink("?import=1")}>
                    <Icon name="upload" />
                    {t("导入备份")}
                </LinkButton>
                <LinkButton {...overlayLink("?export=1")}>
                    <Icon name="download" />
                    {t("导出加密备份")}
                </LinkButton>
            </Setting>
        </Group>
    );
}

/** 提示音和后台提醒都要用户自己打开,浏览器也要求这两件事发生在点击里. */
function NoticeGroup() {
    const { sound, background, amount } = useNoticeSettings();
    const [denied, setDenied] = useState(false);
    const supported = typeof Notification === "function";

    async function setBackground(value: boolean) {
        setDenied(value && !(await requestBackground()));
        if (!value) await saveSetting("notifyBackground", false);
    }

    return (
        <Group id="h-notice" title={t("通知")}>
            <Setting label={t("提示音")} hint={t("收到转账和交易有结果时响一声,测试网不响")}>
                <Switch
                    label={t("提示音")}
                    checked={sound}
                    onChange={(value) => {
                        if (value) unlockChime();
                        void saveSetting("notifySound", value);
                        if (value) playChime("incoming");
                    }}
                />
            </Setting>
            {supported ? (
                <Setting
                    label={t("后台提醒")}
                    hint={denied ? t("浏览器拒绝了通知权限,要在浏览器设置里开启") : t("页面在后台时用系统通知提醒")}
                >
                    <Switch
                        label={t("后台提醒")}
                        checked={background}
                        onChange={(value) => void setBackground(value)}
                    />
                </Setting>
            ) : null}
            {supported ? (
                <Setting label={t("提醒里显示金额")} hint={t("系统通知可能出现在锁屏或屏幕共享里")}>
                    <Switch
                        label={t("提醒里显示金额")}
                        checked={amount}
                        onChange={(value) => void saveSetting("notifyAmount", value)}
                    />
                </Setting>
            ) : null}
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
                    <LanguageSwitch />
                </Setting>
            </Group>

            <NoticeGroup />

            <Group id="h-gw" title={t("网关")}>
                <Setting label={t("网关地址")} hint={t("和网页端同域部署,不能修改")}>
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
                              ? t("浏览器拒绝了申请,请导出备份.")
                              : t("浏览器可能清理本地数据,地址簿和备注只能从备份恢复.")
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
                        {(counts ?? []).map((count) => `${count.label} ${count.value}`).join(t(","))}
                    </span>
                </Setting>
            </Group>

            <BackupCard />

            <Group id="h-adv" title={t("高级")}>
                <Setting label={t("原始请求")} hint={t("转账表单不支持的交易,手动填写后由冷钱包签名")}>
                    <LinkButton to="/raw">{t("打开")}</LinkButton>
                </Setting>
            </Group>

            <Group id="h-danger" title={t("危险操作")} tone="text-danger">
                <Setting label={t("清除本地数据")} hint={t("钱包可重新扫码恢复,地址簿和备注只能从备份恢复")}>
                    <ClearData />
                </Setting>
            </Group>
        </>
    );
}

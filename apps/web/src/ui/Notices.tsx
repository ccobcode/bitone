import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate } from "react-router";
import { playChime, unlockChime } from "../core/chime";
import { t } from "../core/i18n";
import { tokens } from "../core/tokens";
import { saveSetting, useLive } from "../data/app";
import { db, type TxRecord, type TxState } from "../data/db";
import { claimNotice, isTestnet, mergedNotice, onNotice, publish, systemNotice, type Notice } from "../data/notify";
import { Icon } from "./Icon";
import { Button } from "./primitives";

/** 这段时间内连续的转入只响一次. */
const MERGE_WINDOW = 3000;
const TOAST_LIFE = tokens.toast;

interface Toast {
    id: number;
    notice: Notice;
    at: number;
    /** 第一次收到转入时,问一次要不要开提醒. */
    ask: boolean;
}

export interface NoticeSettings {
    sound: boolean;
    background: boolean;
    amount: boolean;
    asked: boolean;
}

/** 通知开关都存在设置表里,默认关闭. */
export function useNoticeSettings(): NoticeSettings {
    const rows = useLive(
        () => db.settings.bulkGet(["notifySound", "notifyBackground", "notifyAmount", "notifyAsked"]),
        [],
    );
    const [sound, background, amount, asked] = rows ?? [];
    return {
        sound: sound?.value === true,
        background: background?.value === true,
        amount: amount?.value === true,
        // 读到之前当成问过,不能闪一下询问.
        asked: rows === undefined || asked?.value === true,
    };
}

/** 请求系统通知权限,要在点击里调用.返回是否拿到. */
export async function requestBackground(): Promise<boolean> {
    if (typeof Notification !== "function") return false;
    const granted = (await Notification.requestPermission()) === "granted";
    await saveSetting("notifyBackground", granted);
    return granted;
}

/** 自己的转账走到这些状态时通知. */
function txNotice(tx: TxRecord): Notice | null {
    const base = { network: tx.network, link: `/history/${encodeURIComponent(tx.id)}`, count: 1 };
    if (tx.state === "confirming" || tx.state === "confirmed") {
        return { ...base, kind: "included", title: t("转账已上链"), summary: t("转账已上链") };
    }
    if (tx.state === "failed") return { ...base, kind: "failed", title: t("转账失败"), summary: t("转账失败") };
    if (tx.state === "expired") return { ...base, kind: "failed", title: t("转账已过期"), summary: t("转账已过期") };
    return null;
}

/** 状态变化时发通知.打开页面时已有的状态是起点,不通知. */
function useTxNotices() {
    const txs = useLive(() => db.txs.toArray(), []);
    const seen = useRef<Map<string, TxState> | null>(null);
    useEffect(() => {
        if (!txs) return;
        const last = seen.current;
        seen.current = new Map(txs.map((tx) => [tx.id, tx.state]));
        if (!last) return;
        for (const tx of txs) {
            if (last.get(tx.id) === tx.state) continue;
            const notice = txNotice(tx);
            if (!notice) continue;
            void claimNotice(`${tx.id}:${notice.kind}`).then((won) => {
                if (won) publish(notice);
            });
        }
    }, [txs]);
}

function ToastCard({ toast, onClose, children }: { toast: Toast; onClose(): void; children?: ReactNode }) {
    const failed = toast.notice.kind === "failed";
    const [held, setHeld] = useState(false);
    const [hidden, setHidden] = useState(document.hidden);

    useEffect(() => {
        const update = () => setHidden(document.hidden);
        document.addEventListener("visibilitychange", update);
        return () => document.removeEventListener("visibilitychange", update);
    }, []);

    // 失败要用户自己关.其余的在页面看不见或鼠标悬停时不计时,不能没看到就消失.
    useEffect(() => {
        if (failed || held || hidden || toast.ask) return;
        const timer = setTimeout(onClose, TOAST_LIFE);
        return () => clearTimeout(timer);
    }, [failed, held, hidden, toast.ask, onClose]);

    return (
        // biome-ignore lint/a11y/noStaticElementInteractions: 悬停和聚焦只暂停计时,不是操作.
        <div
            role={failed ? "alert" : "status"}
            onMouseEnter={() => setHeld(true)}
            onMouseLeave={() => setHeld(false)}
            onFocus={() => setHeld(true)}
            onBlur={() => setHeld(false)}
            className="rise pointer-events-auto flex flex-col gap-s2 rounded-r3 bg-surface px-s4 py-s3 text-t3 shadow-2"
        >
            <div className="flex items-center gap-s2">
                <Icon
                    name={failed ? "alert" : "check"}
                    size={16}
                    className={`shrink-0 ${failed ? "text-danger" : "text-accent"}`}
                />
                <span className="min-w-0 flex-1 [overflow-wrap:anywhere]">{toast.notice.title}</span>
                <Link to={toast.notice.link} onClick={onClose} className="shrink-0 text-t2 text-accent">
                    {t("查看")}
                </Link>
                <button
                    type="button"
                    aria-label={t("关闭")}
                    onClick={onClose}
                    className="press -my-s2 -mr-s2 flex size-11 shrink-0 items-center justify-center text-text-3 nav:size-8"
                >
                    <Icon name="close" size={16} />
                </button>
            </div>
            {children}
        </div>
    );
}

/** 签名和扫码时不打扰,结束后再提示.显示收款二维码时用户正等着到账,照常提示. */
function isQuiet(pathname: string, search: string): boolean {
    const params = new URLSearchParams(search);
    return pathname.startsWith("/sign/") || params.has("connect") || params.has("import");
}

export function Notices() {
    const { pathname, search } = useLocation();
    const navigate = useNavigate();
    const settings = useNoticeSettings();
    const [toasts, setToasts] = useState<Toast[]>([]);
    const nextId = useRef(1);
    const held = useRef<Notice[]>([]);
    const current = useRef({ settings, toasts, quiet: false });
    const quiet = isQuiet(pathname, search);
    current.current = { settings, toasts, quiet };
    useTxNotices();

    const sysNotify = useCallback(
        (notice: Notice) => {
            const { background, amount } = current.current.settings;
            if (!document.hidden || !background || typeof Notification !== "function") return;
            if (Notification.permission !== "granted") return;
            systemNotice(amount ? notice.title : notice.summary, () => {
                window.focus();
                void navigate(notice.link);
            });
        },
        [navigate],
    );

    /** 声音和系统通知,只有发现这件事的标签页负责. */
    const alert = useCallback(
        (notice: Notice) => {
            const { sound } = current.current.settings;
            if (sound && !isTestnet(notice.network)) playChime(notice.kind);
            sysNotify(notice);
        },
        [sysNotify],
    );

    const show = useCallback(
        (notice: Notice, origin: boolean) => {
            const now = Date.now();
            const { settings: options, toasts: list } = current.current;
            const shown = notice.kind === "incoming" && list.find((toast) => toast.notice.kind === "incoming");
            if (shown) {
                // 屏幕上已有转入提示时并进去,不叠一摞.刚响过就不再响.
                const merged = mergedNotice(shown.notice, shown.notice.count + notice.count);
                setToasts((previous) =>
                    previous.map((toast) => (toast.id === shown.id ? { ...toast, notice: merged, at: now } : toast)),
                );
                if (origin && now - shown.at >= MERGE_WINDOW) alert(notice);
                return;
            }
            const ask = notice.kind === "incoming" && !options.asked;
            if (ask) void saveSetting("notifyAsked", true);
            const toast = { id: nextId.current++, notice, at: now, ask };
            setToasts((previous) => [...previous, toast]);
            if (origin) alert(notice);
        },
        [alert],
    );

    useEffect(
        () =>
            onNotice((notice, origin) => {
                if (current.current.quiet) held.current.push(notice);
                else show(notice, origin);
            }),
        [show],
    );

    useEffect(() => {
        if (quiet) return;
        for (const notice of held.current.splice(0)) show(notice, true);
    }, [quiet, show]);

    // 浏览器要求点过页面才能发声,开着提示音时用第一次点击解锁.
    useEffect(() => {
        if (!settings.sound) return;
        document.addEventListener("pointerdown", unlockChime, { once: true });
        return () => document.removeEventListener("pointerdown", unlockChime);
    }, [settings.sound]);

    const close = useCallback((id: number) => {
        setToasts((previous) => previous.filter((toast) => toast.id !== id));
    }, []);

    async function enable(toast: Toast) {
        unlockChime();
        await saveSetting("notifySound", true);
        playChime("incoming");
        await requestBackground();
        close(toast.id);
    }

    if (toasts.length === 0) return null;
    return (
        <div className="pointer-events-none fixed inset-x-(--page-x) top-[calc(var(--safe-top)+var(--s2))] z-40 flex flex-col gap-s2 nav:inset-x-auto nav:top-auto nav:right-s6 nav:bottom-s6 nav:w-90">
            {toasts.map((toast) => (
                <ToastCard key={toast.id} toast={toast} onClose={() => close(toast.id)}>
                    {toast.ask ? (
                        <div className="flex justify-end gap-s2">
                            <Button size="sm" onClick={() => close(toast.id)}>
                                {t("暂不")}
                            </Button>
                            <Button size="sm" variant="primary" onClick={() => void enable(toast)}>
                                {t("开启提醒")}
                            </Button>
                        </div>
                    ) : null}
                </ToastCard>
            ))}
        </div>
    );
}

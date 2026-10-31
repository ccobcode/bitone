import { Link } from "react-router";
import { useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { useGateway } from "../gateway/provider";
import { Chip } from "./primitives";
import { t } from "../core/i18n";

/** 连接状态和延迟。正常时只有圆点和延迟，出问题才显示文字和颜色。窄屏时不显示文字，放不下时也不显示延迟。延迟宽度固定，数字变化时不推动旁边的内容。 */
function LiveState() {
    const { socket, down, latency } = useGateway();
    const state = down
        ? { label: t("已断开"), dot: "bg-danger", text: "text-danger" }
        : socket === "degraded"
          ? { label: t("定时刷新"), dot: "bg-warn", text: "text-warn" }
          : socket !== "live"
            ? { label: t("重连中"), dot: "bg-warn", text: "text-warn" }
            : { label: "", dot: "bg-ok", text: "text-text-3" };
    return (
        <span className={`flex items-center gap-s1 text-t2 transition-colors ${state.text}`}>
            <span aria-hidden="true" className={`size-2 rounded-full transition-colors ${state.dot}`} />
            {state.label ? <span className="sr-only nav:not-sr-only">{state.label}</span> : null}
            <span className="hidden w-12 tabular-nums min-[23rem]:inline">
                {down || latency === null ? "—" : `${latency} ms`}
            </span>
        </span>
    );
}

/** 需要处理的交易：结果不明、链上失败都不能只留在交易记录里。 */
function AttentionChip() {
    const { wallet } = useApp();
    const seenAt = useLive(async () => (await db.settings.get("historySeenAt"))?.value, []);
    const rows = useLive(async () => (wallet ? db.txs.where("walletId").equals(wallet.id).toArray() : []), [wallet]);
    const since = typeof seenAt === "number" ? seenAt : 0;
    const uncertain = (rows ?? []).filter((tx) => tx.state === "uncertain").length;
    const failed = (rows ?? []).filter((tx) => tx.state === "failed" && tx.updatedAt > since).length;
    if (uncertain === 0 && failed === 0) return null;
    const label = failed > 0 ? t("链上失败") : t("结果不明");
    const count = failed > 0 ? failed : uncertain;
    return (
        <Link to="/history" aria-label={t("{0} {1} 笔", label, count)} className="rise no-underline">
            {/* 手机上只留数字，钱包切换不被挤窄。 */}
            <Chip tone={failed > 0 ? "danger" : "warn"}>
                <span className="hidden nav:inline">{label} </span>
                {count}
                <span className="hidden nav:inline"> {t("笔")}</span>
            </Chip>
        </Link>
    );
}

/** 顶栏右侧的状态：先是要处理的事，再是连接状态。 */
export function WebTop() {
    return (
        <div className="flex shrink-0 items-center gap-s3">
            <AttentionChip />
            <LiveState />
        </div>
    );
}

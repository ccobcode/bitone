import { Menu } from "@base-ui/react/menu";
import { useNavigate } from "react-router";
import { useAttention } from "../gateway/feed";
import { useGateway } from "../gateway/provider";
import { Chip } from "./primitives";
import { AssetIcon } from "./TokenIcon";
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

/** 等着用户动手的事。平时不出现，有事时才占顶栏的位置。 */
function Inbox() {
    const rows = useAttention();
    const navigate = useNavigate();
    if (rows.length === 0) return null;
    const tone = rows.some((row) => row.tone === "danger")
        ? "danger"
        : rows.some((row) => row.tone === "warn")
          ? "warn"
          : "accent";
    return (
        <Menu.Root>
            <Menu.Trigger aria-label={t("{0} 项待处理", rows.length)} className="press rise flex min-h-11 items-center">
                {/* 手机上只留数字，钱包切换不被挤窄。 */}
                <Chip tone={tone}>
                    {rows.length}
                    <span className="hidden nav:inline"> {t("待处理")}</span>
                </Chip>
            </Menu.Trigger>
            <Menu.Portal>
                <Menu.Positioner sideOffset={8} align="end" className="z-20">
                    <Menu.Popup className="popup w-[min(22rem,calc(100vw-2*var(--page-x)))]">
                        {rows.map((row) => (
                            <Menu.Item
                                key={row.key}
                                onClick={() => void navigate(row.href)}
                                className="popup-item cursor-pointer"
                            >
                                <AssetIcon entry={row.entry} network={row.network} size={28} />
                                <span className="flex min-w-0 flex-1 flex-col">
                                    <span className="truncate font-medium">{row.title}</span>
                                    <span className="truncate text-t1 text-text-3">{row.party}</span>
                                </span>
                                <Chip tone={row.tone}>{row.status}</Chip>
                            </Menu.Item>
                        ))}
                    </Menu.Popup>
                </Menu.Positioner>
            </Menu.Portal>
        </Menu.Root>
    );
}

/** 顶栏右侧的状态：先是要处理的事，再是连接状态。 */
export function WebTop() {
    return (
        <div className="flex shrink-0 items-center gap-s3">
            <Inbox />
            <LiveState />
        </div>
    );
}

import { AlertDialog } from "@base-ui/react/alert-dialog";
import { Dialog } from "@base-ui/react/dialog";
import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router";
import { Icon } from "./Icon";
import { buttonClass, type ButtonVariant } from "./primitives";
import { t } from "../core/i18n";

/** 打开浮层的链接属性。浮层进历史记录，浏览器的返回能关掉它。 */
export function overlayLink(search: string) {
    return { to: { search }, state: { overlay: true } };
}

/** 打开收款浮层。asset 是清单里的资产键，verify 直接进核对。 */
export function receiveLink(accountId: string, { asset, verify }: { asset?: string; verify?: boolean } = {}) {
    const params = new URLSearchParams({ receive: accountId });
    if (asset) params.set("asset", asset);
    if (verify) params.set("verify", "1");
    return overlayLink(`?${params}`);
}

/** 关掉浮层，回到打开它的页面。直接打开的网址没有上一页，换成清掉参数。 */
export function useCloseOverlay(): () => void {
    const { state } = useLocation();
    const navigate = useNavigate();
    return () => {
        if (state?.overlay) void navigate(-1);
        else void navigate({ search: "" }, { replace: true });
    };
}

function sheetWidth(width: number): CSSProperties {
    return { "--sheet-width": `${width}px` } as CSSProperties;
}

/**
 * 软键盘盖住的高度。iOS 和安卓默认都不缩小布局视口，贴底的浮层要自己让开，
 * 不用 `interactive-widget`，它只有安卓有，而且底部标签栏也会被顶起来。
 */
function useKeyboardInset() {
    useEffect(() => {
        const view = window.visualViewport;
        if (!view) return;
        const root = document.documentElement;
        const update = () => {
            const covered = Math.max(0, window.innerHeight - view.height - view.offsetTop);
            root.style.setProperty("--keyboard", `${Math.round(covered)}px`);
            // 浮层变矮后，正在输入的那一项可能被挤到可视区外。
            if (covered > 0 && document.activeElement instanceof HTMLElement) {
                document.activeElement.scrollIntoView({ block: "nearest" });
            }
        };
        update();
        view.addEventListener("resize", update);
        view.addEventListener("scroll", update);
        return () => {
            view.removeEventListener("resize", update);
            view.removeEventListener("scroll", update);
            root.style.removeProperty("--keyboard");
        };
    }, []);
}

/** 浮层底部的操作区。内容比一屏高时贴着底部，不用滚动才够得到主操作。fill 时按钮等分，和上面的内容共用左右边线。 */
export function SheetActions({ fill, children }: { fill?: boolean; children: ReactNode }) {
    return <div className={`sheet-actions ${fill ? "*:flex-1" : ""}`}>{children}</div>;
}

interface SheetProps {
    title: string;
    /** 宽屏时的宽度。 */
    width?: number;
    children: ReactNode;
}

/** 浮层的外框。一次性的轻任务放在这里，关掉回到原处。 */
export function Sheet({ title, width = 560, children }: SheetProps) {
    const close = useCloseOverlay();
    const popup = useRef<HTMLDivElement>(null);
    useKeyboardInset();
    return (
        <Dialog.Root open onOpenChange={(open) => (open ? undefined : close())}>
            <Dialog.Portal>
                <Dialog.Backdrop className="scrim fixed inset-0 z-30" />
                {/* 焦点放在浮层本身，打开时关闭按钮不显示焦点环。 */}
                <Dialog.Popup
                    ref={popup}
                    initialFocus={popup}
                    tabIndex={-1}
                    className="sheet"
                    style={sheetWidth(width)}
                >
                    <div className="flex min-h-8 items-center gap-s2">
                        <Dialog.Title className="min-w-0 flex-1 truncate text-t5 font-semibold">{title}</Dialog.Title>
                        <Dialog.Close aria-label={t("关闭")} className={buttonClass("ghost", "iconSm")}>
                            <Icon name="close" />
                        </Dialog.Close>
                    </div>
                    {children}
                </Dialog.Popup>
            </Dialog.Portal>
        </Dialog.Root>
    );
}

interface ConfirmProps {
    /** 打开确认框的按钮。 */
    trigger: ReactNode;
    variant?: ButtonVariant;
    title: string;
    /** 放弃这次操作的按钮文字。 */
    cancel: string;
    /** 确认的按钮，放在放弃按钮后面。 */
    actions: ReactNode;
    children: ReactNode;
}

/** 确认一个操作。全站一个写法，默认焦点在放弃按钮上。 */
export function Confirm({ trigger, variant, title, cancel, actions, children }: ConfirmProps) {
    return (
        <AlertDialog.Root>
            <AlertDialog.Trigger className={buttonClass(variant)}>{trigger}</AlertDialog.Trigger>
            <AlertDialog.Portal>
                <AlertDialog.Backdrop className="scrim fixed inset-0 z-30" />
                <AlertDialog.Popup className="sheet" style={sheetWidth(480)}>
                    <AlertDialog.Title className="text-t5 font-semibold">{title}</AlertDialog.Title>
                    <AlertDialog.Description render={<div />} className="flex flex-col gap-s3 text-t3 text-text-2">
                        {children}
                    </AlertDialog.Description>
                    <SheetActions>
                        <AlertDialog.Close className={buttonClass("secondary")}>{cancel}</AlertDialog.Close>
                        {actions}
                    </SheetActions>
                </AlertDialog.Popup>
            </AlertDialog.Portal>
        </AlertDialog.Root>
    );
}

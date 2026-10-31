import { AlertDialog } from "@base-ui/react/alert-dialog";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Outlet, useLocation, useNavigate } from "react-router";
import { useApp, useLive } from "../data/app";
import { db } from "../data/db";
import { GatewayProvider } from "../gateway/provider";
import { Icon } from "./Icon";
import { modeLabel } from "./labels";
import { Notices } from "./Notices";
import { Overlays } from "./Overlays";
import { SheetActions, sheetWidth } from "./Sheet";
import { Button, Chip, LinkButton, TitleSlot, buttonClass } from "./primitives";
import { Steps } from "./Steps";
import { TabBar, WebNav } from "./WebNav";
import { t } from "../core/i18n";

/** 转账和签名跨两台设备，收起导航。退出后回到进来之前的页面。 */
function isFocus(pathname: string): boolean {
    return pathname === "/send" || pathname.startsWith("/sign/");
}

/** 专注流程的顶栏。网络标签不能点，这时切换网络只会出错，但也不能看不见。 */
function FocusBar({ origin, pending }: { origin: string; pending: boolean }) {
    const { mode, wallet } = useApp();
    const navigate = useNavigate();
    return (
        <header className="topbar">
            <div className="page-frame flex h-full items-center justify-between gap-s3">
                {pending ? (
                    <AlertDialog.Root>
                        <AlertDialog.Trigger className={buttonClass("ghost", "sm")}>
                            <Icon name="close" size={16} />
                            {t("退出")}
                        </AlertDialog.Trigger>
                        <AlertDialog.Portal>
                            <AlertDialog.Backdrop className="scrim fixed inset-0 z-30" />
                            <AlertDialog.Popup className="sheet" style={sheetWidth(440)}>
                                <AlertDialog.Title className="text-t5 font-semibold">
                                    {t("退出这笔转账")}
                                </AlertDialog.Title>
                                <AlertDialog.Description className="text-t3 text-text-2">
                                    {t("请求会留在待处理，可以随时回来继续。")}
                                </AlertDialog.Description>
                                <SheetActions>
                                    <AlertDialog.Close className={buttonClass("secondary")}>
                                        {t("继续")}
                                    </AlertDialog.Close>
                                    <Button variant="primary" onClick={() => void navigate(origin)}>
                                        {t("保留并退出")}
                                    </Button>
                                </SheetActions>
                            </AlertDialog.Popup>
                        </AlertDialog.Portal>
                    </AlertDialog.Root>
                ) : (
                    <LinkButton to={origin} variant="ghost" size="sm">
                        <Icon name="close" size={16} />
                        {t("退出")}
                    </LinkButton>
                )}
                <div className="flex min-w-0 items-center gap-s2">
                    <span className="truncate text-t2 text-text-2">{wallet?.name}</span>
                    <Chip tone={mode === "testnet" ? "testnet" : "neutral"}>{modeLabel(mode)}</Chip>
                </div>
            </div>
        </header>
    );
}

/** 转账流程走到哪一步。切页时外壳不重新挂载，下一段是真的填上去的。 */
function useStep(pathname: string, id: string | undefined): { step: 1 | 2 | 3; pending: boolean } {
    const state = useLive(async () => (id ? (await db.txs.get(id))?.state : undefined), [id]);
    if (pathname === "/send") return { step: 1, pending: false };
    return { step: state === "signed" ? 3 : 2, pending: state === "signing" || state === "signed" };
}

function signId(pathname: string): string | undefined {
    return pathname.startsWith("/sign/") ? decodeURIComponent(pathname.slice("/sign/".length)) : undefined;
}

export function Shell() {
    const { pathname } = useLocation();
    const focus = isFocus(pathname);
    const [slot, setSlot] = useState<HTMLElement | null>(null);
    const { step, pending } = useStep(pathname, signId(pathname));
    const origin = useRef("/");

    useEffect(() => {
        if (!focus) origin.current = pathname;
    }, [focus, pathname]);

    useLayoutEffect(() => {
        if (!pathname) return;
        window.scrollTo(0, 0);
    }, [pathname]);

    return (
        <GatewayProvider>
            <div className="app-frame">
                {focus ? <FocusBar origin={origin.current} pending={pending} /> : <WebNav />}
                <main className="app-main" data-focus={focus ? "" : undefined}>
                    <div className="page-frame">
                        <div className="page-head">
                            <div ref={setSlot} className="page-title-slot min-w-0 flex-1" />
                        </div>
                        {focus ? (
                            <div className="pb-s4">
                                <Steps current={step} />
                            </div>
                        ) : null}
                        <div className="page page-body">
                            <TitleSlot value={slot}>
                                <Outlet />
                            </TitleSlot>
                        </div>
                    </div>
                </main>
                {focus ? null : <TabBar />}
            </div>
            <Overlays />
            <Notices />
        </GatewayProvider>
    );
}

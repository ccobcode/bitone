import { useLayoutEffect, useState } from "react";
import { Outlet, useLocation } from "react-router";
import { GatewayProvider } from "../gateway/provider";
import { TitleSlot } from "./primitives";
import { Steps } from "./Steps";
import { TabBar, WebNav } from "./WebNav";

/** 转账流程的前两页共用一条步骤条，切页时下一段直接填上去。 */
// 广播页的主角是最终性进度，再放步骤条就是两条进度叠在一起。
const STEPS: Partial<Record<string, 1 | 2 | 3>> = { "/send": 1, "/sign": 2 };

export function Shell() {
    const { pathname } = useLocation();
    const [slot, setSlot] = useState<HTMLElement | null>(null);
    const step = STEPS[pathname];

    useLayoutEffect(() => {
        if (!pathname) return;
        window.scrollTo(0, 0);
    }, [pathname]);

    return (
        <GatewayProvider>
            <div className="app-frame">
                <WebNav />
                <main className="app-main">
                    <div className="page-frame">
                        <div className="page-head">
                            <div ref={setSlot} className="page-title-slot min-w-0 flex-1" />
                        </div>
                        {step ? (
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
                <TabBar />
            </div>
        </GatewayProvider>
    );
}

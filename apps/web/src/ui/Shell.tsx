import { useLayoutEffect, useState } from "react";
import { Outlet, useLocation } from "react-router";
import { useLive } from "../data/app";
import { db, type TxState } from "../data/db";
import { findTx } from "../data/txs";
import { GatewayProvider } from "../gateway/provider";
import { Notices } from "./Notices";
import { Overlays } from "./Overlays";
import { TitleSlot } from "./primitives";
import { Steps, type StepResult } from "./Steps";
import { TabBar, WebNav } from "./WebNav";

type Progress = { current: 1 | 2 | 3; result?: StepResult };

/** 这笔交易走到哪一步.没上链就结束的请求没有结果可说,不显示. */
const PROGRESS: Partial<Record<TxState, Progress>> = {
    signing: { current: 2 },
    signed: { current: 3 },
    broadcasting: { current: 3, result: "live" },
    broadcast: { current: 3, result: "live" },
    uncertain: { current: 3, result: "live" },
    confirming: { current: 3, result: "live" },
    confirmed: { current: 3, result: "finished" },
    failed: { current: 3, result: "failed" },
};

/** 转账流程走到哪一步.广播后落到交易详情,第三步要在那里等到结果.切页时外壳不重新挂载,下一段是真的填上去的. */
function useProgress(pathname: string): Progress | null {
    const [, first, second] = /^\/(?:sign|history)\/([^/]+)(?:\/([^/]+))?$/.exec(pathname) ?? [];
    const state = useLive(async () => {
        if (first && second) return (await findTx(first, decodeURIComponent(second)))?.state;
        return first ? (await db.txs.get(decodeURIComponent(first)))?.state : undefined;
    }, [first, second]);
    if (pathname === "/send") return { current: 1 };
    return (state && PROGRESS[state]) ?? null;
}

export function Shell() {
    const { pathname } = useLocation();
    const [slot, setSlot] = useState<HTMLElement | null>(null);
    const progress = useProgress(pathname);

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
                        <div className="page-head gap-s3">
                            <div ref={setSlot} className="page-title-slot min-w-0 flex-1" />
                            {progress ? <Steps {...progress} /> : null}
                        </div>
                        <div className="page page-body">
                            <TitleSlot value={slot}>
                                <Outlet />
                            </TitleSlot>
                        </div>
                    </div>
                </main>
                <TabBar />
            </div>
            <Overlays />
            <Notices />
        </GatewayProvider>
    );
}

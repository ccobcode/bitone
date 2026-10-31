// 路由.所有导航都走这里,页面切换用 View Transitions,不支持的浏览器直接切换.
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { flushSync } from "react-dom";
import { Router, UNSAFE_createBrowserHistory, type Location, type NavigationType } from "react-router";

const history = UNSAFE_createBrowserHistory({ v5Compat: true });

/** 前进还是返回看页面在产品里的先后,子页面排在父页面后面,从转账流程回到资产算返回. */
const ORDER = ["welcome", "", "assets", "history", "contacts", "device", "settings", "send", "sign"];

function rank(pathname: string): number {
    const [, section = "", child] = pathname.split("/");
    return ORDER.indexOf(section) * 2 + (child ? 1 : 0);
}

function direction(from: string, to: string, action: NavigationType, delta: number | null): string {
    if ((from === "/welcome") !== (to === "/welcome")) return "swap";
    // 浏览器的前进后退看历史记录的方向,其余看页面的先后.
    if (action === "POP" && delta) return delta < 0 ? "back" : "forward";
    return rank(to) < rank(from) ? "back" : "forward";
}

export function PageRouter({ children }: { children: ReactNode }) {
    const [state, setState] = useState<{ action: NavigationType; location: Location }>({
        action: history.action,
        location: history.location,
    });
    const current = useRef(state.location.pathname);

    useLayoutEffect(
        () =>
            history.listen(({ action, location, delta }) => {
                const from = current.current;
                current.current = location.pathname;
                const next = { action, location };
                // 只改查询参数不算换页,比如打开浮层.
                if (from === location.pathname || !document.startViewTransition) {
                    setState(next);
                    return;
                }
                const root = document.documentElement;
                root.dataset.nav = direction(from, location.pathname, action, delta);
                const transition = document.startViewTransition(() => flushSync(() => setState(next)));
                // 连续切换时上一次转场会被跳过,这不是错误.
                transition.ready.catch(() => undefined);
                transition.finished.catch(() => undefined).finally(() => delete root.dataset.nav);
            }),
        [],
    );

    return (
        <Router location={state.location} navigationType={state.action} navigator={history} useTransitions={false}>
            {children}
        </Router>
    );
}

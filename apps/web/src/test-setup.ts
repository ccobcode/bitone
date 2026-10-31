import "fake-indexeddb/auto";
import { setLang } from "./core/i18n";

// 单测断言的都是中文原文,不跟随运行环境的语言.
setLang("zh");

// Node 22 没有 Web Locks,CI 用的是这个版本.这里按规范的语义补一个:同名请求排队,回调结束才放锁.
if (!navigator.locks) {
    const tails = new Map<string, Promise<unknown>>();
    const request = (name: string, callback: () => Promise<unknown>) => {
        const run = (tails.get(name) ?? Promise.resolve()).then(callback);
        tails.set(
            name,
            run.catch(() => undefined),
        );
        return run;
    };
    Object.defineProperty(navigator, "locks", { value: { request } });
}

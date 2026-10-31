// 中英文。词典以中文原文为键，来源是 design/i18n.json。语言在启动时确定，切换时重新加载页面。
// 核心库返回的文案是中文，显示时用 translate 按模板翻译，译不出来就显示原文。
import en from "./i18n.gen.json";

export type Lang = "zh" | "en";

const KEY = "bitone-lang";
const dictionary: Record<string, string> = en;

function detect(): Lang {
    try {
        const saved = localStorage.getItem(KEY);
        if (saved === "zh" || saved === "en") return saved;
    } catch {
        // 隐私模式下读不到，按系统语言。
    }
    return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

let current = detect();
if (typeof document !== "undefined") document.documentElement.lang = current === "en" ? "en" : "zh-CN";

export function lang(): Lang {
    return current;
}

export function setLang(next: Lang): void {
    current = next;
    try {
        localStorage.setItem(KEY, next);
    } catch {
        // 存不下时只影响这一次。
    }
}

function fill(text: string, values: readonly (string | number)[]): string {
    return text.replace(/\{(\d+)\}/g, (_, index: string) => String(values[Number(index)] ?? ""));
}

/** `values` 依次填入文案里的 `{0}`、`{1}`。 */
export function t(text: string, ...values: (string | number)[]): string {
    return fill(current === "en" ? (dictionary[text] ?? text) : text, values);
}

let patterns: { regex: RegExp; english: string }[] | undefined;

const literal = (key: string) => key.replace(/\{\d+\}/g, "").length;

function compiled() {
    patterns ??= Object.keys(dictionary)
        .filter((key) => /\{\d+\}/.test(key))
        // 固定文字多的模板排在前面，「已输入 {0} 位」不能被「{0} 位」抢先匹配。
        .sort((a, b) => literal(b) - literal(a))
        .map((key) => ({
            regex: new RegExp(
                `^${key
                    .split(/\{\d+\}/)
                    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
                    .join("(.+?)")}$`,
            ),
            english: dictionary[key] ?? key,
        }));
    return patterns;
}

/** 翻译核心库和网关返回的文案。这些文案带动态内容，只替换模板，内容原样保留。 */
export function translate(text: string): string {
    if (current === "zh" || !/[\u4e00-\u9fff]/.test(text)) return text;
    const exact = dictionary[text];
    if (exact !== undefined) return exact;
    for (const { regex, english } of compiled()) {
        const match = regex.exec(text);
        if (match) return fill(english, match.slice(1).map(translate));
    }
    return text;
}

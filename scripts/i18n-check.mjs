#!/usr/bin/env node
// 文案都要有英文，漏译时构建失败。词典是 design/i18n.json，规则见 docs/06-ui.md 第 9 节。
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const CJK = /[一-鿿]/;
const english = JSON.parse(readFileSync("design/i18n.json", "utf8"));
// 这些是比较核心库错误文本用的片段，不显示。
const LOGIC = new Set(["大小上限", "白名单", "不匹配", "公钥"]);
const missing = new Set();
const leaked = [];

function files(dir, ext) {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return ["pkg", "__fixtures__", "target"].includes(entry.name) ? [] : files(path, ext);
        return ext.test(entry.name) ? [path] : [];
    });
}

function need(key) {
    if (CJK.test(key) && key !== "中文" && !LOGIC.has(key) && !(key in english)) missing.add(key);
}

const QUOTED = /"(?:[^"\\\n]|\\.)*"/g;

// 网页端的文案在 t() 里，或者是存进 IndexedDB、显示时才翻译的原文。
for (const path of files("apps/web/src", /\.tsx?$/)) {
    if (/\.test\.|test-setup|i18n\.ts$/.test(path)) continue;
    const text = readFileSync(path, "utf8");
    for (const [literal] of text.matchAll(QUOTED)) need(JSON.parse(literal));
    const rest = text
        .replace(QUOTED, "")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/\/\/.*$/gm, "");
    for (const line of rest.split("\n")) {
        // 模板字符串里的原文同样是存进 IndexedDB 的。
        if (CJK.test(line) && !line.includes("`")) leaked.push(`${path}: ${line.trim()}`);
    }
}

// 冷钱包在 Txt 里翻译，字面量的 $变量 对应词典里的 {0}。
for (const path of files("apps/mobile/shared/src/commonMain", /\.kt$/)) {
    if (/Translations\.kt$/.test(path)) continue;
    const text = readFileSync(path, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const line of text.replace(/^\s*\/\/.*$/gm, "").matchAll(/"(?:[^"\\\n]|\\.|\$\{[^}]*\})*"/g)) {
        let index = 0;
        need(line[0].slice(1, -1).replace(/\$\{(?:[^{}]|\{[^{}]*\})*\}|\$[A-Za-z_][\w.]*/g, () => `{${index++}}`));
    }
}

// 核心库和网关错误的文案在显示时按模板翻译。
const rust = ["bitone-core", "bitone-signer", "bitone-ffi", "bitone-wasm"].flatMap((name) =>
    files(`crates/${name}/src`, /\.rs$/),
);
for (const path of [...rust, "crates/bitone-gateway/src/error.rs"]) {
    const text = readFileSync(path, "utf8")
        .split("#[cfg(test)]")[0]
        .replace(/^\s*\/\/.*$/gm, "");
    for (const [literal] of text.matchAll(QUOTED)) {
        let index = 0;
        need(literal.slice(1, -1).replace(/\{[^{}]*\}/g, () => `{${index++}}`));
    }
}

for (const key of missing) console.error(`缺少英文：${key}`);
for (const line of leaked) console.error(`没有翻译的文案：${line}`);
process.exit(missing.size + leaked.length > 0 ? 1 : 0);

#!/usr/bin/env node
// 文案扫描,规则见 docs/06-ui.md 第 8 节:不用"·"分隔信息,不用分号.
// 码点按数值构造,源码不出现被禁止的字面.
import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { pathToFileURL } from "node:url";

const DOT = String.fromCodePoint(0x00b7);
const SEMICOLON = String.fromCodePoint(0xff1b);

const BINARY =
    /\.(png|jpe?g|gif|webp|ico|bmp|tiff?|woff2?|ttf|otf|eot|pdf|zip|gz|tgz|bz2|xz|7z|rar|mp4|webm|mp3|wav|wasm|so|a|db|sqlite3?|bin|der|jar)$/i;

// 引用规则本身时可以出现.
const QUOTED_DOT = `"${DOT}"`;

/** 文件里不算文案的部分:Markdown 的代码块和行内代码. */
function stripCode(line, inFence) {
    return inFence ? "" : line.replace(/`[^`]*`/g, "");
}

/** 返回每处命中的行号 (1 起),列号 (1 起) 和字符.`.md` 和提交信息里半角分号也算. */
export function scanText(text, { prose = false } = {}) {
    const hits = [];
    let inFence = false;
    text.split("\n").forEach((line, index) => {
        if (prose && line.trimStart().startsWith("```")) {
            inFence = !inFence;
            return;
        }
        const body = (prose ? stripCode(line, inFence) : line).replaceAll(QUOTED_DOT, "");
        const banned = prose ? [DOT, SEMICOLON, ";"] : [DOT, SEMICOLON];
        for (const char of banned) {
            let column = body.indexOf(char);
            while (column !== -1) {
                hits.push({ line: index + 1, column: column + 1, char });
                column = body.indexOf(char, column + 1);
            }
        }
    });
    return hits;
}

function git(...args) {
    return execFileSync("git", args, { encoding: "utf8", maxBuffer: 1 << 26 });
}

function trackedFiles(staged) {
    const output = staged
        ? git("diff", "--cached", "--name-only", "--diff-filter=ACMR")
        : git("ls-files", "--cached", "--others", "--exclude-standard");
    return output.split("\n").filter((path) => path && !BINARY.test(path) && !path.startsWith("scripts/copy-check"));
}

function report(path, hits) {
    for (const hit of hits) {
        console.error(`${path}:${hit.line}:${hit.column}: 文案不能用 ${hit.char}`);
    }
    return hits.length;
}

function main(args) {
    if (args[0] === "--commit-msg") {
        const text = readFileSync(args[1], "utf8")
            .split("\n")
            .filter((line) => !line.startsWith("#"))
            .join("\n");
        return report("提交信息", scanText(text, { prose: true }));
    }
    let count = 0;
    for (const path of trackedFiles(args[0] === "--staged")) {
        let text;
        try {
            if (statSync(path).size > 4_000_000) continue;
            text = readFileSync(path, "utf8");
        } catch {
            continue;
        }
        count += report(path, scanText(text, { prose: path.endsWith(".md") }));
    }
    return count;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
    const count = main(process.argv.slice(2));
    if (count > 0) {
        console.error(`共 ${count} 处,见 docs/06-ui.md 第 8 节`);
        process.exit(1);
    }
}

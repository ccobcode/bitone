#!/usr/bin/env node
// 全角标点扫描与修复,覆盖仓库内全部受版本控制的文本文件,含 .md.
// FF01-FF5E 整体减 0xFEE0,其余中文标点和弯引号逐一映射.码点按数值构造,源码不出现全角字面.
import { execFileSync } from "node:child_process";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export const RANGES = [
    [0xff01, 0xff5e],
    [0x3001, 0x3002],
    [0x3008, 0x300f],
    [0x3010, 0x3011],
    [0x201c, 0x201d],
    [0x2018, 0x2019],
];

const EXTRA = {
    0x3002: ".",
    0x3001: ",",
    0x300a: "<",
    0x300b: ">",
    0x300c: '"',
    0x300d: '"',
    0x300e: '"',
    0x300f: '"',
    0x3010: "[",
    0x3011: "]",
    0x201c: '"',
    0x201d: '"',
    0x2018: "'",
    0x2019: "'",
};

const BINARY =
    /\.(png|jpe?g|gif|webp|ico|bmp|tiff?|woff2?|ttf|otf|eot|pdf|zip|gz|tgz|bz2|xz|7z|rar|mp4|webm|mp3|wav|wasm|db|sqlite3?|bin|der)$/i;

const hex = (cp) => "\\u" + cp.toString(16).padStart(4, "0");
const PATTERN = "[" + RANGES.map(([a, b]) => (a === b ? hex(a) : `${hex(a)}-${hex(b)}`)).join("") + "]";

/** 每次新建带 g 标志的正则,避免共享 lastIndex. */
export function fullwidthRegex() {
    return new RegExp(PATTERN, "gu");
}

/** 单个全角字符转半角,不在规则内时原样返回. */
export function toAscii(ch) {
    const cp = ch.codePointAt(0);
    if (cp >= 0xff01 && cp <= 0xff5e) return String.fromCharCode(cp - 0xfee0);
    return EXTRA[cp] ?? ch;
}

/** 返回每处命中的行号和列号 (都从 1 起),原字符和替换值. */
export function scanText(text) {
    const hits = [];
    text.split("\n").forEach((line, lineIndex) => {
        const re = fullwidthRegex();
        let match;
        while ((match = re.exec(line))) {
            hits.push({
                line: lineIndex + 1,
                column: match.index + 1,
                char: match[0],
                ascii: toAscii(match[0]),
            });
        }
    });
    return hits;
}

export function fixText(text) {
    return text.replace(fullwidthRegex(), toAscii);
}

/** 上游的协议文件,哈希记在 `schemas/lock.toml`,不能改. */
const VENDORED = /^schemas\/tron\//;

function repoRoot() {
    return execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim();
}

function gitFiles(args) {
    return execFileSync("git", args, { encoding: "utf8", cwd: repoRoot() }).split("\0").filter(Boolean);
}

function readable(path) {
    if (BINARY.test(path)) return null;
    try {
        if (!statSync(path).isFile()) return null;
        const text = readFileSync(path, "utf8");
        return text.includes("\0") ? null : text;
    } catch {
        return null;
    }
}

function printHits(path, hits) {
    for (const hit of hits) {
        const cp = hit.char.codePointAt(0).toString(16).toUpperCase().padStart(4, "0");
        console.log(`${path}:${hit.line}:${hit.column}  ${hit.char} (U+${cp}) -> ${JSON.stringify(hit.ascii)}`);
    }
}

function commitMessage(file) {
    const text = readFileSync(file, "utf8")
        .split("\n")
        .filter((line) => !line.startsWith("#"))
        .join("\n");
    const hits = scanText(text);
    if (hits.length === 0) return;
    printHits(file, hits);
    console.error(`\n提交信息命中 ${hits.length} 处全角标点, 改用 ASCII 半角.`);
    process.exit(1);
}

function main() {
    const args = process.argv.slice(2);
    const fix = args.includes("--fix");
    const staged = args.includes("--staged");
    const msgIndex = args.indexOf("--commit-msg");
    if (msgIndex >= 0) {
        const file = args[msgIndex + 1];
        if (!file) throw new Error("--commit-msg 需要消息文件路径");
        commitMessage(file);
        return;
    }

    const paths = args.filter((a) => !a.startsWith("--"));
    const files =
        paths.length > 0
            ? paths
            : staged
              ? gitFiles(["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMR"])
              : gitFiles(["ls-files", "-z"]);

    const hitsByFile = [];
    const fixed = [];
    for (const path of files.filter((file) => !VENDORED.test(file))) {
        const text = readable(path);
        if (text === null) continue;
        const hits = scanText(text);
        if (hits.length === 0) continue;
        if (fix) {
            writeFileSync(path, fixText(text));
            fixed.push(path);
            console.log(`fixed  ${path}  (${hits.length})`);
        } else {
            hitsByFile.push([path, hits]);
        }
    }

    if (fix) {
        console.log(`\n修复 ${fixed.length} 个文件.`);
        return;
    }
    for (const [path, hits] of hitsByFile) printHits(path, hits);
    const total = hitsByFile.reduce((n, [, hits]) => n + hits.length, 0);
    if (total > 0) {
        console.error(`\n命中 ${total} 处全角标点, 涉及 ${hitsByFile.length} 个文件. 用 --fix 转换, 或手动修改.`);
        process.exit(1);
    }
    console.log(`检查 ${files.length} 个文件, 未发现全角标点.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    try {
        main();
    } catch (err) {
        console.error(err instanceof Error ? err.message : String(err));
        process.exit(1);
    }
}

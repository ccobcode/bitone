// 网页端的分层约定.依赖只能向下:页面和共享层用注册表,注册表用各链目录,各链用底层.
// 每条链一个目录,目录之间互不引用,共享层不出现具体的链,加一条链只加一个目录,再在注册表接上它.
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { CHAIN_LIST, chainOf, isChain } from ".";
import { networkList } from "../core";
import { initCore } from "../testing";

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CHAIN_NAMES = ["bitcoin", "evm", "tron", "solana"];
/** 不知道任何一条链的底层,各链目录只能往这里引用. */
const FOUNDATION = [
    "core",
    "data/addresses.ts",
    "data/claims.ts",
    "data/db.ts",
    "data/sync.ts",
    "gateway/client.ts",
    "ui/ActionBlock.tsx",
    "ui/Icon.tsx",
    "ui/primitives.tsx",
];
const SHARED = ["data", "gateway", "pages", "ui"];

function walk(path: string): string[] {
    return readdirSync(join(SRC, path), { withFileTypes: true }).flatMap((entry) => {
        const child = join(path, entry.name);
        if (entry.isDirectory()) return entry.name === "pkg" || entry.name === "__fixtures__" ? [] : walk(child);
        return /\.tsx?$/.test(entry.name) && !/\.test\./.test(entry.name) ? [child] : [];
    });
}

/** 去掉注释.注释可以写具体的链. */
function code(path: string): string {
    return readFileSync(join(SRC, path), "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/^\s*\/\/.*$/gm, "")
        .replace(/\s\/\/.*$/gm, "");
}

/** 运行时的引用,解析成相对 src 的路径.`import type` 不进构建产物,不算依赖. */
function imports(path: string): string[] {
    const found = code(path).matchAll(/^(?:import|export) (?!type )[^;]*?from "(\.[^"]+)";/gm);
    return [...found].map(([, target]) => relative(SRC, resolve(SRC, dirname(path), target ?? "")));
}

function isUnder(path: string, root: string): boolean {
    return path === root || path === root.replace(/\.tsx?$/, "") || path.startsWith(`${root}/`);
}

const chainDirs = readdirSync(join(SRC, "chains"), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);

describe("架构", () => {
    it("共享层不出现具体的链", () => {
        const named = new RegExp(`["'\`](${CHAIN_NAMES.join("|")})["'\`]`);
        const identifier = /Bitcoin|Evm|EVM|Tron|TRON|Solana|Psbt|PSBT/;
        for (const layer of SHARED) {
            for (const path of walk(layer)) {
                expect(code(path), path).not.toMatch(named);
                expect(code(path), path).not.toMatch(identifier);
            }
        }
    });

    it("核心库只在 Chain 类型里写链名", () => {
        const named = new RegExp(`"(${CHAIN_NAMES.join("|")})"`);
        for (const path of walk("core")) {
            const lines = code(path).split("\n");
            const hits = lines.filter((line) => named.test(line) && !line.startsWith("export type Chain ="));
            expect(hits, path).toEqual([]);
        }
    });

    it("只有注册表能接触各链目录,按链分发也只在这里", () => {
        const dispatch = /(?:switch \(\S*\.?chain\)|\.chain [!=]== ["'])/;
        const outside = [...SHARED, "core", "chains"].flatMap(walk).filter((path) => {
            return path !== "chains/index.ts" && !chainDirs.some((chain) => isUnder(path, join("chains", chain)));
        });
        for (const path of outside) {
            expect(code(path), path).not.toMatch(dispatch);
            for (const target of imports(path)) {
                for (const chain of chainDirs) expect(isUnder(target, join("chains", chain)), path).toBe(false);
            }
        }
    });

    it("依赖只能向下", () => {
        for (const root of FOUNDATION) {
            const files = root.includes(".") ? [root] : walk(root);
            for (const path of files) {
                for (const target of imports(path)) expect(isUnder(target, "chains"), path).toBe(false);
            }
        }
        // `..` 开头的是 src 之外的图标.
        const allowed = (target: string) =>
            target.startsWith("..") || isUnder(target, "chains") || FOUNDATION.some((root) => isUnder(target, root));
        for (const path of walk("chains").filter((file) => file !== "chains/index.ts")) {
            for (const target of imports(path)) expect(allowed(target), `${path} -> ${target}`).toBe(true);
        }
    });

    it("各链目录互不引用", () => {
        expect(chainDirs.length).toBeGreaterThanOrEqual(4);
        for (const chain of chainDirs) {
            const own = join("chains", chain);
            for (const path of walk(own)) {
                for (const other of chainDirs.filter((name) => name !== chain)) {
                    for (const target of imports(path)) {
                        expect(isUnder(target, join("chains", other)), path).toBe(false);
                    }
                }
                expect(imports(path), path).not.toContain("chains/index");
                expect(imports(path), path).not.toContain("chains");
            }
        }
    });

    it("核心库的每个网络都有注册的链", () => {
        initCore();
        for (const { chain } of networkList()) expect(isChain(chain), chain).toBe(true);
        expect(CHAIN_LIST.map((spec) => spec.id)).toEqual(CHAIN_NAMES);
        for (const name of CHAIN_NAMES) expect(isChain(name)).toBe(true);
        for (const spec of CHAIN_LIST) expect(chainOf(spec.id)).toBe(spec);
        expect(isChain("toString")).toBe(false);
    });
});

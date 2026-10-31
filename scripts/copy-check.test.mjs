import assert from "node:assert/strict";
import { test } from "node:test";
import { scanText } from "./copy-check.mjs";

const DOT = String.fromCodePoint(0x00b7);
const SEMICOLON = String.fromCodePoint(0xff1b);

test("分隔点和全角分号都算", () => {
    assert.equal(scanText(`主网 ${DOT} 测试网`).length, 1);
    assert.deepEqual(scanText(`第一行\n说明${SEMICOLON}继续`)[0], { line: 2, column: 3, char: SEMICOLON });
});

test("代码里的半角分号不算，文档里的算", () => {
    assert.equal(scanText("const a = 1;").length, 0);
    assert.equal(scanText("说明; 继续", { prose: true }).length, 1);
});

test("文档的代码块和行内代码不算", () => {
    assert.equal(scanText("```\na;\n```\n正文", { prose: true }).length, 0);
    assert.equal(scanText("调用 `a; b` 之后", { prose: true }).length, 0);
});

test("引用规则本身不算", () => {
    assert.equal(scanText(`不用「${DOT}」分隔`).length, 0);
});

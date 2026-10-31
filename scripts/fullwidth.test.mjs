import assert from "node:assert/strict";
import test from "node:test";
import { fixText, fullwidthRegex, scanText, toAscii } from "./fullwidth.mjs";

const cp = (code) => String.fromCodePoint(code);

test("toAscii maps fullwidth ASCII forms by the 0xFEE0 offset", () => {
    assert.equal(toAscii(cp(0xff0c)), ",");
    assert.equal(toAscii(cp(0xff1a)), ":");
    assert.equal(toAscii(cp(0xff01)), "!");
    assert.equal(toAscii(cp(0xff08)), "(");
    assert.equal(toAscii(cp(0xff5e)), "~");
});

test("toAscii maps CJK punctuation and curly quotes", () => {
    assert.equal(toAscii(cp(0x3002)), ".");
    assert.equal(toAscii(cp(0x3001)), ",");
    assert.equal(toAscii(cp(0x300c)), '"');
    assert.equal(toAscii(cp(0x3010)), "[");
    assert.equal(toAscii(cp(0x201c)), '"');
    assert.equal(toAscii(cp(0x2019)), "'");
});

test("toAscii keeps ASCII and Chinese characters unchanged", () => {
    assert.equal(toAscii(","), ",");
    assert.equal(toAscii("A"), "A");
    assert.equal(toAscii("中"), "中");
});

test("scanText reports line and column for every hit", () => {
    const text = ["ok", `a${cp(0xff0c)}b${cp(0x3002)}`, `x${cp(0x3010)}y${cp(0x3011)}`].join("\n");
    const hits = scanText(text);
    assert.deepEqual(
        hits.map((h) => [h.line, h.column, h.ascii]),
        [
            [2, 2, ","],
            [2, 4, "."],
            [3, 2, "["],
            [3, 4, "]"],
        ],
    );
});

test("fixText converts all hits and leaves the rest intact", () => {
    const input = `${cp(0xff08)}a${cp(0xff0c)}b${cp(0x3002)}${cp(0xff09)}`;
    assert.equal(fixText(input), "(a,b.)");
    assert.deepEqual(scanText(fixText(input)), []);
});

test("fullwidthRegex returns independent instances", () => {
    const a = fullwidthRegex();
    const b = fullwidthRegex();
    a.test(cp(0xff0c));
    assert.equal(b.lastIndex, 0);
});

import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";
import { initCoreSync } from ".";
import { lookupAsset, priceKey } from "./manifest";

const USDT = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

beforeAll(() => {
    initCoreSync(readFileSync(new URL("./pkg/bitone_wasm_bg.wasm", import.meta.url)));
});

describe("priceKey", () => {
    it("主网资产用自己的键", () => {
        expect(priceKey("ethereum", USDT)).toBe(`ethereum:${USDT.toLowerCase()}`);
        expect(priceKey("bitcoin", "native")).toBe("bitcoin:native");
    });

    it("测试网资产用同链同符号的主网资产", () => {
        expect(priceKey("btc-signet", "native")).toBe("bitcoin:native");
        expect(priceKey("sepolia", "native")).toBe("ethereum:native");
        expect(priceKey("bsc-testnet", "native")).toBe("bsc:native");
        expect(priceKey("amoy", "native")).toBe("polygon:native");
        expect(priceKey("solana-devnet", "native")).toBe("solana:native");
        expect(priceKey("tron-nile", "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf")).toBe(
            "tron:TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        );
    });

    it("不在清单里的测试网代币没有对应的主网资产", () => {
        expect(priceKey("sepolia", USDT)).toBe(`sepolia:${USDT.toLowerCase()}`);
    });
});

describe("lookupAsset", () => {
    it("按网络加合约地址查到", () => {
        expect(lookupAsset("ethereum", USDT)).toMatchObject({ symbol: "USDT", decimals: 6 });
        expect(lookupAsset("tron", "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t")).toMatchObject({
            symbol: "USDT",
            decimals: 6,
        });
        expect(lookupAsset("ethereum", "native")).toMatchObject({ symbol: "ETH", decimals: 18, native: true });
    });

    it("不按符号或名称匹配", () => {
        expect(lookupAsset("ethereum", "USDT")).toBeNull();
        expect(lookupAsset("ethereum", "Tether USD")).toBeNull();
        expect(lookupAsset("tron", "usdt")).toBeNull();
    });

    it("仿冒合约查不到", () => {
        expect(lookupAsset("ethereum", "0x0000000000000000000000000000000000000001")).toBeNull();
    });

    it("同一地址换网络查不到", () => {
        expect(lookupAsset("bsc", USDT)).toBeNull();
        expect(lookupAsset("polygon", USDT)).toBeNull();
        expect(lookupAsset("sepolia", USDT)).toBeNull();
    });

    it("同一符号在不同网络的小数位不同", () => {
        expect(lookupAsset("bsc", "0x55d398326f99059fF775485246999027B3197955")).toMatchObject({ decimals: 18 });
    });

    it("EVM 地址忽略大小写", () => {
        expect(lookupAsset("ethereum", USDT.toLowerCase())?.symbol).toBe("USDT");
        // docs/09 的示例就是这种写法。
        expect(lookupAsset("ethereum", "0xdAC17F958D2ee523a2206206994597C13D831EC7")?.symbol).toBe("USDT");
    });

    it("Base58 地址区分大小写", () => {
        expect(lookupAsset("tron", "tr7nhqjekqxgtci8q8zy4pl8otszgjlj6t")).toBeNull();
        expect(lookupAsset("solana", "epjfwdd5aufqssqem2qn1xzybapc8g4wegGkzwytdt1v")).toBeNull();
    });
});

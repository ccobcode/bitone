import { describe, expect, it } from "vitest";
import { groupAddress } from "./text";

const groups = (address: string) => groupAddress(address).map((group) => group.text);

describe("groupAddress", () => {
    // 分段和冷钱包的 addressChunks 相同，用户要两边逐段对比。
    it("0x 连着前四位", () => {
        expect(groups("0x70997970C51812dc")).toEqual(["0x7099", "7970", "C518", "12dc"]);
    });

    it("其他地址每四位一段，最后一段可以不满", () => {
        expect(groups("bc1qcr8te4kr609")).toEqual(["bc1q", "cr8t", "e4kr", "609"]);
    });
});

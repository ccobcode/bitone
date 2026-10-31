import { describe, expect, it } from "vitest";
import { formatUnits, formatUsd, parseAmount, parseUnits, toInput, usdCents } from "./amount";

describe("parseAmount", () => {
    it("只接受十进制整数字符串", () => {
        expect(parseAmount("100000000")).toBe(100000000n);
        expect(parseAmount("0")).toBe(0n);
        for (const bad of ["", " 1", "0x10", "1e3", "-1", "1.5", 12, null, undefined, "9".repeat(79)]) {
            expect(parseAmount(bad)).toBeNull();
        }
    });
});

describe("formatUnits", () => {
    it("换算小数位并分组", () => {
        expect(formatUnits(1000000000000000000n, 18)).toBe("1");
        expect(formatUnits(2500000n, 6)).toBe("2.5");
        expect(formatUnits(1234567890123n, 6)).toBe("1,234,567.890123");
        expect(formatUnits(0n, 18)).toBe("0");
        expect(formatUnits(5n, 0)).toBe("5");
    });

    it("超过浮点精度时不丢位", () => {
        expect(formatUnits(123456789012345678901234567890n, 18)).toBe("123,456,789,012.34567890123456789");
        expect(formatUnits(9007199254740993n, 0)).toBe("9,007,199,254,740,993");
        expect(formatUnits(1n, 18)).toBe("0.000000000000000001");
    });

    it("按最少和最多小数位显示", () => {
        expect(formatUnits(48210000n, 8, { min: 8 })).toBe("0.48210000");
        expect(formatUnits(5000000000n, 6, { min: 2 })).toBe("5,000.00");
        expect(formatUnits(2150300000000000000n, 18, { min: 4, max: 8 })).toBe("2.1503");
    });

    it("超出最多小数位时舍去，不进位", () => {
        expect(formatUnits(1999999999999999999n, 18, { max: 8 })).toBe("1.99999999");
        expect(formatUnits(1n, 18, { max: 8 })).toBe("<0.00000001");
    });
});

describe("parseUnits", () => {
    it("转成最小单位，不用浮点数", () => {
        expect(parseUnits("1.5", 18)).toBe(1_500_000_000_000_000_000n);
        expect(parseUnits("0.1", 8)).toBe(10_000_000n);
        expect(parseUnits("1,250.00", 6)).toBe(1_250_000_000n);
        expect(parseUnits("5", 0)).toBe(5n);
        expect(parseUnits("5.", 6)).toBe(5_000_000n);
    });

    it("格式不对或小数位超出时返回 null", () => {
        expect(parseUnits("", 6)).toBeNull();
        expect(parseUnits("abc", 6)).toBeNull();
        expect(parseUnits("-1", 6)).toBeNull();
        expect(parseUnits("1e5", 6)).toBeNull();
        expect(parseUnits("0x10", 6)).toBeNull();
        expect(parseUnits(".5", 6)).toBeNull();
        expect(parseUnits("0.1234567", 6)).toBeNull();
    });

    it("和 toInput 互逆", () => {
        expect(toInput(parseUnits("1234.5678", 18) ?? 0n, 18)).toBe("1234.5678");
    });
});

describe("美元估值", () => {
    it("用 BigInt 计算，舍去分以下", () => {
        expect(usdCents(10n ** 18n, 18, "3400.12")).toBe(340_012n);
        expect(usdCents(1_500_000n, 6, "1.00")).toBe(150n);
        expect(usdCents(1n, 18, "3400.12")).toBe(0n);
        expect(formatUsd(123_456_789n)).toBe("$1,234,567.89");
        expect(formatUsd(5n)).toBe("$0.05");
    });

    it("价格格式不对时没有估值", () => {
        expect(usdCents(1n, 0, "abc")).toBeNull();
        expect(usdCents(1n, 0, "1e3")).toBeNull();
    });
});

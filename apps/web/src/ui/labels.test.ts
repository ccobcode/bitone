import { describe, expect, it } from "vitest";
import { dayLabel, timeLabel } from "./labels";

const now = new Date(2026, 9, 31, 12, 0).getTime();

describe("timeLabel", () => {
    it("今天和昨天写字,今年的写月日,往年的写全", () => {
        expect(timeLabel(new Date(2026, 9, 31, 9, 5).getTime(), now)).toBe("今天 09:05");
        expect(timeLabel(new Date(2026, 9, 30, 23, 59).getTime(), now)).toBe("昨天 23:59");
        expect(timeLabel(new Date(2026, 0, 2, 7, 0).getTime(), now)).toBe("01-02 07:00");
        expect(timeLabel(new Date(2025, 11, 31, 7, 0).getTime(), now)).toBe("2025-12-31 07:00");
    });

    it("月初的昨天跨到上个月", () => {
        const first = new Date(2026, 10, 1, 8, 0).getTime();
        expect(dayLabel(new Date(2026, 9, 31, 8, 0).getTime(), first)).toBe("昨天");
    });
});

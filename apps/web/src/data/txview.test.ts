import { describe, expect, it } from "vitest";
import type { TxRecord } from "./db";
import { finality } from "./txview";

describe("finality", () => {
    it("TRON 的阶段和网关的 stage 一致,失败的交易也能走到区块确认", () => {
        const tx = { chain: "tron", state: "failed", stage: "finalized", confirmations: 19 } as TxRecord;
        expect(finality(tx)).toMatchObject({ reached: 2, progress: "区块确认" });
    });
});

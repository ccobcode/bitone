import { beforeAll, describe, expect, it } from "vitest";
import { BTC_ADDRESS, EVM_ADDRESS, TRON_ADDRESS, USDT, initCore, must } from "../testing";
import type { Account } from "../data/db";
import { lookupAsset } from "../core/manifest";
import { assetShares, groupPending, groupRows, largestRow, type AssetRow } from "./assets";

beforeAll(initCore);

const USDT_BSC = "0x55d398326f99059fF775485246999027B3197955";
const USDT_TRON = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
const account = (id: string) => ({ id, chain: "evm", kind: "single" }) as Account;

function row(network: string, asset: string, amount: bigint, extra: Partial<AssetRow> = {}): AssetRow {
    return {
        id: `${network}|${asset}`,
        account: account("a"),
        network,
        asset,
        entry: lookupAsset(network, asset),
        amount,
        loaded: true,
        address: EVM_ADDRESS,
        ...extra,
    };
}

describe("groupRows", () => {
    it("同一个币跨网络合并,不同小数位先换算", () => {
        const [group] = groupRows([row("ethereum", USDT, 5_000_000n), row("bsc", USDT_BSC, 10n ** 18n)], null);
        expect(group?.key).toBe("usdt");
        expect(group?.decimals).toBe(18);
        expect(group?.amount).toBe(6n * 10n ** 18n);
        expect(group?.rows).toHaveLength(2);
    });

    it("清单外的代币不合并,符号再像也不行", () => {
        const fake = "0x0000000000000000000000000000000000000001";
        const groups = groupRows([row("ethereum", USDT, 1n), row("ethereum", fake, 9n)], null);
        expect(groups.map((group) => group.key)).toEqual(["usdt"]);
        expect(groups[0]?.amount).toBe(1n);
    });

    it("有余额的按估值排序,没估值的其次,零余额最后", () => {
        const prices = new Map([
            ["ethereum:native", "3000"],
            ["bitcoin:native", "60000"],
            [`ethereum:${USDT.toLowerCase()}`, "1"],
        ]);
        const groups = groupRows(
            [
                row("ethereum", "native", 0n),
                row("ethereum", USDT, 1000_000000n),
                row("bitcoin", "native", 100_000_000n, { address: BTC_ADDRESS }),
                row("tron", "native", 5_000000n, { address: TRON_ADDRESS }),
            ],
            prices,
        );
        expect(groups.map((group) => group.key)).toEqual(["btc", "usdt", "trx", "eth"]);
        expect(groups.at(-1)?.empty).toBe(true);
    });

    it("没查到的网络不算零余额", () => {
        const [group] = groupRows([row("ethereum", "native", 0n, { loaded: false })], null);
        expect(group).toMatchObject({ loaded: false, empty: false });
    });
});

describe("largestRow", () => {
    it("比较时按换算后的金额,小数位多的不占便宜", () => {
        const [group] = groupRows([row("ethereum", USDT, 5_000_000n), row("bsc", USDT_BSC, 10n ** 18n)], null);
        expect(largestRow(must(group)).network).toBe("ethereum");
    });
});

describe("groupPending", () => {
    it("待转出的合计用同一个单位", () => {
        const [group] = groupRows([row("ethereum", USDT, 5_000_000n), row("tron", USDT_TRON, 0n)], null);
        const tx = {
            kind: "transfer",
            state: "signing",
            accountId: "a",
            network: "ethereum",
            asset: USDT,
            amount: "2",
        };
        expect(groupPending(must(group), [tx as never])).toBe(2n);
    });
});

describe("assetShares", () => {
    const group = (network: string, cents: string) => {
        const one = 10n ** BigInt(must(lookupAsset(network, "native")).decimals);
        const [item] = groupRows([row(network, "native", one)], new Map([[`${network}:native`, cents]]));
        return must(item);
    };

    it("占比按估值算,没有估值时不显示", () => {
        const shares = assetShares([group("ethereum", "3"), group("bsc", "1")]);
        expect(shares.map((share) => share.percent)).toEqual([75, 25]);
        expect(assetShares(groupRows([row("ethereum", "native", 1n)], null))).toEqual([]);
    });

    it("超过五项时最小的合成其他,只多一项时不合并", () => {
        const networks = ["ethereum", "bsc", "polygon", "tron", "solana", "bitcoin"];
        const groups = networks.map((network, i) => group(network, String(10 - i)));
        expect(assetShares(groups).map((share) => share.key)).toEqual(["eth", "bnb", "pol", "trx", "other"]);
        expect(assetShares(groups.slice(0, 5)).map((share) => share.key)).toEqual(["eth", "bnb", "pol", "trx", "sol"]);
    });
});

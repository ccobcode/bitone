import { must } from "../testing";
import { generateKeyPairSync, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";
import { FrameReceiver } from "./pkg/bitone_wasm";
import {
    PairingReceiver,
    UrReceiver,
    addressFrame,
    deriveAddress,
    initCoreSync,
    manifestPackageVersion,
    networkList,
    networkName,
    qrMatrix,
    qrText,
    validateAddress,
    verifyResponse,
    type Pairing,
} from ".";
import { formatAsset, lookupAsset } from "./manifest";
import { lookalike } from "./text";

const frames = readFileSync(new URL("./__fixtures__/pairing.txt", import.meta.url), "utf8")
    .trim()
    .split("\n");

function decode(): Pairing {
    const receiver = new PairingReceiver();
    for (const frame of frames) {
        receiver.feed(frame);
        if (receiver.complete) break;
    }
    expect(receiver.complete).toBe(true);
    return receiver.pairing();
}

function address(pairing: Pairing, path: string): string {
    const account = pairing.accounts.find((a) => a.path === path);
    if (!account) throw new Error(path);
    return deriveAddress(account, "mainnet", 0, 0);
}

beforeAll(() => {
    initCoreSync(readFileSync(new URL("./pkg/bitone_wasm_bg.wasm", import.meta.url)));
});

describe("配对数据", () => {
    it("解出指纹和四条链的地址", () => {
        const pairing = decode();
        expect(pairing.fingerprint).toBe("73C5DA0A");
        expect("testnet" in pairing).toBe(false);
        expect(address(pairing, "m/84'/0'/0'")).toBe("bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu");
        expect(address(pairing, "m/44'/60'/0'/0/0")).toBe("0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
        expect(address(pairing, "m/44'/195'/0'/0/0")).toBe("TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH");
        expect(address(pairing, "m/44'/501'/0'/0'")).toBe("HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk");
    });

    it("一次导出全部账户，按 scope 区分主网和测试网", () => {
        const scopes = new Map(decode().accounts.map((account) => [account.path, account.scope]));
        expect(scopes.size).toBe(9);
        expect(scopes.get("m/84'/0'/0'")).toBe("mainnet");
        expect(scopes.get("m/84'/1'/0'")).toBe("testnet");
        expect(scopes.get("m/44'/60'/0'/0/0")).toBe("both");
    });

    it("帧顺序打乱也能解出，并报告帧数和进度", () => {
        const receiver = new PairingReceiver();
        for (const frame of [...frames].reverse()) {
            receiver.feed(frame.toUpperCase());
            if (receiver.complete) break;
        }
        expect(receiver.complete).toBe(true);
        expect(receiver.frames).toBe(4);
        expect(receiver.progress).toBe(1);
    });

    it("误扫别的二维码不影响后续帧", () => {
        const receiver = new PairingReceiver();
        expect(receiver.feed("https://example.com")).toBe("foreign");
        expect(receiver.feed("ur:bitone-request/1-2/abc")).toBe("foreign");
        expect(receiver.feed(must(frames[0]))).toBe("accepted");
        expect(receiver.feed(must(frames[0]))).toBe("repeated");
        expect(receiver.feed("ur:bitone-accounts/1-3/zzzz")).toBe("invalid");
    });

    it("网络表来自核心库", () => {
        const list = networkList();
        expect(list).toHaveLength(17);
        expect(list.find((n) => n.id === "sepolia")).toMatchObject({ chain: "evm", testnet: true, chainId: 11155111 });
    });
});

describe("地址核对二维码", () => {
    const check = {
        fingerprint: "73C5DA0A",
        network: "solana",
        path: "m/44'/501'/0'/0'",
        address: "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk",
    };

    const frame = (c: typeof check) => addressFrame(c.fingerprint, c.network, c.path, c.address);

    it("核心库的帧解码器能收下，大写后也一样", () => {
        for (const part of [frame(check), frame(check).toUpperCase()]) {
            const receiver = new FrameReceiver();
            expect(receiver.receive(part)).toBe(1);
            expect(receiver.complete()).toBe(true);
            receiver.free();
        }
    });

    it("路径与网络冲突时拒绝生成", () => {
        expect(() => frame({ ...check, network: "solana-devnet" })).toThrow();
        expect(() => frame({ ...check, fingerprint: "xyz" })).toThrow();
    });

    it("收款二维码保留地址的大小写", () => {
        expect(qrText(check.address)).not.toEqual(qrText(check.address.toUpperCase()));
    });

    it("能画成二维码", () => {
        const matrix = qrMatrix(frame(check));
        expect(matrix.modules).toHaveLength(matrix.size * matrix.size);
    });
});

describe("收款地址校验", () => {
    it("返回规范写法，拒绝别的网络", () => {
        expect(validateAddress("ethereum", " 0x9858effd232b4033e47d90003d41ec34ecaeda94 ")).toBe(
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
        );
        expect(() => validateAddress("ethereum", "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu")).toThrow();
        expect(() => validateAddress("bitcoin", "tb1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu")).toThrow();
    });
});

describe("相似地址", () => {
    const a = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

    it("首尾相同中间不同才算", () => {
        expect(lookalike(a, "0x7099aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadc79C8")).toBe(true);
        expect(lookalike(a, a)).toBe(false);
        expect(lookalike(a, a.toLowerCase())).toBe(false);
        expect(lookalike(a, "0x1111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadc79C8")).toBe(false);
        expect(lookalike(a, "0x7099aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadc7911")).toBe(false);
    });

    it("短格式看起来一样就算相似，Bitcoin 只显示前缀后两位", () => {
        expect(lookalike("bc1qk7aaxxxxxxxxzzzz", "bc1qk7bbyyyyyyyyzzzz")).toBe(true);
    });

    it("固定前缀不算相同，Base58 区分大小写", () => {
        expect(lookalike("bc1qaaaaxxxxxxxxzzzz", "bc1qbbbbyyyyyyyyzzzz")).toBe(false);
        expect(lookalike("TAbcdXXXXXXXXXXwxyz", "TAbcdYYYYYYYYYYwxyz")).toBe(true);
        expect(lookalike("TAbcdXXXXXXXXXXwxyz", "tabcdYYYYYYYYYYwxyz")).toBe(false);
    });
});

describe("内置清单", () => {
    it("来自核心库，覆盖全部网络的原生币", () => {
        for (const network of networkList()) expect(lookupAsset(network.id, "native"), network.id).not.toBeNull();
        expect(formatAsset("ethereum", "native", 10n ** 18n)).toBe("1.0000 ETH");
        expect(formatAsset("ethereum", "0x0000000000000000000000000000000000000001", 7n)).toBe("7 未知代币");
    });
});

describe("清单包", () => {
    // 临时生成的密钥。厂商私钥不进仓库，这里只能测拒绝，接受的路径由 Rust 的测试覆盖。
    function pack(version: number): Uint8Array {
        const { privateKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
        const body = Buffer.from(JSON.stringify({ version, assets: [] }));
        return Uint8Array.from(Buffer.concat([sign("sha256", body, privateKey), body]));
    }

    it("不是厂商密钥签的包被拒绝", () => {
        expect(() => manifestPackageVersion(pack(7))).toThrow("签名无效");
    });

    it("太短或不是清单包时拒绝", () => {
        expect(() => manifestPackageVersion(new Uint8Array(10))).toThrow();
        expect(() => manifestPackageVersion(new Uint8Array(200))).toThrow();
    });
});

describe("网络名", () => {
    it("来自核心库，未知网络原样返回", () => {
        expect(networkName("bsc")).toBe("BNB Smart Chain");
        expect(networkName("btc-testnet4")).toBe("Testnet4");
        expect(networkName("nope")).toBe("nope");
    });
});

describe("签名核验", () => {
    const fixture = JSON.parse(readFileSync(new URL("./__fixtures__/evm-signed.json", import.meta.url), "utf8")) as {
        request: string;
        frames: string[];
        key: string;
    };
    const bytes = (hex: string) => Uint8Array.from(Buffer.from(hex, "hex"));

    function response(): Uint8Array {
        const receiver = new UrReceiver("bitone-response");
        for (const frame of fixture.frames) if (!receiver.complete) receiver.feed(frame);
        expect(receiver.complete).toBe(true);
        return receiver.message();
    }

    it("真实冷钱包的签名能通过，得到可广播的交易和哈希", () => {
        const verified = verifyResponse(bytes(fixture.request), response(), bytes(fixture.key));
        expect(verified.raw).toMatch(/^02/);
        expect(verified.hash).toMatch(/^0x[0-9a-f]{64}$/);
    });

    it("换了公钥不通过", () => {
        const other = bytes(fixture.key);
        other[5] = (other[5] ?? 0) ^ 1;
        expect(() => verifyResponse(bytes(fixture.request), response(), other)).toThrow();
    });

    it("请求 ID 不一致不通过", () => {
        const request = bytes(fixture.request);
        const at = Buffer.from(request).indexOf(Buffer.from([0x50]));
        request[at + 1] = (request[at + 1] ?? 0) ^ 0xff;
        expect(() => verifyResponse(request, response(), bytes(fixture.key))).toThrow("不匹配");
    });

    it("改过的交易内容不通过", () => {
        const request = bytes(fixture.request);
        request[request.length - 1] = (request[request.length - 1] ?? 0) ^ 1;
        expect(() => verifyResponse(request, response(), bytes(fixture.key))).toThrow();
    });

    it("接收器只收 bitone-response，误扫别的二维码不影响后面的帧", () => {
        const receiver = new UrReceiver("bitone-response");
        expect(receiver.feed("ur:bitone-accounts/1-2/abc")).toBe("foreign");
        expect(receiver.feed("https://example.com")).toBe("foreign");
        expect(receiver.feed(must(fixture.frames[0]))).toBe("accepted");
        receiver.free();
    });
});

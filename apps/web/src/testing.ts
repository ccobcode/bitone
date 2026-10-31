// 测试共用的工具，不会进入构建产物。
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { vi } from "vitest";
import { PairingReceiver, initCoreSync, type Pairing } from "./core";
import { lookupAsset } from "./core/manifest";
import { db, type Account, type Wallet } from "./data/db";
import { savePairing, walletId } from "./data/pairing";
import type { TransferDraft } from "./data/transfer";
import { t } from "./core/i18n";

export const EVM_ADDRESS = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
export const TRON_ADDRESS = "TUEZSdKsoDHQMeZwihtdoBiN46zxhGWYdH";
export const SOLANA_ADDRESS = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";
export const BTC_ADDRESS = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu";
export const PEER_EVM = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
export const USDT = "0xdAC17F958D2ee523a2206206994597C13D831ec7";

/** 取出一定存在的值，不存在时测试失败。 */
export function must<T>(value: T | null | undefined): T {
    if (value === null || value === undefined) throw new Error(t("值不存在"));
    return value;
}

export function initCore(): void {
    initCoreSync(readFileSync(new URL("./core/pkg/bitone_wasm_bg.wasm", import.meta.url)));
}

export function loadPairing(): Pairing {
    const receiver = new PairingReceiver();
    const frames = readFileSync(new URL("./core/__fixtures__/pairing.txt", import.meta.url), "utf8")
        .trim()
        .split("\n");
    for (const frame of frames) if (!receiver.complete) receiver.feed(frame);
    return receiver.pairing();
}

export interface Fixture {
    pairing: Pairing;
    wallet: Wallet;
    accounts: Account[];
    account(path: string): Account;
}

/** 配对真实的测试钱包。助记词是 abandon×11 about。 */
export async function pairWallet(): Promise<Fixture> {
    const pairing = loadPairing();
    await savePairing(pairing, {
        name: t("主钱包"),
        paths: new Set(pairing.accounts.map((a) => a.path)),
        mode: "mainnet",
    });
    const id = walletId(pairing.fingerprint, "mainnet");
    const wallet = must(await db.wallets.get(id));
    const accounts = await db.accounts.where("walletId").equals(id).toArray();
    return {
        pairing,
        wallet,
        accounts,
        account(path) {
            const found = accounts.find((a) => a.path === path);
            if (!found) throw new Error(path);
            return found;
        },
    };
}

export const PATHS = {
    evm: "m/44'/60'/0'/0/0",
    tron: "m/44'/195'/0'/0/0",
    solana: "m/44'/501'/0'/0'",
    segwit: "m/84'/0'/0'",
    taproot: "m/86'/0'/0'",
};

export function draftFor(
    fixture: Fixture,
    path: string,
    from: string,
    network: string,
    to: string,
    amount: bigint,
    asset = "native",
): TransferDraft {
    const entry = lookupAsset(network, asset);
    if (!entry) throw new Error(asset);
    return {
        wallet: fixture.wallet,
        accounts: fixture.accounts,
        account: fixture.account(path),
        from,
        network,
        asset,
        entry,
        to,
        amount,
        tier: "medium",
    };
}

export type Route = (body: Record<string, unknown>) => unknown;

/** 假网关。`routes` 的键是 `/v1/` 之后的路径，返回 Response 时原样使用。 */
export function stubGateway(routes: Record<string, Route>) {
    const calls: string[] = [];
    const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
        const route = path.replace(/^\/v1\//, "");
        calls.push(route);
        const handler = routes[route];
        if (!handler) return Response.json({ error: { code: "not_stubbed", message: route } }, { status: 500 });
        const body = init?.body ? (JSON.parse(init.body as string) as Record<string, unknown>) : {};
        const reply = await handler(body);
        return reply instanceof Response ? reply : Response.json(reply);
    });
    vi.stubGlobal("fetch", fetchMock);
    return { calls, fetchMock };
}

export function gatewayError(code: string, message = "", status = 400): Response {
    return Response.json({ error: { code, message } }, { status });
}

const CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/** 测试里把地址转成输出脚本，只支持 SegWit v0 和 Taproot。 */
export function scriptFor(address: string): Buffer {
    const data = [...address.slice(address.lastIndexOf("1") + 1, -6)].map((char) => CHARSET.indexOf(char));
    const version = data[0] ?? 0;
    const bytes: number[] = [];
    let acc = 0;
    let bits = 0;
    for (const value of data.slice(1)) {
        acc = (acc << 5) | value;
        bits += 5;
        if (bits >= 8) {
            bits -= 8;
            bytes.push((acc >> bits) & 0xff);
            acc &= (1 << bits) - 1;
        }
    }
    return Buffer.from([version === 0 ? 0 : 0x50 + version, bytes.length, ...bytes]);
}

function u64(value: number): Buffer {
    const buffer = Buffer.alloc(8);
    buffer.writeBigUInt64LE(BigInt(value));
    return buffer;
}

/** 造一笔付给 `script` 的前序交易，返回 txid 和十六进制。`padding` 是额外的输出数量，用来撑大交易。 */
export function fakePrevTx(script: Buffer, amount: number, salt = 0, padding = 0): { txid: string; hex: string } {
    const input = Buffer.concat([
        Buffer.alloc(32, salt + 1),
        Buffer.from([0, 0, 0, 0]),
        Buffer.from([0]),
        Buffer.from([0xff, 0xff, 0xff, 0xff]),
    ]);
    const output = (value: number) => Buffer.concat([u64(value), Buffer.from([script.length]), script]);
    const count = padding + 1;
    const countBytes = count < 0xfd ? Buffer.from([count]) : Buffer.from([0xfd, count & 0xff, count >> 8]);
    const outputs = [output(amount), ...Array.from({ length: padding }, () => output(1))];
    const raw = Buffer.concat([
        Buffer.from([2, 0, 0, 0]),
        Buffer.from([1]),
        input,
        countBytes,
        ...outputs,
        Buffer.alloc(4),
    ]);
    const hash = createHash("sha256").update(createHash("sha256").update(raw).digest()).digest();
    return { txid: Buffer.from(hash).reverse().toString("hex"), hex: raw.toString("hex") };
}

// 各链构建交易共用的部分.协议和编码都在核心库里,这里只负责取数据和生成记录.
import {
    accountKeyList,
    buildPayloadRequest,
    previewRequest,
    validateAddress,
    type RequestHeader,
    type Review,
} from "../core";
import { t, translate } from "../core/i18n";
import type { AssetEntry } from "../core/manifest";
import type { Account, NewTx, TxKind, TxPlan, Wallet } from "../data/db";
import { GatewayError, type FeeTier } from "../gateway/client";

/** 网页端的 30 分钟只是手续费报价的有效期,见 docs/03-system.md 第 9 节. */
export const QUOTE_LIFETIME = 30 * 60_000;

export type TransferErrorCode =
    | "revert"
    | "insufficient"
    | "no-nonce-account"
    | "nonce-busy"
    | "nonce-held"
    | "duplicate"
    | "prev-tx"
    | "too-large"
    | "unsupported"
    | "invalid";

export class TransferError extends Error {
    readonly code: TransferErrorCode;
    constructor(code: TransferErrorCode, message: string) {
        super(message);
        this.code = code;
    }
}

/** 生成请求失败时给用户看的话. */
export function failureText(error: unknown): string {
    if (error instanceof TransferError) return translate(error.message);
    if (error instanceof GatewayError) return translate(error.detail) || t("暂时无法生成交易,请稍后重试.");
    return error instanceof Error ? translate(error.message) : t("生成失败,没有写入任何记录.");
}

export const badFees = (): TransferError => new TransferError("invalid", t("手续费数据异常,请稍后重试."));

export interface TransferDraft {
    wallet: Wallet;
    /** 这个钱包的全部账户,核心库预览和构建 PSBT 时要用. */
    accounts: Account[];
    account: Account;
    from: string;
    network: string;
    asset: string;
    entry: AssetEntry;
    to: string;
    amount: bigint;
    tier: FeeTier;
    /** 高级设置里手动填写的值. */
    gasLimit?: bigint;
    feeLimit?: bigint;
}

export type SetupDraft = Pick<TransferDraft, "wallet" | "accounts" | "account" | "from" | "network">;

export type ReplaceMode = "speedup" | "cancel" | "rebuild";

export type ReplaceDraft = Pick<TransferDraft, "wallet" | "accounts" | "account">;

export type RawDraft = SetupDraft;

/** 原始请求页的输入,键是表单字段的 `id`. */
export type RawValues = Readonly<Record<string, string>>;

/** 原始请求不选手续费档位,要快时用替换交易加速. */
export const RAW_TIER: FeeTier = "medium";

export function randomRequestId(): Uint8Array {
    return crypto.getRandomValues(new Uint8Array(16));
}

export function hexOf(bytes: Uint8Array): string {
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function header(wallet: Wallet, network: string): RequestHeader {
    return { id: randomRequestId(), fingerprint: wallet.fingerprint, network };
}

/** 预览只是提示,判断以冷钱包屏幕为准,所以这里不拦截.只有二维码装不下时才停下.预览解析不了时是 null. */
export function reviewRequest(request: Uint8Array, accounts: Account[]): Review | null {
    try {
        return previewRequest(request, accountKeyList(accounts));
    } catch (error) {
        if (error instanceof Error && error.message.includes("大小上限")) {
            throw new TransferError("too-large", t("交易数据超过二维码上限,请减少输入数量."));
        }
        return null;
    }
}

export function rawText(values: RawValues, id: string): string {
    return values[id]?.trim() ?? "";
}

/** 粘贴的数据.全是十六进制时按十六进制,其余按 base64. */
export function parsePayload(text: string): Uint8Array {
    const clean = text.replace(/\s/g, "").replace(/^0x/i, "");
    if (/^(?:[0-9a-f]{2})*$/i.test(clean)) {
        return Uint8Array.from(clean.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
    }
    try {
        return Uint8Array.from(atob(clean), (char) => char.charCodeAt(0));
    } catch {
        throw new TransferError("invalid", t("数据格式错误,要用十六进制或 base64."));
    }
}

export function wholeNumber(text: string, label: string): bigint {
    if (!/^\d+$/.test(text)) throw new TransferError("invalid", t("{0}要填写整数.", label));
    return BigInt(text);
}

export function checkedAddress(network: string, text: string, label: string): string {
    try {
        return validateAddress(network, text);
    } catch {
        throw new TransferError("invalid", t("{0}无效.", label));
    }
}

/** 粘贴的载荷不经过构建器,原样进请求,内容由冷钱包解析和判断.`to` 只用于显示,取自预览. */
export function pastedRequest(draft: RawDraft, values: RawValues) {
    const payload = parsePayload(rawText(values, "payload"));
    if (payload.length === 0) throw new TransferError("invalid", t("要填写待签名数据."));
    const head = header(draft.wallet, draft.network);
    const request = buildPayloadRequest({ ...head, path: draft.account.path, payload });
    const to = reviewRequest(request, draft.accounts)?.counterparty ?? "";
    return { head, payload, request, target: { ...draft, asset: "native", to } };
}

export function requestFields(
    draft: Pick<TransferDraft, "wallet" | "account" | "from" | "network" | "asset" | "to">,
    id: Uint8Array,
    request: Uint8Array,
    plan: TxPlan,
    amount: string,
    kind: TxKind,
    expiresAt: number | null,
    hardExpiry: boolean,
): NewTx {
    return {
        walletId: draft.wallet.id,
        accountId: draft.account.id,
        chain: draft.account.chain,
        network: draft.network,
        kind,
        asset: draft.asset,
        amount,
        from: draft.from,
        to: draft.to,
        request,
        requestId: hexOf(id),
        plan,
        expiresAt,
        hardExpiry,
    };
}

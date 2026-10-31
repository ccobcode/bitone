// 各链构建交易共用的部分.协议和编码都在核心库里,这里只负责取数据和生成记录.
import { accountKeyList, previewRequest, type RequestHeader, type Review } from "../core";
import { t } from "../core/i18n";
import type { AssetEntry } from "../core/manifest";
import type { Account, NewTx, TxKind, TxPlan, Wallet } from "../data/db";
import type { FeeTier } from "../gateway/client";

/** 网页端的 30 分钟只是手续费报价的有效期,见 docs/03-system.md 第 9 节. */
export const QUOTE_LIFETIME = 30 * 60_000;

export type TransferErrorCode =
    | "forbidden"
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
    readonly review?: Review;
    constructor(code: TransferErrorCode, message: string, review?: Review) {
        super(message);
        this.code = code;
        this.review = review;
    }
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

export function randomRequestId(): Uint8Array {
    return crypto.getRandomValues(new Uint8Array(16));
}

function hex(bytes: Uint8Array): string {
    return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function header(wallet: Wallet, network: string): RequestHeader {
    return { id: randomRequestId(), fingerprint: wallet.fingerprint, network };
}

export function reviewRequest(request: Uint8Array, accounts: Account[]): Review {
    let review: Review;
    try {
        review = previewRequest(request, accountKeyList(accounts));
    } catch (error) {
        if (error instanceof Error && error.message.includes("大小上限")) {
            throw new TransferError("too-large", t("交易数据超过二维码上限,请减少输入数量."));
        }
        throw new TransferError("invalid", t("交易无法解析,已拒绝."));
    }
    if (review.confirm === "refuse") {
        throw new TransferError("forbidden", review.alerts[0] ?? t("这笔交易不能签名."), review);
    }
    return review;
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
        requestId: hex(id),
        plan,
        expiresAt,
        hardExpiry,
    };
}

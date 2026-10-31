// Solana 的构建和替换.用 durable nonce,签名不过期,直到 nonce 推进.
import { t } from "../../core/i18n";
import { lookupAsset } from "../../core/manifest";
import { reservationsOf } from "../../data/claims";
import { planOf, type Account, type TxRecord } from "../../data/db";
import type { Built, FeeSource } from "../spec";
import {
    TransferError,
    badFees,
    header,
    requestFields,
    reviewRequest,
    type ReplaceDraft,
    type ReplaceMode,
    type SetupDraft,
    type TransferDraft,
} from "../transfer";
import { estimateSolana, fetchBlockhash, fetchNonceAccount } from "./api";
import { buildSolanaAdvanceNonce, buildSolanaCreateNonce, buildSolanaTransfer, solanaNonceAddress } from "./core";

export interface SolanaPlan {
    chain: "solana";
    /** 创建 nonce 账户这一笔只有 blockhash,没有 nonce 值. */
    nonceValue: string | null;
    nonceAccount: string;
    priority: number;
    computeUnits: number;
    createAta: boolean;
}

export type SolanaClaim = "solana-nonce";

/** blockhash 约 150 个 slot 后过期,这笔只有这么长的窗口. */
export const SOLANA_BLOCKHASH_WINDOW = 60_000;
/** nonce 账户 80 字节的免租金额,账户关闭后可以赎回. */
export const NONCE_RENT = 1_447_680n;
/** Solana 基础费是每个签名 5000 lamports. */
export const SOLANA_BASE_FEE = 5000n;

const SOLANA_NONCE_CLAIM = "account";

/** 单元数都留了余量,超出会失败,多出的部分不收费以外的代价. */
export function solanaComputeUnits(token: boolean, createAta: boolean): number {
    if (createAta) return 200_000;
    return token ? 50_000 : 10_000;
}

/** nonce 值是一次性的,上一笔没有结束时再构建,两笔只有一笔能上链. */
async function ensureNonceFree(account: Account, network: string): Promise<void> {
    const taken = (await reservationsOf(account.id, network)).some((row) => row.type === "solana-nonce");
    if (taken) throw new TransferError("nonce-busy", t("上一笔 Solana 交易还没有结束,完成后才能转账."));
}

export async function transfer(draft: TransferDraft, fees: FeeSource): Promise<Built> {
    const { account, network, from, to, amount, asset, entry } = draft;
    await ensureNonceFree(account, network);
    const nonceAccount = solanaNonceAddress(from);
    const token = asset !== "native";
    const [feeData, info, estimate] = await Promise.all([
        fees(network),
        fetchNonceAccount(network, nonceAccount),
        token ? estimateSolana(network, to, asset) : Promise.resolve({ recipientTokenAccount: true, rent: 0n }),
    ]);
    if (!info.exists) throw new TransferError("no-nonce-account", t("首次 Solana 转账前要先创建 nonce 账户."));
    if (feeData.kind !== "solana") throw badFees();
    const createAta = token && !estimate.recipientTokenAccount;
    const computeUnits = solanaComputeUnits(token, createAta);
    const head = header(draft.wallet, network);
    const priority = Number(feeData[draft.tier]);
    const request = buildSolanaTransfer({
        ...head,
        path: account.path,
        from,
        to,
        amount,
        ...(token ? { mint: asset } : {}),
        decimals: entry.decimals,
        nonceValue: info.nonce,
        priorityMicroLamports: priority,
        computeUnits,
        createRecipientAta: createAta,
    });
    reviewRequest(request, draft.accounts);
    const plan: SolanaPlan = {
        chain: "solana",
        nonceValue: info.nonce,
        nonceAccount,
        priority,
        computeUnits,
        createAta,
    };
    // 不过期,直到 nonce 推进.
    const fields = requestFields(draft, head.id, request, plan, amount.toString(), "transfer", null, false);
    return { fields, claims: [{ type: "solana-nonce", value: SOLANA_NONCE_CLAIM }] };
}

/** 创建 nonce 账户.这一笔只能用 blockhash,超时要重新构建. */
export async function setup(draft: SetupDraft): Promise<Built> {
    const { account, network, from } = draft;
    await ensureNonceFree(account, network);
    const nonceAccount = solanaNonceAddress(from);
    const info = await fetchNonceAccount(network, nonceAccount);
    if (info.exists) throw new TransferError("invalid", t("nonce 账户已存在."));
    const blockhash = await fetchBlockhash(network);
    const head = header(draft.wallet, network);
    const request = buildSolanaCreateNonce({ ...head, path: account.path, from, blockhash, lamports: NONCE_RENT });
    reviewRequest(request, draft.accounts);
    const plan: SolanaPlan = {
        chain: "solana",
        nonceValue: null,
        nonceAccount,
        priority: 0,
        computeUnits: 0,
        createAta: false,
    };
    const fields = requestFields(
        { ...draft, asset: "native", to: nonceAccount },
        head.id,
        request,
        plan,
        NONCE_RENT.toString(),
        "create-nonce",
        Date.now() + SOLANA_BLOCKHASH_WINDOW,
        true,
    );
    return { fields, claims: [{ type: "solana-nonce", value: SOLANA_NONCE_CLAIM }] };
}

/** Solana 没有手续费替换.取消只推进 nonce,用同一个 nonce 值签过名的交易随之作废. */
export async function replace(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft): Promise<Built> {
    const plan = planOf(tx, "solana");
    const { nonceValue } = plan;
    if (nonceValue === null || mode === "speedup") {
        throw new TransferError("unsupported", t("Solana 不提供加速."));
    }
    const place = { ...draft, from: tx.from, network: tx.network };
    const head = header(draft.wallet, tx.network);
    const path = draft.account.path;
    if (mode === "cancel" || tx.kind === "cancel") {
        const request = buildSolanaAdvanceNonce({ ...head, path, from: tx.from, nonceValue });
        reviewRequest(request, draft.accounts);
        const next: SolanaPlan = { ...plan, priority: 0, computeUnits: 0, createAta: false };
        const target = { ...place, asset: "native", to: tx.from };
        return { fields: requestFields(target, head.id, request, next, "0", "cancel", null, false), claims: [] };
    }
    const token = tx.asset !== "native";
    const decimals = lookupAsset(tx.network, tx.asset)?.decimals;
    if (decimals === undefined) throw new TransferError("invalid", t("代币不在内置清单."));
    const request = buildSolanaTransfer({
        ...head,
        path,
        from: tx.from,
        to: tx.to,
        amount: BigInt(tx.amount),
        ...(token ? { mint: tx.asset } : {}),
        decimals,
        nonceValue,
        priorityMicroLamports: plan.priority,
        computeUnits: plan.computeUnits,
        createRecipientAta: plan.createAta,
    });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        { ...place, asset: tx.asset, to: tx.to },
        head.id,
        request,
        plan,
        tx.amount,
        mode,
        null,
        false,
    );
    return { fields, claims: [] };
}

export async function spentFlags(network: string, txs: readonly TxRecord[]): Promise<Map<string, boolean>> {
    const flags = new Map<string, boolean>();
    for (const tx of txs) {
        const plan = planOf(tx, "solana");
        if (plan.nonceValue === null) continue;
        const account = await fetchNonceAccount(network, plan.nonceAccount);
        if (account.exists) flags.set(tx.id, account.nonce !== plan.nonceValue);
    }
    return flags;
}

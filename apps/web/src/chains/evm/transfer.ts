// EVM 的构建和替换.nonce 是序号,替换交易复用它.
import { t } from "../../core/i18n";
import { abandonedHolder, nextNonce, reservationsOf } from "../../data/claims";
import { db, planOf, type TxRecord } from "../../data/db";
import { GatewayError } from "../../gateway/client";
import type { Built, FeeSource } from "../spec";
import {
    QUOTE_LIFETIME,
    TransferError,
    badFees,
    header,
    requestFields,
    reviewRequest,
    type ReplaceDraft,
    type ReplaceMode,
    type TransferDraft,
} from "../transfer";
import { estimateEvmGas, fetchNonces, type EvmTierFee } from "./api";
import { buildEvmTransfer, evmTransferData } from "./core";

export interface EvmPlan {
    chain: "evm";
    nonce: number;
    gasLimit: number;
    maxFee: string;
    maxPriority: string;
}

export type EvmClaim = "nonce";

const EVM_TRANSFER_GAS = 21_000n;

/** 估算 Gas.普通转账就是 21000,合约调用留 20% 余量. */
export async function evmGasLimit(draft: Omit<TransferDraft, "tier">): Promise<bigint> {
    if (draft.gasLimit !== undefined) return draft.gasLimit;
    const native = draft.asset === "native";
    try {
        const gas = await estimateEvmGas(draft.network, {
            from: draft.from,
            to: native ? draft.to : draft.asset,
            value: native ? draft.amount.toString() : "0",
            ...(native ? {} : { data: evmTransferData(draft.to, draft.amount.toString()) }),
        });
        return gas === EVM_TRANSFER_GAS ? gas : (gas * 12n) / 10n;
    } catch (error) {
        if (error instanceof GatewayError && error.code === "invalid_request") {
            throw new TransferError("revert", error.detail || t("这笔交易预计会失败."));
        }
        throw error;
    }
}

export async function transfer(draft: TransferDraft, fees: FeeSource): Promise<Built> {
    const { account, network, from, to, amount, asset } = draft;
    const [feeData, nonces, gas] = await Promise.all([fees(network), fetchNonces(network, [from]), evmGasLimit(draft)]);
    if (feeData.kind !== "evm") throw badFees();
    const fee = feeData[draft.tier];
    const chain = nonces.get(from);
    if (!chain) throw new TransferError("invalid", t("链上数据不完整,请稍后重试."));
    const floor = Math.max(chain.pending, chain.latest);
    const held = abandonedHolder(await reservationsOf(account.id, network), await db.txs.toArray(), floor);
    if (held) throw new TransferError("nonce-held", t("交易 {0} 已取消但签名仍然有效,先发替换交易.", held.id));
    const nonce = await nextNonce(account.id, network, floor);
    const head = header(draft.wallet, network);
    const plan: EvmPlan = {
        chain: "evm",
        nonce,
        gasLimit: Number(gas),
        maxFee: fee.maxFee.toString(),
        maxPriority: fee.priority.toString(),
    };
    const request = buildEvmTransfer({
        ...head,
        path: account.path,
        to,
        amount: amount.toString(),
        ...(asset === "native" ? {} : { contract: asset }),
        nonce,
        gasLimit: plan.gasLimit,
        maxFee: plan.maxFee,
        maxPriority: plan.maxPriority,
    });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        draft,
        head.id,
        request,
        plan,
        amount.toString(),
        "transfer",
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return { fields, claims: [{ type: "nonce", value: String(nonce) }] };
}

/** 替换交易的单价至少比原来高 12.5%,节点要求高 10% 才接受. */
function bump(value: bigint): bigint {
    return (value * 9n) / 8n + 1n;
}

export function bumpedEvmFees(plan: { maxFee: string; maxPriority: string }, fresh: EvmTierFee): EvmTierFee {
    const raise = (old: bigint, next: bigint) => (next > bump(old) ? next : bump(old));
    return {
        maxFee: raise(BigInt(plan.maxFee), fresh.maxFee),
        priority: raise(BigInt(plan.maxPriority), fresh.priority),
    };
}

export async function replace(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft, fees: FeeSource): Promise<Built> {
    const plan = planOf(tx, "evm");
    const feeData = await fees(tx.network);
    if (feeData.kind !== "evm") throw badFees();
    const fee = bumpedEvmFees(plan, feeData.fast);
    const cancel = mode === "cancel";
    const to = cancel ? tx.from : tx.to;
    const amount = cancel ? "0" : tx.amount;
    const contract = cancel || tx.asset === "native" ? undefined : tx.asset;
    const gasLimit = cancel ? Number(EVM_TRANSFER_GAS) : plan.gasLimit;
    const head = header(draft.wallet, tx.network);
    const next: EvmPlan = {
        chain: "evm",
        nonce: plan.nonce,
        gasLimit,
        maxFee: fee.maxFee.toString(),
        maxPriority: fee.priority.toString(),
    };
    const request = buildEvmTransfer({
        ...head,
        path: draft.account.path,
        to,
        amount,
        ...(contract ? { contract } : {}),
        nonce: plan.nonce,
        gasLimit,
        maxFee: next.maxFee,
        maxPriority: next.maxPriority,
    });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        { ...draft, from: tx.from, network: tx.network, asset: cancel ? "native" : tx.asset, to },
        head.id,
        request,
        next,
        amount,
        mode,
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return { fields, claims: [] };
}

export async function spentFlags(network: string, txs: readonly TxRecord[]): Promise<Map<string, boolean>> {
    const flags = new Map<string, boolean>();
    const nonces = await fetchNonces(network, [...new Set(txs.map((tx) => tx.from))]);
    for (const tx of txs) {
        const chain = nonces.get(tx.from);
        if (chain) flags.set(tx.id, chain.latest > planOf(tx, "evm").nonce);
    }
    return flags;
}

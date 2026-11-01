// EVM 的构建和替换.nonce 是序号,替换交易复用它.
import { t } from "../../core/i18n";
import { cleanText } from "../../core/text";
import { abandonedHolder, nextNonce, reservationsOf } from "../../data/claims";
import { db, planOf, type TxRecord } from "../../data/db";
import { GatewayError } from "../../gateway/client";
import type { Built, FeeSource, RawMode } from "../spec";
import {
    QUOTE_LIFETIME,
    RAW_TIER,
    TransferError,
    badFees,
    checkedAddress,
    header,
    hexOf,
    parsePayload,
    rawText,
    requestFields,
    reviewRequest,
    wholeNumber,
    type RawDraft,
    type RawValues,
    type ReplaceDraft,
    type ReplaceMode,
    type TransferDraft,
} from "../transfer";
import { estimateEvmGas, fetchNonces, type EvmTierFee } from "./api";
import { buildEvmCall, buildEvmTransfer, evmApproveData, evmTransferData } from "./core";

/** 原始请求的调用.`to` 是 null 表示部署合约,`data` 是不带 `0x` 的十六进制. */
export interface EvmCall {
    to: string | null;
    value: string;
    data: string;
}

export interface EvmPlan {
    chain: "evm";
    nonce: number;
    gasLimit: number;
    maxFee: string;
    maxPriority: string;
    /** 替换交易要原样带上. */
    call?: EvmCall;
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

/** 估算失败时用的 Gas 上限.页面会提示,用户可以在高级设置里改. */
export function evmDefaultGas(asset: string): bigint {
    return asset === "native" ? EVM_TRANSFER_GAS : 65_000n;
}

/** 在账户锁里分配 nonce.已放弃但签名仍然有效的交易挡路时不构建. */
async function claimNonce({ account, network, from }: RawDraft): Promise<number> {
    const chain = (await fetchNonces(network, [from])).get(from);
    if (!chain) throw new TransferError("invalid", t("链上数据不完整,请稍后重试."));
    const floor = Math.max(chain.pending, chain.latest);
    const held = abandonedHolder(await reservationsOf(account.id, network), await db.txs.toArray(), floor);
    if (held) throw new TransferError("nonce-held", t("交易 {0} 已取消但签名仍然有效,先发替换交易.", held.id));
    return nextNonce(account.id, network, floor);
}

export async function transfer(draft: TransferDraft, fees: FeeSource): Promise<Built> {
    const { network, to, amount, asset, account } = draft;
    const [feeData, nonce, gas] = await Promise.all([
        fees(network),
        claimNonce(draft),
        evmGasLimit(draft).catch(() => evmDefaultGas(asset)),
    ]);
    if (feeData.kind !== "evm") throw badFees();
    const fee = feeData[draft.tier];
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

export function rawModes(): RawMode[] {
    const gas = { id: "gas", label: t("Gas 上限"), hint: t("留空时自动估算") };
    const contract = { id: "contract", label: t("代币合约") };
    return [
        {
            id: "call",
            label: t("调用数据"),
            fields: [
                { id: "to", label: t("合约或收款地址"), hint: t("留空表示部署合约") },
                { id: "value", label: t("附带金额(wei)"), hint: t("留空表示 0") },
                { id: "data", label: t("调用数据"), hint: t("十六进制或 base64"), multiline: true },
                gas,
            ],
        },
        {
            id: "token",
            label: t("代币转账"),
            fields: [contract, { id: "to", label: t("收款地址") }, { id: "amount", label: t("数量(最小单位)") }, gas],
        },
        {
            id: "approve",
            label: t("授权或撤销"),
            fields: [
                contract,
                { id: "spender", label: t("被授权方") },
                { id: "amount", label: t("额度(最小单位)"), hint: t("0 表示撤销授权") },
                gas,
            ],
        },
    ];
}

/** 表单内容换成一次调用.`asset`,`amount`,`shown` 只用于显示. */
function rawCall(network: string, mode: string, values: RawValues) {
    if (mode === "call") {
        const typed = rawText(values, "to");
        const to = typed ? checkedAddress(network, typed, t("合约或收款地址")) : null;
        const value = wholeNumber(rawText(values, "value") || "0", t("附带金额")).toString();
        const data = hexOf(parsePayload(rawText(values, "data")));
        if (to === null && !data) throw new TransferError("invalid", t("部署合约要填写调用数据."));
        return { call: { to, value, data }, asset: "native", amount: value, shown: to ?? "" };
    }
    const contract = checkedAddress(network, rawText(values, "contract"), t("代币合约"));
    const amount = wholeNumber(rawText(values, "amount"), t("数量")).toString();
    if (mode === "token") {
        const to = checkedAddress(network, rawText(values, "to"), t("收款地址"));
        const call = { to: contract, value: "0", data: evmTransferData(to, amount).slice(2) };
        return { call, asset: contract, amount, shown: to };
    }
    const spender = checkedAddress(network, rawText(values, "spender"), t("被授权方"));
    const call = { to: contract, value: "0", data: evmApproveData(spender, amount).slice(2) };
    return { call, asset: "native", amount: "0", shown: contract };
}

async function rawGasLimit({ network, from }: RawDraft, call: EvmCall, typed: string): Promise<bigint> {
    if (typed) return wholeNumber(typed, t("Gas 上限"));
    if (call.to === null) throw new TransferError("invalid", t("部署合约要填写 Gas 上限."));
    try {
        const body = { from, to: call.to, value: call.value, ...(call.data ? { data: `0x${call.data}` } : {}) };
        const gas = await estimateEvmGas(network, body);
        return gas === EVM_TRANSFER_GAS ? gas : (gas * 12n) / 10n;
    } catch (error) {
        const reason = error instanceof GatewayError ? error.detail || error.code : t("无法估算.");
        throw new TransferError("invalid", t("估算失败:{0},请填写 Gas 上限.", cleanText(reason, 160)));
    }
}

export async function raw(draft: RawDraft, mode: string, values: RawValues, fees: FeeSource): Promise<Built> {
    const { network, account } = draft;
    const { call, asset, amount, shown } = rawCall(network, mode, values);
    const gas = await rawGasLimit(draft, call, rawText(values, "gas"));
    const [feeData, nonce] = await Promise.all([fees(network), claimNonce(draft)]);
    if (feeData.kind !== "evm") throw badFees();
    const fee = feeData[RAW_TIER];
    const head = header(draft.wallet, network);
    const plan: EvmPlan = {
        chain: "evm",
        nonce,
        gasLimit: Number(gas),
        maxFee: fee.maxFee.toString(),
        maxPriority: fee.priority.toString(),
        call,
    };
    const request = buildEvmCall({ ...head, path: account.path, ...callArgs(call), ...feeArgs(plan) });
    reviewRequest(request, draft.accounts);
    const fields = requestFields(
        { ...draft, asset, to: shown },
        head.id,
        request,
        plan,
        amount,
        "raw",
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return { fields, claims: [{ type: "nonce", value: String(nonce) }] };
}

function callArgs({ to, value, data }: EvmCall) {
    return { ...(to === null ? {} : { to }), value, data };
}

function feeArgs({ nonce, gasLimit, maxFee, maxPriority }: EvmPlan) {
    return { nonce, gasLimit, maxFee, maxPriority };
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
    const call = cancel ? undefined : plan.call;
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
        ...(call ? { call } : {}),
    };
    const path = draft.account.path;
    const request = call
        ? buildEvmCall({ ...head, path, ...callArgs(call), ...feeArgs(next) })
        : buildEvmTransfer({ ...head, path, to, amount, ...(contract ? { contract } : {}), ...feeArgs(next) });
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

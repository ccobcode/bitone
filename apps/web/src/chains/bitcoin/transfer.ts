// Bitcoin 的构建和替换.输入和找零都是序号,靠占用保证最多一笔上链.
import { accountKeyList } from "../../core";
import { t } from "../../core/i18n";
import { storeAddress } from "../../data/addresses";
import { reservationsOf, type Claim } from "../../data/claims";
import { db, planOf, type Account, type TxRecord, type Wallet } from "../../data/db";
import type { FeeTier } from "../../gateway/client";
import type { Fees } from "../index";
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
import { fetchRawTxs, fetchUtxos } from "./api";
import {
    bitcoinFeeInfo,
    bitcoinReplacement,
    buildBitcoinTransfer,
    deriveAddress,
    selectCoins,
    type BitcoinReplacement,
    type CoinInput,
    type CoinSelection,
} from "./core";

export interface PlanInput {
    txid: string;
    vout: number;
    amount: number;
    path: string;
    address: string;
}

export interface BitcoinPlan {
    chain: "bitcoin";
    inputs: PlanInput[];
    changePath: string | null;
    changeIndex: number | null;
    change: number;
    fee: number;
    feeRate: number;
    /** 虚拟大小,替换交易按它计算手续费. */
    vsize: number;
}

export type BitcoinClaim = "utxo" | "change";

export interface SpendableCoin extends CoinInput {
    address: string;
}

function coinPath(account: Account, change: 0 | 1, index: number): string {
    return `${account.path}/${change}/${index}`;
}

/** 可以花的 UTXO:已上链,属于这个账户,没有被待处理的交易占用. */
export async function spendableCoins(account: Account, network: string): Promise<SpendableCoin[]> {
    const rows = await db.addresses.where("accountId").equals(account.id).toArray();
    const byAddress = new Map(rows.map((row) => [row.address, row]));
    const utxos = await fetchUtxos(network, [...byAddress.keys()]);
    const reservations = await reservationsOf(account.id, network);
    const taken = new Set(reservations.filter((row) => row.type === "utxo").map((row) => row.value));
    const coins = new Map<string, SpendableCoin>();
    for (const utxo of utxos) {
        const key = `${utxo.txid}:${utxo.vout}`;
        const row = byAddress.get(utxo.address);
        if (!row || utxo.height === 0 || taken.has(key) || utxo.amount > BigInt(Number.MAX_SAFE_INTEGER)) continue;
        coins.set(key, {
            txid: utxo.txid,
            vout: utxo.vout,
            amount: Number(utxo.amount),
            path: coinPath(account, row.change, row.index),
            address: row.address,
        });
    }
    return [...coins.values()];
}

function isTaprootPath(path: string): boolean {
    return path.startsWith("m/86'");
}

/** 除全 Taproot 外都要带前序交易,混合输入里的 Taproot 也一样. */
export function needsPrevTx(inputs: readonly { path: string }[]): boolean {
    return inputs.some((input) => !isTaprootPath(input.path));
}

function bytesFromHex(text: string): Uint8Array {
    const bytes = new Uint8Array(text.length / 2);
    for (let i = 0; i < bytes.length; i++) bytes[i] = Number.parseInt(text.slice(i * 2, i * 2 + 2), 16);
    return bytes;
}

/** 取回前序交易.txid 由核心库构建时重算,对不上会拒绝. */
async function withPrevTx<T extends { txid: string; path: string }>(
    network: string,
    inputs: readonly T[],
): Promise<(T & { prevTx?: Uint8Array })[]> {
    if (!needsPrevTx(inputs)) return [...inputs];
    const raw = await fetchRawTxs(network, [...new Set(inputs.map((input) => input.txid))]);
    return inputs.map((input) => {
        const text = raw.get(input.txid);
        if (!text) throw new TransferError("prev-tx", t("链上数据不完整,请稍后重试."));
        return { ...input, prevTx: bytesFromHex(text) };
    });
}

/** 核心库的错误原样给用户看,比如金额低于粉尘线.参数错误只可能来自网关给的前序交易. */
function buildPsbt(args: Parameters<typeof buildBitcoinTransfer>[0]): Uint8Array {
    try {
        return buildBitcoinTransfer(args);
    } catch (error) {
        const message = error instanceof Error ? error.message : "";
        if (!message || message === "交易参数无效") {
            throw new TransferError("prev-tx", t("前序交易与输入不符,已拒绝构建."));
        }
        throw new TransferError("invalid", `${message}.`);
    }
}

async function nextChangeIndex(account: Account, network: string, mode: Wallet["mode"]): Promise<number> {
    const rows = await db.addresses.where("accountId").equals(account.id).toArray();
    const used = new Set(rows.filter((row) => row.change === 1 && row.used).map((row) => row.index));
    const reservations = await reservationsOf(account.id, network);
    for (const row of reservations) if (row.type === "change") used.add(Number(row.value));
    let index = 0;
    while (used.has(index)) index += 1;
    await storeAddress(deriveAddress, account, mode, index, 1);
    return index;
}

function tierRate(fees: Fees, tier: FeeTier): bigint {
    if (fees.kind !== "bitcoin") throw badFees();
    return fees[tier];
}

function pickCoins(coins: SpendableCoin[], amount: bigint, rate: bigint, account: Account): CoinSelection {
    try {
        return selectCoins(coins, amount, rate, account.kind);
    } catch {
        throw new TransferError("insufficient", t("可用余额不足,部分余额被待处理的交易占用."));
    }
}

export async function transfer(draft: TransferDraft, fees: FeeSource): Promise<Built> {
    const { account, network, to, amount, wallet } = draft;
    const rate = tierRate(await fees(network), draft.tier);
    const coins = await spendableCoins(account, network);
    const selection = pickCoins(coins, amount, rate, account);
    const changeIndex = selection.change > 0 ? await nextChangeIndex(account, network, wallet.mode) : null;
    const changePath = changeIndex === null ? null : coinPath(account, 1, changeIndex);
    const addresses = new Map(coins.map((coin) => [`${coin.txid}:${coin.vout}`, coin.address]));
    const inputs: PlanInput[] = selection.inputs.map((input) => ({
        ...input,
        address: addresses.get(`${input.txid}:${input.vout}`) ?? "",
    }));
    const head = header(wallet, network);
    const request = buildPsbt({
        ...head,
        keys: accountKeyList(draft.accounts),
        inputs: await withPrevTx(network, selection.inputs),
        to,
        amount: Number(amount),
        ...(changePath ? { changePath } : {}),
        change: selection.change,
    });
    reviewRequest(request, draft.accounts);
    const plan: BitcoinPlan = {
        chain: "bitcoin",
        inputs,
        changePath,
        changeIndex,
        change: selection.change,
        feeRate: Number(rate),
        ...bitcoinFeeInfo(request),
    };
    const claims: Claim[] = selection.inputs.map((input) => ({
        type: "utxo",
        value: `${input.txid}:${input.vout}`,
    }));
    if (changeIndex !== null) claims.push({ type: "change", value: String(changeIndex) });
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
    return { fields, claims };
}

function replacementFees(plan: BitcoinPlan, amount: bigint, fast: bigint, account: Account): BitcoinReplacement {
    const total = plan.inputs.reduce((sum, input) => sum + BigInt(input.amount), 0n);
    try {
        return bitcoinReplacement(total, amount, BigInt(plan.fee), BigInt(plan.vsize), fast, account.kind);
    } catch {
        throw new TransferError("insufficient", t("输入金额不够支付更高的手续费."));
    }
}

export async function replace(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft, fees: FeeSource): Promise<Built> {
    const plan = planOf(tx, "bitcoin");
    const { account, wallet } = draft;
    const fast = tierRate(await fees(tx.network), "fast");
    // 取消交易再加速还是取消.
    const cancel = mode === "cancel" || tx.kind === "cancel";
    const amount = cancel ? 0n : BigInt(tx.amount);
    const { change } = replacementFees(plan, amount, fast, account);

    let changePath = change > 0 ? plan.changePath : null;
    let changeIndex = changePath ? plan.changeIndex : null;
    const fresh: Claim[] = [];
    if (change > 0 && changePath === null) {
        // 原交易没有找零,取消时要给资金分配一个回来的地址.
        changeIndex = await nextChangeIndex(account, tx.network, wallet.mode);
        changePath = coinPath(account, 1, changeIndex);
        fresh.push({ type: "change", value: String(changeIndex) });
    }
    const home = changeIndex === null ? null : await storeAddress(deriveAddress, account, wallet.mode, changeIndex, 1);
    const to = cancel && home ? home.address : tx.to;

    const inputs = plan.inputs.map(({ txid, vout, amount: value, path }) => ({ txid, vout, amount: value, path }));
    const head = header(wallet, tx.network);
    const request = buildPsbt({
        ...head,
        keys: accountKeyList(draft.accounts),
        inputs: await withPrevTx(tx.network, inputs),
        ...(cancel ? {} : { to, amount: Number(amount) }),
        ...(changePath ? { changePath } : {}),
        change,
    });
    reviewRequest(request, draft.accounts);
    const { fee, vsize } = bitcoinFeeInfo(request);
    const next: BitcoinPlan = { ...plan, changePath, changeIndex, change, fee, feeRate: Math.ceil(fee / vsize), vsize };
    const fields = requestFields(
        { ...draft, from: tx.from, network: tx.network, asset: tx.asset, to },
        head.id,
        request,
        next,
        amount.toString(),
        mode,
        Date.now() + QUOTE_LIFETIME,
        false,
    );
    return { fields, claims: fresh };
}

export async function spentFlags(network: string, txs: readonly TxRecord[]): Promise<Map<string, boolean>> {
    const flags = new Map<string, boolean>();
    const inputs = txs.flatMap((tx) => planOf(tx, "bitcoin").inputs);
    const utxos = await fetchUtxos(network, [...new Set(inputs.map((input) => input.address))]);
    const unspent = new Set(utxos.map((utxo) => `${utxo.txid}:${utxo.vout}`));
    for (const tx of txs) {
        flags.set(
            tx.id,
            planOf(tx, "bitcoin").inputs.some((input) => !unspent.has(`${input.txid}:${input.vout}`)),
        );
    }
    return flags;
}

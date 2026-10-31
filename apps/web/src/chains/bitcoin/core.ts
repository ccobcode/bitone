// Bitcoin 的核心库绑定.
import {
    primaryNetwork,
    type AccountKind,
    type KeyEntry,
    type Mode,
    type PairingAccount,
    type RequestHeader,
} from "../../core";
import {
    bitcoinAddress,
    bitcoinFeeInfo as wasmBitcoinFeeInfo,
    bitcoinReplacement as wasmBitcoinReplacement,
    buildBitcoinTransfer as wasmBuildBitcoin,
    selectBitcoinCoins as wasmSelectCoins,
} from "../../core/pkg/bitone_wasm";

export function deriveAddress(
    account: Pick<PairingAccount, "kind" | "key">,
    mode: Mode,
    change: 0 | 1,
    index: number,
): string {
    return bitcoinAddress(account.key, account.kind, change === 1, index, primaryNetwork("bitcoin", mode));
}

export interface CoinInput {
    txid: string;
    vout: number;
    amount: number;
    path: string;
    prevTx?: Uint8Array;
}

export interface BitcoinTransfer extends RequestHeader {
    keys: KeyEntry[];
    inputs: CoinInput[];
    /** 留空表示取消,全部回到 `changePath`,冷钱包显示转给自己. */
    to?: string;
    amount?: number;
    changePath?: string;
    change: number;
}

export const buildBitcoinTransfer = (args: BitcoinTransfer): Uint8Array => wasmBuildBitcoin(args);

export interface CoinSelection {
    inputs: Omit<CoinInput, "prevTx">[];
    fee: number;
    change: number;
}

/** 选币.金额不够时抛错.选出后要取回前序交易才能构建. */
export function selectCoins(utxos: CoinInput[], amount: bigint, feeRate: bigint, kind: AccountKind): CoinSelection {
    return wasmSelectCoins(utxos, amount, feeRate, kind) as CoinSelection;
}

export interface BitcoinFeeInfo {
    fee: number;
    vsize: number;
}

/** PSBT 的手续费和估算的虚拟大小. */
export function bitcoinFeeInfo(request: Uint8Array): BitcoinFeeInfo {
    return wasmBitcoinFeeInfo(request) as BitcoinFeeInfo;
}

export interface BitcoinReplacement {
    fee: number;
    change: number;
}

/** RBF 用同一组输入提高手续费.`amount` 为 0 表示取消.输入不够付时抛错. */
export function bitcoinReplacement(
    total: bigint,
    amount: bigint,
    oldFee: bigint,
    vsize: bigint,
    feeRate: bigint,
    changeKind: AccountKind,
): BitcoinReplacement {
    return wasmBitcoinReplacement(total, amount, oldFee, vsize, feeRate, changeKind) as BitcoinReplacement;
}

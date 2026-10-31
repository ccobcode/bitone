// TRON 的核心库绑定.
import type { RequestHeader } from "../../core";
import { buildTronTransfer as wasmBuildTron } from "../../core/pkg/bitone_wasm";

export interface TronTransfer extends RequestHeader {
    path: string;
    from: string;
    to: string;
    amount: string;
    contract?: string;
    refBlockBytes: string;
    refBlockHash: string;
    expirationMs: number;
    timestampMs: number;
    feeLimit: number;
}

export const buildTronTransfer = (args: TronTransfer): Uint8Array => wasmBuildTron(args);

import type { RequestHeader } from "../../core";
import { buildTronTransfer as wasmBuildTron, tronPayloadInfo as wasmPayloadInfo } from "../../core/pkg/bitone_wasm";

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

export interface TronPayloadInfo {
    expirationMs: number;
    feeLimit: number;
}

/** 粘贴的 `raw_data` 的过期时间和 fee_limit.核心库解析不了时是 undefined. */
export const tronPayloadInfo = (payload: Uint8Array): TronPayloadInfo | undefined => wasmPayloadInfo(payload);

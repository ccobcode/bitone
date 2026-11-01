import type { RequestHeader } from "../../core";
import {
    buildEvmCall as wasmBuildEvmCall,
    buildEvmTransfer as wasmBuildEvm,
    evmApproveData,
    evmTransferData,
} from "../../core/pkg/bitone_wasm";

export { evmApproveData, evmTransferData };

export interface EvmTransfer extends RequestHeader {
    path: string;
    to: string;
    amount: string;
    contract?: string;
    nonce: number;
    gasLimit: number;
    maxFee: string;
    maxPriority: string;
}

export const buildEvmTransfer = (args: EvmTransfer): Uint8Array => wasmBuildEvm(args);

export interface EvmCallRequest extends RequestHeader {
    path: string;
    /** 留空是部署合约. */
    to?: string;
    value: string;
    /** 十六进制,不带 `0x`. */
    data: string;
    nonce: number;
    gasLimit: number;
    maxFee: string;
    maxPriority: string;
}

export const buildEvmCall = (args: EvmCallRequest): Uint8Array => wasmBuildEvmCall(args);

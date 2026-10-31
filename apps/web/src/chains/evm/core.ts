// EVM 的核心库绑定.
import type { RequestHeader } from "../../core";
import { buildEvmTransfer as wasmBuildEvm, evmTransferData } from "../../core/pkg/bitone_wasm";

export { evmTransferData };

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

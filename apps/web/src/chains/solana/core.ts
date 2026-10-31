// Solana 的核心库绑定.
import type { RequestHeader } from "../../core";
import {
    buildSolanaAdvanceNonce as wasmBuildSolanaAdvance,
    buildSolanaCreateNonce as wasmBuildSolanaNonce,
    buildSolanaTransfer as wasmBuildSolana,
    solanaNonceAddress,
} from "../../core/pkg/bitone_wasm";

export { solanaNonceAddress };

export interface SolanaTransfer extends RequestHeader {
    path: string;
    from: string;
    to: string;
    amount: bigint;
    mint?: string;
    decimals: number;
    nonceValue: string;
    priorityMicroLamports: number;
    computeUnits: number;
    createRecipientAta: boolean;
}

export interface SolanaCreateNonce extends RequestHeader {
    path: string;
    from: string;
    blockhash: string;
    lamports: bigint;
}

export interface SolanaAdvanceNonce extends RequestHeader {
    path: string;
    from: string;
    nonceValue: string;
}

export const buildSolanaTransfer = (args: SolanaTransfer): Uint8Array => wasmBuildSolana(args);
export const buildSolanaCreateNonce = (args: SolanaCreateNonce): Uint8Array => wasmBuildSolanaNonce(args);
export const buildSolanaAdvanceNonce = (args: SolanaAdvanceNonce): Uint8Array => wasmBuildSolanaAdvance(args);

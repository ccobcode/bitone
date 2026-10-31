// Solana 的网关接口.
import { isRecord } from "../../core/text";
import { get, malformed, post, requireAmount, requireText, requireTiers, type FeeTier } from "../../gateway/client";

export interface NonceAccount {
    exists: boolean;
    nonce: string;
}

export async function fetchNonceAccount(network: string, address: string): Promise<NonceAccount> {
    const body = await post(network, "nonce-account", { address });
    if (!isRecord(body) || typeof body.exists !== "boolean") throw malformed();
    return body.exists ? { exists: true, nonce: requireText(body.nonce, 64) } : { exists: false, nonce: "" };
}

export async function fetchBlockhash(network: string): Promise<string> {
    const body = await get(network, "blockhash");
    return requireText(isRecord(body) ? body.blockhash : null, 64);
}

export type SolanaFees = { kind: "solana" } & Record<FeeTier, bigint>;

export function parseFees(body: Record<string, unknown>): SolanaFees {
    return { kind: "solana", ...requireTiers(body) };
}

export interface SolanaEstimate {
    recipientTokenAccount: boolean;
    rent: bigint;
}

export async function estimateSolana(network: string, recipient: string, mint?: string): Promise<SolanaEstimate> {
    const body = await post(network, "estimate", { recipient, mint });
    if (!isRecord(body) || typeof body.recipientTokenAccount !== "boolean") throw malformed();
    return { recipientTokenAccount: body.recipientTokenAccount, rent: requireAmount(body.rent) };
}

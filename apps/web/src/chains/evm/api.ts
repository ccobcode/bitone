import { addressKey, isRecord } from "../../core/text";
import { malformed, post, requireAmount, requireCount, type FeeTier } from "../../gateway/client";

export interface NonceInfo {
    latest: number;
    pending: number;
}

export async function fetchNonces(network: string, addresses: readonly string[]): Promise<Map<string, NonceInfo>> {
    const body = await post(network, "nonce", { addresses });
    const map = isRecord(body) ? body.nonces : null;
    if (!isRecord(map)) throw malformed();
    const lower = new Map(Object.entries(map).map(([address, value]) => [addressKey(address), value]));
    const result = new Map<string, NonceInfo>();
    for (const address of addresses) {
        const row = lower.get(addressKey(address));
        if (!isRecord(row)) throw malformed();
        result.set(address, { latest: requireCount(row.latest), pending: requireCount(row.pending) });
    }
    return result;
}

export interface EvmTierFee {
    maxFee: bigint;
    priority: bigint;
}

export type EvmFees = { kind: "evm"; baseFee: bigint } & Record<FeeTier, EvmTierFee>;

function tier(value: unknown): EvmTierFee {
    if (!isRecord(value)) throw malformed();
    return { maxFee: requireAmount(value.maxFee), priority: requireAmount(value.priority) };
}

export function parseFees(body: Record<string, unknown>): EvmFees {
    return {
        kind: "evm",
        baseFee: requireAmount(body.baseFee),
        slow: tier(body.slow),
        medium: tier(body.medium),
        fast: tier(body.fast),
    };
}

/** 合约回滚时网关返回 400 `invalid_request` 加节点原文. */
export async function estimateEvmGas(
    network: string,
    call: { from: string; to: string; value: string; data?: string },
): Promise<bigint> {
    const body = await post(network, "estimate", call);
    return requireAmount(isRecord(body) ? body.gas : null);
}

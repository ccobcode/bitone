// TRON 的网关接口,格式见 docs/09-gateway-api.md.
import { isRecord } from "../../core/text";
import { get, malformed, post, requireAmount, requireCount, requireHex } from "../../gateway/client";

export interface TronResources {
    bandwidth: bigint;
    energy: bigint;
    activated: boolean;
}

export async function fetchResources(network: string, address: string): Promise<TronResources> {
    const body = await post(network, "resources", { addresses: [address] });
    const map = isRecord(body) ? body.resources : null;
    const row = isRecord(map) ? map[address] : null;
    if (!isRecord(row)) throw malformed();
    return {
        bandwidth: BigInt(requireCount(row.bandwidth)),
        energy: BigInt(requireCount(row.energy)),
        activated: row.activated === true,
    };
}

export interface RefBlock {
    refBlockBytes: string;
    refBlockHash: string;
    timestamp: number;
}

export async function fetchRefBlock(network: string): Promise<RefBlock> {
    const body = await get(network, "ref-block");
    if (!isRecord(body)) throw malformed();
    return {
        refBlockBytes: requireHex(body.refBlockBytes, 2),
        refBlockHash: requireHex(body.refBlockHash, 8),
        timestamp: requireCount(body.timestamp),
    };
}

export interface TronFees {
    kind: "tron";
    energyPrice: bigint;
    bandwidthPrice: bigint;
}

export function parseFees(body: Record<string, unknown>): TronFees {
    return {
        kind: "tron",
        energyPrice: BigInt(requireCount(body.energyPrice)),
        bandwidthPrice: BigInt(requireCount(body.bandwidthPrice)),
    };
}

export interface TronEstimate {
    energy: bigint;
    bandwidth: bigint;
    activationFee: bigint;
}

export async function estimateTron(
    network: string,
    call: { from: string; to: string; contract?: string; data?: string },
): Promise<TronEstimate> {
    const body = await post(network, "estimate", call);
    if (!isRecord(body)) throw malformed();
    return {
        energy: requireAmount(body.energy),
        bandwidth: requireAmount(body.bandwidth),
        activationFee: requireAmount(body.activationFee),
    };
}

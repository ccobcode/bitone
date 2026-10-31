// Bitcoin 的网关接口,格式见 docs/09-gateway-api.md.
import { isRecord } from "../../core/text";
import {
    MAX_ADDRESSES,
    chunks,
    malformed,
    post,
    requireAmount,
    requireCount,
    requireHex,
    requireTiers,
    type FeeTier,
} from "../../gateway/client";

export interface Utxo {
    address: string;
    txid: string;
    vout: number;
    amount: bigint;
    /** 0 表示还在内存池. */
    height: number;
}

/** 只留下请求过的地址.网关给的 UTXO 不可信,金额由前序交易的 txid 校验. */
export async function fetchUtxos(network: string, addresses: readonly string[]): Promise<Utxo[]> {
    const pages = await Promise.all(
        chunks(addresses, MAX_ADDRESSES).map(async (chunk) => {
            const body = await post(network, "utxos", { addresses: chunk });
            const list = isRecord(body) ? body.utxos : null;
            if (!Array.isArray(list)) throw malformed();
            const known = new Set(chunk);
            return list.flatMap((item: unknown): Utxo[] => {
                if (!isRecord(item) || typeof item.address !== "string" || !known.has(item.address)) return [];
                return [
                    {
                        address: item.address,
                        // 占用按字符串比较,大小写不同不能算成另一个 UTXO.
                        txid: requireHex(item.txid, 32).toLowerCase(),
                        vout: requireCount(item.vout),
                        amount: requireAmount(item.amount),
                        height: requireCount(item.height),
                    },
                ];
            });
        }),
    );
    return pages.flat();
}

/** 只返回请求过的 txid.内容对不对由核心库重算 txid 判断. */
export async function fetchRawTxs(network: string, txids: readonly string[]): Promise<Map<string, string>> {
    const result = new Map<string, string>();
    for (const chunk of chunks(txids, MAX_ADDRESSES)) {
        const body = await post(network, "raw-txs", { txids: chunk });
        const map = isRecord(body) ? body.txs : null;
        if (!isRecord(map)) throw malformed();
        for (const txid of chunk) {
            const hex = map[txid];
            if (typeof hex === "string" && hex.length % 2 === 0 && /^[0-9a-f]*$/i.test(hex)) result.set(txid, hex);
        }
    }
    return result;
}

export type BitcoinFees = { kind: "bitcoin" } & Record<FeeTier, bigint>;

export function parseFees(body: Record<string, unknown>): BitcoinFees {
    return { kind: "bitcoin", ...requireTiers(body) };
}

// 链的注册表.这是网页端唯一按链分发的地方,共享层只通过这里拿到一条链的能力.
// 加一条链:在 `chains/` 下加一个目录,再在这个文件里接上它.
import type { Chain } from "../core";
import { fetchFeeBody, malformed } from "../gateway/client";
import { bitcoin } from "./bitcoin";
import type { BitcoinFees } from "./bitcoin/api";
import type { BitcoinClaim, BitcoinPlan } from "./bitcoin/transfer";
import { evm } from "./evm";
import type { EvmFees } from "./evm/api";
import type { EvmClaim, EvmPlan } from "./evm/transfer";
import { solana } from "./solana";
import type { SolanaFees } from "./solana/api";
import type { SolanaClaim, SolanaPlan } from "./solana/transfer";
import type { ChainSpec, NetworkFamily } from "./spec";
import { tron } from "./tron";
import type { TronFees } from "./tron/api";
import type { TronPlan } from "./tron/transfer";

export type TxPlan = BitcoinPlan | EvmPlan | TronPlan | SolanaPlan;
export type Fees = BitcoinFees | EvmFees | TronFees | SolanaFees;
export type ReservationType = BitcoinClaim | EvmClaim | SolanaClaim;

const CHAINS: Record<Chain, ChainSpec> = { bitcoin, evm, tron, solana };

/** 界面上按这个顺序列出. */
export const CHAIN_LIST: readonly ChainSpec[] = Object.values(CHAINS);

/** 新建联系人时默认选中的链. */
export const DEFAULT_CONTACT_CHAIN: Chain = "evm";

/** 不认识的网络用这个图标. */
const FALLBACK_FAMILY = "ethereum";

export function chainOf(chain: Chain): ChainSpec {
    return CHAINS[chain];
}

export function isChain(value: unknown): value is Chain {
    return typeof value === "string" && Object.hasOwn(CHAINS, value);
}

export const NETWORK_FAMILIES: readonly NetworkFamily[] = CHAIN_LIST.flatMap((spec) => spec.families);

/** 网络标识对应的图标.测试网和主网用同一个. */
export function networkFamily(network: string): string {
    return NETWORK_FAMILIES.find((family) => family.matches(network))?.id ?? FALLBACK_FAMILY;
}

const EXPLORERS: Record<string, (hash: string) => string> = Object.assign(
    {},
    ...CHAIN_LIST.map((spec) => spec.explorers),
);

/** 区块浏览器链接内置在网页端,不从网关获取,见 docs/04-security.md 第 5 节. */
export function explorerLink(network: string, hash: string): string | null {
    return EXPLORERS[network]?.(encodeURIComponent(hash)) ?? null;
}

export function deriveAddress(...args: Parameters<ChainSpec["deriveAddress"]>): string {
    return chainOf(args[0].chain).deriveAddress(...args);
}

/** 手续费按网关声明的 `kind` 解析,它和网络对不对由各链确认. */
export async function fetchFees(network: string): Promise<Fees> {
    const body = await fetchFeeBody(network);
    if (!isChain(body.kind)) throw malformed();
    return chainOf(body.kind).parseFees(body);
}

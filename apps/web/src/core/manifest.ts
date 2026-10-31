// 内置清单.符号,小数位,图标只按"网络加合约地址"取,见 docs/04-security.md 第 2.8 节.
// 这里没有按符号查找的入口.清单只有核心库这一个来源,网页端和冷钱包用同一份.
import { formatUnits } from "./amount";
import { assets, networks } from "./pkg/bitone_wasm";
import { addressKey } from "./text";
import { t } from "./i18n";

/** 图标由注册表和通用代币表提供,清单只引用名字. */
export type IconName = string;

export interface AssetEntry {
    symbol: string;
    decimals: number;
    icon: IconName;
    minFraction: number;
    native: boolean;
}

type ManifestAsset = Omit<AssetEntry, "native"> & { network: string; contract: string };

function key(network: string, asset: string): string {
    return `${network}:${addressKey(asset)}`;
}

let manifest: Map<string, AssetEntry> | undefined;

function entries(): Map<string, AssetEntry> {
    manifest ??= new Map(
        (assets() as ManifestAsset[]).map(({ network, contract, ...entry }) => [
            key(network, contract),
            { ...entry, native: contract === "native" },
        ]),
    );
    return manifest;
}

/** 这个网络要查询的资产,原生币和清单里的合约. */
export function assetsOf(network: string): string[] {
    const listed = (assets() as ManifestAsset[]).filter((asset) => asset.network === network);
    return ["native", ...listed.map((asset) => asset.contract).filter((contract) => contract !== "native")];
}

/** `asset` 是 `native` 或合约地址.查不到时返回 null,界面按不在内置清单处理. */
export function lookupAsset(network: string, asset: string): AssetEntry | null {
    return entries().get(key(network, asset)) ?? null;
}

/** 清单里这个 icon 对应的符号.来自网址的资产键要先过这一关,界面只显示清单里有的. */
export function symbolOf(icon: string): string | null {
    for (const entry of entries().values()) if (entry.icon === icon) return entry.symbol;
    return null;
}

let pricedAs: Map<string, string> | undefined;

/** 测试网资产到同链同符号的主网资产.只含清单里的资产,骗子代币起同样的符号也没有价格. */
function pricedAsMainnet(): Map<string, string> {
    if (pricedAs) return pricedAs;
    const list = assets() as ManifestAsset[];
    const info = new Map((networks() as { id: string; chain: string; testnet: boolean }[]).map((n) => [n.id, n]));
    pricedAs = new Map();
    for (const { network, contract, symbol } of list) {
        const chain = info.get(network);
        if (!chain?.testnet) continue;
        const mainnet = list.find((other) => {
            const target = info.get(other.network);
            return other.symbol === symbol && target?.chain === chain.chain && !target.testnet;
        });
        if (mainnet) pricedAs.set(key(network, contract), key(mainnet.network, mainnet.contract));
    }
    return pricedAs;
}

/** 在价格表里查这个资产用的键.价格只有主网资产,测试网资产的估值只用于展示. */
export function priceKey(network: string, asset: string): string {
    const own = key(network, asset);
    return pricedAsMainnet().get(own) ?? own;
}

/** 金额的显示文字.不在内置清单时显示原始整数,不显示符号. */
export function formatAsset(network: string, asset: string, raw: bigint): string {
    const entry = lookupAsset(network, asset);
    if (!entry) return t("{0} 未知代币", raw.toString());
    return `${formatUnits(raw, entry.decimals, { min: entry.minFraction })} ${entry.symbol}`;
}

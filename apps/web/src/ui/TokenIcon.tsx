// 图标从 design/icons 打包.只接受内置清单的条目,不接受符号或链上名称.
import usdc from "../../../../design/icons/assets/usdc.svg";
import usdt from "../../../../design/icons/assets/usdt.svg";
import { CHAIN_LIST, NETWORK_FAMILIES, networkFamily } from "../chains";
import type { AssetEntry, IconName } from "../core/manifest";
import { t } from "../core/i18n";

/** 代币不属于某条链,原生币由各链提供. */
const ASSET_ICONS: Record<IconName, string> = Object.fromEntries([
    ["usdc", usdc],
    ["usdt", usdt],
    ...CHAIN_LIST.flatMap((spec) => spec.natives.map(({ icon, src }) => [icon, src])),
]);

const NETWORK_ICONS: Record<string, string> = Object.fromEntries(NETWORK_FAMILIES.map(({ id, src }) => [id, src]));

/** 网络图标的标识,对应 `chains/` 里声明的一组网络. */
export type NetworkIcon = string;

export const networkIcon = networkFamily;

interface TokenIconProps {
    /** 内置清单的条目.null 表示不在清单内,显示虚线问号. */
    asset?: AssetEntry | null;
    /** 不传 asset 时显示网络图标. */
    network?: NetworkIcon;
    /** 右下角的网络角标,代币在非原生链上时才有. */
    chain?: NetworkIcon;
    size: number;
}

/** 资产图标.代币在非原生链上时带网络角标. */
export function AssetIcon({ entry, network, size }: { entry: AssetEntry | null; network: string; size: number }) {
    return <TokenIcon asset={entry} chain={entry && !entry.native ? networkIcon(network) : undefined} size={size} />;
}

export function TokenIcon({ asset, network, chain, size }: TokenIconProps) {
    const src = (asset ? ASSET_ICONS[asset.icon] : network ? NETWORK_ICONS[network] : null) ?? null;
    const badge = (chain ? NETWORK_ICONS[chain] : null) ?? null;
    const badgeSize = Math.max(12, Math.round(size * 0.44));
    const box = { width: size, height: size };
    return (
        <span className="relative inline-flex shrink-0" style={box}>
            {src ? (
                // 换资产时不复用元素,否则新图加载完之前旁边的币种和图标对不上.
                <img key={src} src={src} alt="" className="block rounded-full" style={box} />
            ) : (
                <span
                    role="img"
                    aria-label={t("不在内置清单")}
                    className="flex items-center justify-center rounded-full border-[1.5px] border-dashed border-line-strong bg-surface-2 font-medium text-text-3"
                    style={{ ...box, fontSize: Math.round(size * 0.45) }}
                >
                    ?
                </span>
            )}
            {badge ? (
                <img
                    src={badge}
                    alt=""
                    className="absolute -right-0.5 -bottom-0.5 block rounded-full ring-2 ring-surface"
                    style={{ width: badgeSize, height: badgeSize }}
                />
            ) : null}
        </span>
    );
}

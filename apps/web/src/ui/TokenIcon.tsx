// 图标从 design/icons 打包。只接受内置清单的条目，不接受符号或链上名称。
import btc from "../../../../design/icons/assets/btc.svg";
import bnb from "../../../../design/icons/assets/bnb.svg";
import eth from "../../../../design/icons/assets/eth.svg";
import pol from "../../../../design/icons/assets/pol.svg";
import sol from "../../../../design/icons/assets/sol.svg";
import trx from "../../../../design/icons/assets/trx.svg";
import usdc from "../../../../design/icons/assets/usdc.svg";
import usdt from "../../../../design/icons/assets/usdt.svg";
import bitcoinNet from "../../../../design/icons/networks/bitcoin.svg";
import bscNet from "../../../../design/icons/networks/bsc.svg";
import ethereumNet from "../../../../design/icons/networks/ethereum.svg";
import polygonNet from "../../../../design/icons/networks/polygon.svg";
import solanaNet from "../../../../design/icons/networks/solana.svg";
import tronNet from "../../../../design/icons/networks/tron.svg";
import type { AssetEntry, IconName } from "../core/manifest";
import { t } from "../core/i18n";

const ASSET_ICONS: Record<IconName, string> = { btc, bnb, eth, pol, sol, trx, usdc, usdt };

export type NetworkIcon = "bitcoin" | "ethereum" | "bsc" | "polygon" | "tron" | "solana";

const NETWORK_ICONS: Record<NetworkIcon, string> = {
    bitcoin: bitcoinNet,
    ethereum: ethereumNet,
    bsc: bscNet,
    polygon: polygonNet,
    tron: tronNet,
    solana: solanaNet,
};

/** 网络标识对应的图标。测试网和主网用同一个。 */
export function networkIcon(network: string): NetworkIcon {
    if (network === "bitcoin" || network.startsWith("btc-")) return "bitcoin";
    if (network === "bsc" || network === "bsc-testnet") return "bsc";
    if (network === "polygon" || network === "amoy") return "polygon";
    if (network.startsWith("tron")) return "tron";
    if (network.startsWith("solana")) return "solana";
    return "ethereum";
}

interface TokenIconProps {
    /** 内置清单的条目。null 表示不在清单内，显示虚线问号。 */
    asset?: AssetEntry | null;
    /** 不传 asset 时显示网络图标。 */
    network?: NetworkIcon;
    /** 右下角的网络角标，代币在非原生链上时才有。 */
    chain?: NetworkIcon;
    size: number;
}

export function TokenIcon({ asset, network, chain, size }: TokenIconProps) {
    const src = asset ? ASSET_ICONS[asset.icon] : network ? NETWORK_ICONS[network] : null;
    const badge = chain ? NETWORK_ICONS[chain] : null;
    const badgeSize = Math.max(12, Math.round(size * 0.44));
    const box = { width: size, height: size };
    return (
        <span className="relative inline-flex shrink-0" style={box}>
            {src ? (
                // 换资产时不复用元素，否则新图加载完之前旁边的币种和图标对不上。
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

import bnb from "../../../../../design/icons/assets/bnb.svg";
import eth from "../../../../../design/icons/assets/eth.svg";
import pol from "../../../../../design/icons/assets/pol.svg";
import bscNet from "../../../../../design/icons/networks/bsc.svg";
import ethereumNet from "../../../../../design/icons/networks/ethereum.svg";
import polygonNet from "../../../../../design/icons/networks/polygon.svg";
import { accountAddress } from "../../core";
import { t } from "../../core/i18n";
import { SpeedCard } from "../SpeedCard";
import type { ChainSpec } from "../spec";
import { parseFees } from "./api";
import { quote } from "./quote";
import { raw, rawModes, replace, spentFlags, transfer } from "./transfer";
import { view } from "./view";

export const evm: ChainSpec = {
    id: "evm",
    title: "EVM",
    icon: "ethereum",
    kindLabels: { single: "Ethereum,BSC,Polygon" },
    namedByKind: false,
    keyLabel: () => t("账户公钥"),
    families: [
        {
            id: "ethereum",
            label: "Ethereum",
            src: ethereumNet,
            matches: (network) => network === "ethereum" || network === "sepolia" || network === "hoodi",
        },
        {
            id: "bsc",
            label: "BSC",
            src: bscNet,
            matches: (network) => network === "bsc" || network === "bsc-testnet",
        },
        {
            id: "polygon",
            label: "Polygon",
            src: polygonNet,
            matches: (network) => network === "polygon" || network === "amoy",
        },
    ],
    natives: [
        { icon: "eth", src: eth, color: "var(--chart-1)" },
        { icon: "bnb", src: bnb, color: "var(--chart-4)" },
        { icon: "pol", src: pol, color: "var(--chart-7)" },
    ],
    explorers: {
        ethereum: (hash) => `https://etherscan.io/tx/${hash}`,
        sepolia: (hash) => `https://sepolia.etherscan.io/tx/${hash}`,
        hoodi: (hash) => `https://hoodi.etherscan.io/tx/${hash}`,
        bsc: (hash) => `https://bscscan.com/tx/${hash}`,
        "bsc-testnet": (hash) => `https://testnet.bscscan.com/tx/${hash}`,
        polygon: (hash) => `https://polygonscan.com/tx/${hash}`,
        amoy: (hash) => `https://amoy.polygonscan.com/tx/${hash}`,
    },
    hdAddresses: false,
    deriveAddress: (account) => accountAddress("evm", account.key),
    wholeTxEntries: false,
    caseSensitiveHash: false,
    checksummed: (typed, canonical) => typed === canonical,
    signerKey: (account) => account.key,
    parseFees,
    failureNote: "合约回滚,手续费已扣除",
    overdueAfter: 30 * 60_000,
    spentFlags,
    transfer,
    replace: { build: replace },
    rawModes,
    raw,
    quote,
    view,
    BroadcastCard: SpeedCard,
};

import btc from "../../../../../design/icons/assets/btc.svg";
import bitcoinNet from "../../../../../design/icons/networks/bitcoin.svg";
import { t } from "../../core/i18n";
import { planOf } from "../../data/db";
import { SpeedCard } from "../SpeedCard";
import type { ChainSpec } from "../spec";
import { EXECUTION_FAILED } from "../view";
import { parseFees } from "./api";
import { deriveAddress } from "./core";
import { quote } from "./quote";
import { replace, spentFlags, transfer } from "./transfer";
import { view } from "./view";

export const bitcoin: ChainSpec = {
    id: "bitcoin",
    title: "Bitcoin",
    icon: "bitcoin",
    kindLabels: {
        segwit: "Native SegWit",
        taproot: "Taproot",
        "nested-segwit": "Nested SegWit",
        legacy: "Legacy",
    },
    namedByKind: true,
    keyLabel: () => t("扩展公钥"),
    families: [
        {
            id: "bitcoin",
            label: "Bitcoin",
            src: bitcoinNet,
            matches: (network) => network === "bitcoin" || network.startsWith("btc-"),
        },
    ],
    natives: [{ icon: "btc", src: btc, color: "var(--chart-2)" }],
    explorers: {
        bitcoin: (hash) => `https://mempool.space/tx/${hash}`,
        "btc-testnet4": (hash) => `https://mempool.space/testnet4/tx/${hash}`,
        "btc-signet": (hash) => `https://mempool.space/signet/tx/${hash}`,
        "btc-testnet3": (hash) => `https://mempool.space/testnet/tx/${hash}`,
    },
    hdAddresses: true,
    deriveAddress,
    wholeTxEntries: true,
    caseSensitiveHash: false,
    checksummed: () => false,
    // 公钥在 PSBT 里.
    signerKey: () => new Uint8Array(),
    parseFees,
    failureNote: EXECUTION_FAILED,
    overdueAfter: 24 * 60 * 60_000,
    changeIndex: (tx) => planOf(tx, "bitcoin").changeIndex,
    spentFlags,
    discoveryNotice: () => t("Bitcoin 余额可能不全."),
    transfer,
    replace: { build: replace },
    quote,
    view,
    BroadcastCard: SpeedCard,
};

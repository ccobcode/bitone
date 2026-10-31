import sol from "../../../../../design/icons/assets/sol.svg";
import solanaNet from "../../../../../design/icons/networks/solana.svg";
import { accountAddress } from "../../core";
import { t } from "../../core/i18n";
import type { ChainSpec } from "../spec";
import { EXECUTION_FAILED } from "../view";
import { parseFees } from "./api";
import { BroadcastCard } from "./BroadcastCard";
import { quote } from "./quote";
import { replace, setup, spentFlags, transfer } from "./transfer";
import { view } from "./view";

export const solana: ChainSpec = {
    id: "solana",
    title: "Solana",
    icon: "solana",
    kindLabels: { single: "SPL" },
    namedByKind: false,
    keyLabel: () => t("账户公钥"),
    families: [{ id: "solana", label: "Solana", src: solanaNet, matches: (network) => network.startsWith("solana") }],
    natives: [{ icon: "sol", src: sol, color: "var(--chart-5)" }],
    explorers: {
        solana: (hash) => `https://solscan.io/tx/${hash}`,
        "solana-devnet": (hash) => `https://solscan.io/tx/${hash}?cluster=devnet`,
        "solana-testnet": (hash) => `https://solscan.io/tx/${hash}?cluster=testnet`,
    },
    hdAddresses: false,
    deriveAddress: (account) => accountAddress("solana", account.key),
    wholeTxEntries: false,
    // Base58 区分大小写.
    caseSensitiveHash: true,
    checksummed: () => false,
    signerKey: (account) => account.key,
    parseFees,
    failureNote: EXECUTION_FAILED,
    overdueAfter: null,
    spentFlags,
    transfer,
    replace: { build: replace },
    setup,
    quote,
    view,
    BroadcastCard,
};

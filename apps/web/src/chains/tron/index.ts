import trx from "../../../../../design/icons/assets/trx.svg";
import tronNet from "../../../../../design/icons/networks/tron.svg";
import { accountAddress } from "../../core";
import { t } from "../../core/i18n";
import type { ChainSpec } from "../spec";
import { EXECUTION_FAILED } from "../view";
import { parseFees } from "./api";
import { BroadcastCard } from "./BroadcastCard";
import { quote } from "./quote";
import { raw, rawModes, transfer } from "./transfer";
import { view } from "./view";

export const tron: ChainSpec = {
    id: "tron",
    title: "TRON",
    icon: "tron",
    kindLabels: { single: "TRC-20" },
    namedByKind: false,
    keyLabel: () => t("账户公钥"),
    families: [{ id: "tron", label: "TRON", src: tronNet, matches: (network) => network.startsWith("tron") }],
    natives: [{ icon: "trx", src: trx, color: "var(--chart-8)" }],
    explorers: {
        tron: (hash) => `https://tronscan.org/#/transaction/${hash}`,
        "tron-nile": (hash) => `https://nile.tronscan.org/#/transaction/${hash}`,
        "tron-shasta": (hash) => `https://shasta.tronscan.org/#/transaction/${hash}`,
    },
    hdAddresses: false,
    deriveAddress: (account) => accountAddress("tron", account.key),
    wholeTxEntries: false,
    caseSensitiveHash: false,
    checksummed: () => true,
    signerKey: (account) => account.key,
    parseFees,
    failureNote: EXECUTION_FAILED,
    overdueAfter: null,
    transfer,
    // 没有序号,重建就是另一笔独立的交易,可能重复付款,所以不提供.
    replace: { refusal: () => t("TRON 不提供加速和重建,超过过期时间后自动失效.") },
    rawModes,
    raw,
    quote,
    view,
    BroadcastCard,
};

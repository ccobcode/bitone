// 浏览器存储,表结构见 docs/03-system.md 第 3 节.
import { Dexie, type Table } from "dexie";
import type { ReservationType, TxPlan } from "../chains";
import type { AccountKind, Chain, Mode } from "../core";
import { t } from "../core/i18n";
import type { Flows } from "../gateway/client";
import type { ChainStatus } from "./machine";

export type { ReservationType, TxPlan };

export interface Wallet {
    /** 指纹加模式.同一个冷钱包的主网和测试网是两个观察钱包. */
    id: string;
    fingerprint: string;
    name: string;
    mode: Mode;
    pairedAt: number;
}

export interface Account {
    /** 钱包加路径,重新配对时不会产生重复账户. */
    id: string;
    walletId: string;
    chain: Chain;
    kind: AccountKind;
    network: string;
    path: string;
    key: Uint8Array;
}

export interface Address {
    accountId: string;
    /** IndexedDB 的键不能是布尔值,所以用 0 和 1. */
    change: 0 | 1;
    index: number;
    address: string;
    verified: boolean;
    /** 链上有记录,或者本地交易已经用过.Bitcoin 的地址发现和找零分配用它. */
    used?: boolean;
}

export interface Setting {
    key: string;
    value: unknown;
}

/** 交易生命周期,见 docs/03-system.md 第 7 节. */
export type TxState =
    | "signing"
    | "signed"
    | "abandoned"
    | "broadcasting"
    | "broadcast"
    | "uncertain"
    | "replaced"
    | "expired"
    | "confirming"
    | "confirmed"
    | "failed";

/** `speedup` 和 `cancel` 是替换交易,`rebuild` 是结果不明时的重建,都复用原交易的序号.`raw` 来自原始请求页. */
export type TxKind = "transfer" | "speedup" | "cancel" | "rebuild" | "create-nonce" | "raw";

export interface TimelineEntry {
    state: TxState;
    at: number;
    note?: string;
}

export interface TxRecord {
    /** 业务编号,如 TX-20261030-0001. */
    id: string;
    walletId: string;
    accountId: string;
    chain: Chain;
    network: string;
    kind: TxKind;
    state: TxState;
    /** `native` 或合约地址. */
    asset: string;
    /** 最小单位的整数字符串. */
    amount: string;
    from: string;
    to: string;
    /** 未签名数据,`bitone-request` 的消息. */
    request: Uint8Array;
    requestId: string;
    /** 签名数据,可以直接广播的十六进制. */
    raw?: string;
    hash?: string;
    /** 核验时发现手续费没有经过前序交易确认,显示的金额只是 PSBT 的声明. */
    feeUnconfirmed?: boolean;
    plan: TxPlan;
    createdAt: number;
    updatedAt: number;
    /** 待签名的本地期限.`hardExpiry` 为真时签名本身也在这时作废,否则只是手续费报价. */
    expiresAt: number | null;
    hardExpiry: boolean;
    timeline: TimelineEntry[];
    replaces?: string;
    replacedBy?: string;
    attempts: number;
    broadcastAt?: number;
    /** 最近一次广播失败时网关的原文,只当纯文本显示. */
    lastError?: string;
    /** 连续几轮看到替换证据. */
    conflicts: number;
    conflictAt?: number;
    /** 链上显示执行失败,但还没到最终阶段. */
    reverted?: boolean;
    confirmations: number;
    stage: string | null;
    block: number | null;
    fee: string | null;
}

export type NewTx = Pick<
    TxRecord,
    | "walletId"
    | "accountId"
    | "chain"
    | "network"
    | "kind"
    | "asset"
    | "amount"
    | "from"
    | "to"
    | "request"
    | "requestId"
    | "plan"
    | "expiresAt"
    | "hardExpiry"
    | "replaces"
>;

export function planOf<C extends TxPlan["chain"]>(tx: TxRecord, chain: C): Extract<TxPlan, { chain: C }> {
    if (tx.plan.chain !== chain) throw new Error(t("交易类型不符"));
    return tx.plan as Extract<TxPlan, { chain: C }>;
}

/** 待处理交易占用的序号,替换交易和原交易共用一条. */
export interface Reservation {
    id: string;
    accountId: string;
    network: string;
    type: ReservationType;
    value: string;
    txIds: string[];
    createdAt: number;
}

export interface Contact {
    id: string;
    name: string;
    chain: Chain;
    mode: Mode;
    address: string;
    /** 从备份导入的联系人,确认后才能用于转账. */
    pending: boolean;
    createdAt: number;
}

/** 交易的唯一标识:`网络:哈希`.十六进制哈希统一成小写,Solana 的 Base58 区分大小写,不能转换. */
export function txKey(network: string, hash: string): string {
    return `${network}:${hash.startsWith("0x") || /^[0-9a-f]{64}$/i.test(hash) ? hash.toLowerCase() : hash}`;
}

/** 从网关同步下来的链上记录.随时能重新拉取,不进备份. */
export interface ChainEntry {
    /** 一笔交易可以有多条转账,所以在 [txKey] 后面带上内容.Bitcoin 每笔只有一条. */
    id: string;
    walletId: string;
    /** 这条记录来自哪批地址,回滚时只清这批的. */
    stream: string;
    txKey: string;
    network: string;
    hash: string;
    time: number | null;
    from: string | null;
    to: string | null;
    asset: string;
    amount: string;
    status: ChainStatus;
    /** Bitcoin 的输入输出,显示时按整个钱包归纳. */
    flows?: Flows;
    /** 最后一次在快照或推送里看到它的时间. */
    seenAt?: number;
}

/** 一批地址的同步进度.新地址另成一批,已有的批次和水位不受影响. */
export interface FeedCursor {
    /** `钱包:网络:这批的第一个地址`. */
    id: string;
    network: string;
    /** 翻页游标绑定这批地址,不能改动. */
    addresses: string[];
    /** 水位.这之前的记录不会再变,下次只查这之后的. */
    checkpoint: string;
    /** 往前翻页的游标. */
    older: string | null;
}

/** 余额的本地缓存.`height` 是取到它时的高度,只有不低于它的结果才能覆盖. */
export interface BalanceRow {
    /** `网络|地址|资产`. */
    id: string;
    network: string;
    address: string;
    asset: string;
    amount: string;
    height: number;
}

export interface Note {
    /** [txKey],链上重新拉取的记录也能对上. */
    id: string;
    text: string;
    updatedAt: number;
}

export class BitOneDb extends Dexie {
    wallets!: Table<Wallet, string>;
    accounts!: Table<Account, string>;
    addresses!: Table<Address, [string, number, number]>;
    settings!: Table<Setting, string>;
    txs!: Table<TxRecord, string>;
    reservations!: Table<Reservation, string>;
    contacts!: Table<Contact, string>;
    notes!: Table<Note, string>;
    chainTxs!: Table<ChainEntry, string>;
    feedCursors!: Table<FeedCursor, string>;
    balances!: Table<BalanceRow, string>;

    constructor() {
        super("bitone");
        this.version(1).stores({
            wallets: "id, mode",
            accounts: "id, walletId",
            addresses: "[accountId+change+index], accountId, address",
            settings: "key",
            txs: "id",
            reservations: "id",
            contacts: "id",
        });
        this.version(2).stores({
            txs: "id, walletId, state, hash",
            reservations: "id, accountId, *txIds",
            contacts: "id, [chain+mode]",
            notes: "id",
        });
        this.version(3)
            .stores({ chainTxs: "id, walletId", feedCursors: "id" })
            .upgrade(async (tx) => {
                const notes = tx.table<Note, string>("notes");
                for (const note of await notes.toArray()) {
                    const split = note.id.indexOf(":");
                    const id = txKey(note.id.slice(0, split), note.id.slice(split + 1));
                    if (id === note.id) continue;
                    await notes.delete(note.id);
                    await notes.put({ ...note, id });
                }
            });
        // 旧游标没有记下地址,只能从头同步.链上记录随时能重新拉取.
        this.version(4)
            .stores({ balances: "id, network" })
            .upgrade(async (tx) => {
                await tx.table("feedCursors").clear();
                await tx.table("chainTxs").clear();
            });
    }
}

export const db = new BitOneDb();

/** 清除全部本地数据,之后可以继续使用同一个连接. */
export async function clearLocalData(): Promise<void> {
    await db.transaction("rw", db.tables, () => Promise.all(db.tables.map((table) => table.clear())));
}

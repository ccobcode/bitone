// 一条链要回答的全部问题.注册表是 `Record<Chain, ChainSpec>`,漏实现时编译失败.
import type { ComponentType } from "react";
import type { AccountKind, Chain } from "../core";
import type { DeriveAddress } from "../data/addresses";
import type { Claim } from "../data/claims";
import type { Account, NewTx, TxRecord } from "../data/db";
import type { Fees } from "./index";
import type { QuoteInput, QuotePlan } from "./quote";
import type { ReplaceDraft, ReplaceMode, SetupDraft, TransferDraft } from "./transfer";
import type { Finality, TxFact } from "./view";

/** 一笔待写入的交易.写入和加锁由 `data/transfer.ts` 统一完成,失败时什么也不写. */
export interface Built {
    fields: NewTx;
    claims: Claim[];
}

/** 手续费由注册表解析,各链拿到后自己确认形式. */
export type FeeSource = (network: string) => Promise<Fees>;

export type Replacement =
    /** 这条链不提供替换,这是拒绝时给用户的说明. */
    | { refusal(): string }
    | { build(tx: TxRecord, mode: ReplaceMode, draft: ReplaceDraft, fees: FeeSource): Promise<Built> };

/** 一组共用图标的网络.它同时是记录页的筛选项. */
export interface NetworkFamily {
    id: string;
    label: string;
    src: string;
    matches(network: string): boolean;
}

/** 这条链的原生币.代币的图标不属于某条链. */
export interface NativeAsset {
    icon: string;
    src: string;
    color: string;
}

export interface BroadcastCardProps {
    tx: TxRecord;
    busy: boolean;
    failure: string;
    overdue: boolean;
    replace(mode: ReplaceMode): Promise<void>;
    resend(): Promise<void>;
}

export interface ChainView {
    /** 手续费上限或实际手续费的摘要,W4 的摘要区和核对列表用. */
    fee(tx: TxRecord): TxFact;
    /** 这笔的序号,W4 摘要和结果不明时要显示. */
    sequence(tx: TxRecord): TxFact;
    /** W4 收到签名后自动核对的项目. */
    checks(tx: TxRecord, common: TxFact, signer: TxFact): TxFact[];
    /** W5 里这条链特有的行. */
    details(tx: TxRecord): TxFact[];
    finality(tx: TxRecord): Finality;
    cancelNote(tx: TxRecord): string;
    /** 发替换交易的按钮文字.不能替换时是 null. */
    replaceLabel(tx: TxRecord): string | null;
    countdownLabel(tx: TxRecord): string;
    /** 广播结果不明时的说明. */
    uncertainBody(sequence: TxFact): string;
    failedBody(): string;
}

export interface ChainSpec {
    id: Chain;
    title: string;
    /** 账户的图标,对应 `families` 里的一项. */
    icon: string;
    /** 账户类型的说明.Bitcoin 有多种账户,其余链只有 `single`. */
    kindLabels: Partial<Record<AccountKind, string>>;
    /** 账户名称要带上类型. */
    namedByKind: boolean;
    keyLabel(): string;
    families: NetworkFamily[];
    natives: NativeAsset[];
    explorers: Record<string, (hash: string) => string>;

    /** 每个账户有多个地址:用地址发现,区分接收和找零. */
    hdAddresses: boolean;
    deriveAddress: DeriveAddress;
    /** 记录页的条目带输入输出,从哪批地址看都是同一笔,所以一笔只留一条. */
    wholeTxEntries: boolean;
    caseSensitiveHash: boolean;
    /** 用户输入的地址经过了校验和检查. */
    checksummed(typed: string, canonical: string): boolean;
    signerKey(account: Pick<Account, "key">): Uint8Array;
    parseFees(body: Record<string, unknown>): Fees;
    /** 链上失败的说明,存进时间线,显示时才翻译. */
    failureNote: string;
    /** 已广播超过这么久提示加速或取消.不能加速的链是 null. */
    overdueAfter: number | null;
    /** 这笔用掉的找零索引. */
    changeIndex?(tx: TxRecord): number | null;
    /** 这一组交易的序号有没有被用掉.没有序号的链不实现. */
    spentFlags?(network: string, txs: readonly TxRecord[]): Promise<Map<string, boolean>>;
    discoveryNotice?(): string;

    transfer(draft: TransferDraft, fees: FeeSource): Promise<Built>;
    replace: Replacement;
    /** 首次转账前要先上链的准备步骤. */
    setup?(draft: SetupDraft): Promise<Built>;
    quote(input: QuoteInput): QuotePlan;
    view: ChainView;
    /** 已广播还没打包时显示的操作. */
    BroadcastCard: ComponentType<BroadcastCardProps>;
}

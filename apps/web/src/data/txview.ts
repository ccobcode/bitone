// 交易详情的显示文字，W4、W5 共用。金额和手续费都从构建记录里取。
import { networkList, networkName } from "../core";
import { formatUnits } from "../core/amount";
import { formatAsset, lookupAsset } from "../core/manifest";
import { shortAddress } from "../core/text";
import type { TxRecord, TxState } from "./db";
import { STATE_LABELS } from "./machine";
import { t, translate } from "../core/i18n";

export interface TxFact {
    label: string;
    value: string;
    mono: boolean;
}

function nativeText(tx: Pick<TxRecord, "network">, raw: bigint): string {
    const native = lookupAsset(tx.network, "native");
    return native ? `${formatUnits(raw, native.decimals)} ${native.symbol}` : raw.toString();
}

export function amountText(tx: TxRecord): string {
    return formatAsset(tx.network, tx.asset, BigInt(tx.amount));
}

/** 手续费上限或实际手续费的摘要，W4 的摘要区和核对列表用。 */
export function feeFact(tx: TxRecord): TxFact {
    const { plan } = tx;
    switch (plan.chain) {
        case "evm":
            return {
                label: t("手续费上限"),
                value: nativeText(tx, BigInt(plan.gasLimit) * BigInt(plan.maxFee)),
                mono: false,
            };
        case "bitcoin":
            return { label: t("手续费"), value: nativeText(tx, BigInt(plan.fee)), mono: false };
        case "tron":
            return { label: "fee_limit", value: nativeText(tx, BigInt(plan.feeLimit)), mono: false };
        case "solana":
            return {
                label: t("手续费"),
                value: nativeText(tx, 5000n + solanaPriority(plan.priority, plan.computeUnits)),
                mono: false,
            };
    }
}

function solanaPriority(price: number, units: number): bigint {
    return (BigInt(price) * BigInt(units) + 999_999n) / 1_000_000n;
}

/** 这笔的序号，W4 摘要和结果不明时要显示。 */
export function sequenceFact(tx: TxRecord): TxFact {
    const { plan } = tx;
    switch (plan.chain) {
        case "evm":
            return { label: "Nonce", value: String(plan.nonce), mono: false };
        case "bitcoin":
            return {
                label: t("找零索引"),
                value: plan.changeIndex === null ? t("无找零") : String(plan.changeIndex),
                mono: false,
            };
        case "tron":
            return { label: t("过期时间"), value: t("30 分钟"), mono: false };
        case "solana":
            return { label: t("nonce 账户"), value: shortAddress(plan.nonceAccount), mono: true };
    }
}

/** W4 收到签名后自动核对的项目，与 docs/03-system.md 第 8 节一致。 */
export function verifyChecks(tx: TxRecord): TxFact[] {
    const { plan } = tx;
    const common: TxFact = { label: t("请求 ID"), value: tx.requestId.slice(0, 8), mono: true };
    const signer: TxFact = { label: t("签名者"), value: shortAddress(tx.from), mono: true };
    switch (plan.chain) {
        case "evm": {
            const chainId = networkList().find((item) => item.id === tx.network)?.chainId;
            return [
                common,
                signer,
                { label: t("收款地址"), value: shortAddress(tx.to), mono: true },
                { label: t("金额和数据"), value: amountText(tx), mono: false },
                { label: "Nonce", value: String(plan.nonce), mono: false },
                { label: "Gas Limit", value: plan.gasLimit.toLocaleString("en-US"), mono: false },
                feeFact(tx),
                { label: "Chain ID", value: String(chainId ?? "—"), mono: false },
            ];
        }
        case "bitcoin":
            return [
                common,
                { label: t("输入"), value: t("{0} 个", plan.inputs.length), mono: false },
                { label: t("输出"), value: t("{0} 个", plan.changePath && tx.kind !== "cancel" ? 2 : 1), mono: false },
                { label: t("找零路径"), value: plan.changePath ?? t("无找零"), mono: true },
            ];
        case "tron":
            return [
                common,
                signer,
                { label: t("raw_data 字节"), value: t("完全一致"), mono: false },
                {
                    label: t("合约类型"),
                    value: tx.asset === "native" ? "TransferContract" : "TriggerSmartContract",
                    mono: false,
                },
                { label: t("过期时间"), value: t("30 分钟"), mono: false },
                feeFact(tx),
            ];
        case "solana":
            return [
                common,
                signer,
                { label: t("消息字节"), value: t("完全一致"), mono: false },
                { label: t("nonce 账户"), value: shortAddress(plan.nonceAccount), mono: true },
            ];
    }
}

/** W5 的交易详情。 */
export function detailFacts(tx: TxRecord): TxFact[] {
    const { plan } = tx;
    const rows: TxFact[] = [
        { label: t("网络"), value: networkName(tx.network), mono: false },
        { label: t("发送方"), value: tx.from, mono: true },
        { label: t("接收方"), value: tx.to, mono: true },
        { label: t("金额"), value: amountText(tx), mono: false },
    ];
    if (tx.asset !== "native") rows.push({ label: t("代币合约"), value: tx.asset, mono: true });
    switch (plan.chain) {
        case "evm": {
            const chainId = networkList().find((item) => item.id === tx.network)?.chainId;
            rows.push(
                { label: "Chain ID", value: String(chainId ?? "—"), mono: false },
                { label: "Nonce", value: String(plan.nonce), mono: false },
                { label: t("Gas 上限"), value: plan.gasLimit.toLocaleString("en-US"), mono: false },
                feeFact(tx),
                {
                    label: t("实际手续费"),
                    value: tx.fee ? nativeText(tx, BigInt(tx.fee)) : t("打包后显示"),
                    mono: false,
                },
            );
            break;
        }
        case "bitcoin":
            rows.push(
                { label: t("输入"), value: t("{0} 个", plan.inputs.length), mono: false },
                { label: t("找零"), value: plan.changePath ?? t("无找零"), mono: true },
                { label: t("手续费"), value: nativeText(tx, BigInt(plan.fee)), mono: false },
                { label: t("费率"), value: `${plan.feeRate} sat/vB`, mono: false },
                { label: "RBF", value: t("开启"), mono: false },
            );
            break;
        case "tron":
            rows.push(
                { label: t("过期时间"), value: t("30 分钟"), mono: false },
                { label: "fee_limit", value: nativeText(tx, BigInt(plan.feeLimit)), mono: false },
                {
                    label: t("实际手续费"),
                    value: tx.fee ? nativeText(tx, BigInt(tx.fee)) : t("打包后显示"),
                    mono: false,
                },
            );
            break;
        case "solana":
            rows.push(
                { label: t("nonce 账户"), value: plan.nonceAccount, mono: true },
                { label: t("基础费"), value: nativeText(tx, 5000n), mono: false },
                {
                    label: t("优先费"),
                    value: nativeText(tx, solanaPriority(plan.priority, plan.computeUnits)),
                    mono: false,
                },
            );
            break;
    }
    return rows;
}

export interface Finality {
    labels: string[];
    reached: number;
    count: boolean;
    goal: string;
    progress: string;
}

/** 最终性进度，标准见 docs/03-system.md 第 9 节。Bitcoin 显示确认数，其余链显示阶段。 */
export function finality(tx: TxRecord): Finality {
    const done = tx.state === "confirmed";
    const packed = tx.state === "confirming" || tx.state === "failed" || done;
    const stage = (labels: string[], stages: string[], goal: string): Finality => {
        const index = stages.indexOf(tx.stage ?? "");
        const reached = done ? labels.length : packed ? Math.max(index + 1, 1) : 0;
        return {
            labels,
            reached,
            count: false,
            goal,
            progress: reached > 0 ? (labels[reached - 1] ?? "") : t("等待打包"),
        };
    };
    switch (tx.chain) {
        case "bitcoin": {
            const reached = done ? 6 : Math.min(tx.confirmations, 6);
            return {
                labels: ["1", "2", "3", "4", "5", "6"],
                reached,
                count: true,
                goal: t("达到 6 个确认后完成"),
                progress: `${reached}/6`,
            };
        }
        case "evm":
            return stage(
                [t("已打包"), t("安全"), t("最终")],
                ["latest", "safe", "finalized"],
                t("到达 finalized 区块后完成"),
            );
        case "tron":
            return stage([t("已打包"), t("区块确认")], ["included", "solidified"], t("区块确认后完成"));
        case "solana":
            return stage(
                [t("已处理"), t("已投票"), t("最终")],
                ["processed", "confirmed", "finalized"],
                t("到达 finalized 提交等级后完成"),
            );
    }
}

const STEP_LABELS: Partial<Record<TxState, string>> = {
    signing: t("已构建"),
    signed: t("冷钱包已签名"),
    confirming: t("已打包"),
};

export interface TimelineStep {
    title: string;
    time: number | null;
    note: string | null;
    phase: "done" | "current" | "todo";
}

/** W5 的进度，走过的步骤来自时间线，后面还要走的按原型列出来。 */
export function timelineSteps(tx: TxRecord): TimelineStep[] {
    const done = tx.timeline.map((entry, i): TimelineStep => ({
        title: STEP_LABELS[entry.state] ?? STATE_LABELS[entry.state],
        time: entry.at,
        note: entry.note ? translate(entry.note) : entry.state === "signing" ? tx.id : null,
        phase: i === tx.timeline.length - 1 ? "current" : "done",
    }));
    const todo = (titles: string[]): TimelineStep[] =>
        titles.map((title) => ({ title, time: null, note: null, phase: "todo" }));
    switch (tx.state) {
        case "signing":
            return [...done, ...todo([t("冷钱包已签名"), t("已广播"), t("已打包"), t("确认中"), t("已确认")])];
        case "signed":
            return [...done, ...todo([t("已广播"), t("已打包"), t("确认中"), t("已确认")])];
        case "broadcasting":
        case "broadcast":
        case "uncertain":
            return [...done, ...todo([t("已打包"), t("确认中"), t("已确认")])];
        case "confirming":
            return [
                ...done.map((step) => ({ ...step, phase: "done" as const })),
                { title: t("确认中"), time: null, note: finality(tx).goal, phase: "current" },
                ...todo([t("已确认")]),
            ];
        default:
            return done;
    }
}

/** 取消时说明签名是否仍然有效，见 docs/03-system.md 第 9 节。 */
export function cancelNote(tx: TxRecord): string {
    if (tx.kind === "create-nonce")
        return t("如果已在冷钱包签名，这笔交易在 blockhash 过期前仍然有效，过期后自动作废。");
    switch (tx.chain) {
        case "evm":
            return t("如果已在冷钱包签名，这笔交易仍然有效，发同 nonce 的替换交易才算作废。");
        case "bitcoin":
            return t("如果已在冷钱包签名，这笔交易仍然有效，发同一组输入的替换交易才算作废。");
        case "tron":
            return t("如果已在冷钱包签名，这笔交易在过期前仍然有效，无法主动作废。");
        case "solana":
            return t("如果已在冷钱包签名，这笔交易仍然有效，推进 nonce 才算作废。");
    }
}

/** 重新发起的入口。取消和创建 nonce 账户这两种不是用户的转账，不能预填内容。 */
export function retryLink(tx: Pick<TxRecord, "id" | "kind" | "accountId" | "network" | "asset">): string {
    if (tx.kind === "cancel" || tx.kind === "create-nonce") {
        return `/send?account=${encodeURIComponent(tx.accountId)}&network=${encodeURIComponent(tx.network)}&asset=native`;
    }
    return `/send?from=${encodeURIComponent(tx.id)}`;
}

/** 这笔能不能发替换交易，按钮文字按链不同。TRON 不能作废，创建 nonce 账户的 blockhash 自己会过期。 */
export function replaceLabel(tx: TxRecord): string | null {
    if (tx.kind === "create-nonce" || tx.chain === "tron") return null;
    return tx.chain === "solana" ? t("推进 nonce") : t("发替换交易");
}

/** W4 倒计时的含义。EVM 和 Bitcoin 只是手续费报价，签名本身不过期。 */
export function countdownLabel(tx: TxRecord): string {
    if (tx.kind === "create-nonce") return t("blockhash 窗口还剩");
    if (tx.chain === "tron") return t("签名过期还剩");
    if (tx.chain === "solana") return t("签名不过期，直到 nonce 推进");
    return t("手续费报价还剩");
}

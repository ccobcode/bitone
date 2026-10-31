// TRON 的构建.没有序号可以占用,防重放靠过期时间和交易哈希.
import { t } from "../../core/i18n";
import type { Built, FeeSource } from "../spec";
import { TransferError, badFees, header, requestFields, reviewRequest, type TransferDraft } from "../transfer";
import { estimateTron, fetchRefBlock, type TronEstimate, type TronResources } from "./api";
import { buildTronTransfer } from "./core";

export interface TronPlan {
    chain: "tron";
    feeLimit: number;
}

/** TRON 默认过期时间只有 60 秒,二维码往返来不及,构建时显式设置. */
export const TRON_LIFETIME = 30 * 60_000;
/** 引用区块的时间和本机差太多时不用,防止网关把签名有效期拉长. */
const REF_BLOCK_DRIFT = 60 * 60_000;

/** TRC-20 的调用数据.TRON 地址的编码在核心库里,没有单独的接口,所以从预览里读回来. */
export function tronCallData(draft: Omit<TransferDraft, "tier">): string {
    const now = Date.now();
    const request = buildTronTransfer({
        ...header(draft.wallet, draft.network),
        path: draft.account.path,
        from: draft.from,
        to: draft.to,
        amount: draft.amount.toString(),
        contract: draft.asset,
        refBlockBytes: "0000",
        refBlockHash: "0000000000000000",
        expirationMs: now + TRON_LIFETIME,
        timestampMs: now,
        feeLimit: 1_000_000,
    });
    return reviewRequest(request, draft.accounts)
        .raw.map((line) => line.text)
        .join("");
}

export interface TronCost {
    /** 能量不足时燃烧的 TRX,单位 sun. */
    energyBurn: bigint;
    /** 带宽不足时整笔按字节燃烧. */
    bandwidthBurn: bigint;
    activationFee: bigint;
    total: bigint;
}

export function tronCost(
    estimate: TronEstimate,
    resources: TronResources,
    energyPrice: bigint,
    bandwidthPrice: bigint,
): TronCost {
    const missing = estimate.energy > resources.energy ? estimate.energy - resources.energy : 0n;
    const energyBurn = missing * energyPrice;
    const bandwidthBurn = estimate.bandwidth > resources.bandwidth ? estimate.bandwidth * bandwidthPrice : 0n;
    return {
        energyBurn,
        bandwidthBurn,
        activationFee: estimate.activationFee,
        total: energyBurn + bandwidthBurn + estimate.activationFee,
    };
}

/** 默认的 fee_limit:全部能量都靠燃烧时的花费,再留 20% 余量. */
export function tronFeeLimit(energy: bigint, energyPrice: bigint): bigint {
    return (energy * energyPrice * 12n) / 10n;
}

export async function transfer(draft: TransferDraft, fees: FeeSource): Promise<Built> {
    const { account, network, from, to, amount, asset } = draft;
    const feeData = await fees(network);
    if (feeData.kind !== "tron") throw badFees();
    const token = asset !== "native";
    const call = token ? { from, to, contract: asset, data: tronCallData(draft) } : { from, to };
    const [estimate, block] = await Promise.all([estimateTron(network, call), fetchRefBlock(network)]);
    if (Math.abs(block.timestamp - Date.now()) > REF_BLOCK_DRIFT) {
        throw new TransferError("invalid", t("引用区块的时间异常,已停止构建."));
    }
    const feeLimit = token ? (draft.feeLimit ?? tronFeeLimit(estimate.energy, feeData.energyPrice)) : 0n;
    // 过期时间从区块时间算,不依赖本机时钟.
    const expirationMs = block.timestamp + TRON_LIFETIME;
    const head = header(draft.wallet, network);
    const request = buildTronTransfer({
        ...head,
        path: account.path,
        from,
        to,
        amount: amount.toString(),
        ...(token ? { contract: asset } : {}),
        refBlockBytes: block.refBlockBytes,
        refBlockHash: block.refBlockHash,
        expirationMs,
        timestampMs: block.timestamp,
        feeLimit: Number(feeLimit),
    });
    reviewRequest(request, draft.accounts);
    const plan: TronPlan = { chain: "tron", feeLimit: Number(feeLimit) };
    return {
        fields: requestFields(draft, head.id, request, plan, amount.toString(), "transfer", expirationMs, true),
        claims: [],
    };
}

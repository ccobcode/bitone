package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.FieldRow
import com.ccobcode.bitone.platform.FieldSource
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.ReviewRow
import com.ccobcode.bitone.platform.Risk

internal const val CURRENT_CONTROLLER = "当前控制方"
internal const val NEW_CONTROLLER = "新控制方"

/** 核心库在权限被移除时写在新控制方一行的值,它不是地址. */
internal const val NO_CONTROLLER = "无,权限被永久移除"
internal val PARTY_LABELS = setOf("收款地址", "目标合约", "被授权方")

/** Solana 的付款方在 rows 里叫手续费支付者. */
private val PAYER_LABELS = setOf("付款账户", "手续费支付者")

/** 和核心库的 `Network::mode()` 一致,[networkName] 给的就是这条链的主网.不认识的网络也算主网,宁可多提示一次. */
fun isMainnet(network: String): Boolean {
    val info = networkInfo(network) ?: return true
    return info.name == networkName(info.icon)
}

/** A6d 顶部卡片.[own] 表示当前控制方就是付款账户. */
class ControllerCard(val current: String?, val next: String?, val own: Boolean)

/** 全部动作只有一组控制方时才有卡片.不止一组时返回 null,每一组都留在自己的动作里. */
fun controllerCard(review: ReviewResult): ControllerCard? {
    val rows = review.actions.flatMap { it.rows }
    val currents = rows.valuesOf(CURRENT_CONTROLLER)
    val nexts = rows.valuesOf(NEW_CONTROLLER)
    if (currents.size > 1 || nexts.size > 1) return null
    val payer = review.rows.firstOrNull { it.label in PAYER_LABELS }?.value
    val current = currents.firstOrNull() ?: payer
    return ControllerCard(current, nexts.firstOrNull() ?: review.counterparty, current != null && current == payer)
}

/** 每个动作列出的行.A6d 的对方和卡片上的控制方已经在上面,这里不重复. */
fun actionRows(review: ReviewResult, style: ReviewStyle): List<List<ReviewRow>> {
    if (style != ReviewStyle.Permission) return review.actions.map { it.rows }
    val above = if (controllerCard(review) == null) PARTY_LABELS else PARTY_LABELS + CURRENT_CONTROLLER + NEW_CONTROLLER
    return review.actions.map { action -> action.rows.filterNot { it.label in above } }
}

private fun List<ReviewRow>.valuesOf(label: String) = filter { it.label == label }.map { it.value }.distinct()

/** 红色样式.无法识别虽然排在上面,但它有自己的黄色样式. */
fun isHighRisk(risk: Risk) = risk != Risk.Unknown && risk >= Risk.Approval

/** 通用列表的一块.[foldable] 的行可以折叠. */
class FieldBlock(val rows: List<FieldRow>, val foldable: Boolean)

/** 相邻的 `context` 行合成一块,只有它们可以折叠.行的顺序不变,一行也不少. */
fun fieldBlocks(fields: List<FieldRow>): List<FieldBlock> {
    val blocks = mutableListOf<Pair<Boolean, MutableList<FieldRow>>>()
    for (row in fields) {
        val foldable = row.source == FieldSource.Context
        val last = blocks.lastOrNull()
        if (last?.first == foldable) last.second += row else blocks += foldable to mutableListOf(row)
    }
    return blocks.map { (foldable, rows) -> FieldBlock(rows, foldable) }
}

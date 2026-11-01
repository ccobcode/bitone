package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.FieldRow
import com.ccobcode.bitone.platform.FieldSource
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.Risk

internal const val CURRENT_CONTROLLER = "当前控制方"
internal const val NEW_CONTROLLER = "新控制方"

/** 核心库在权限被移除时写在新控制方一行的值,它不是地址. */
internal const val NO_CONTROLLER = "无,权限被永久移除"

/** 这笔权限变更没有新控制方时,卡片上的文字. */
internal const val NO_NEW_CONTROLLER = "没有新控制方"

/** A6d 顶部卡片.[own] 表示当前控制方就是付款账户.[next] 为空表示这笔权限变更没有新控制方. */
class ControllerCard(val current: String?, val next: String?, val own: Boolean)

/** 全部动作只有一组控制方时才有卡片.不止一组时返回 null,每一组都在下面的字段列表里. */
fun controllerCard(review: ReviewResult): ControllerCard? {
    val summary = review.summary
    val currents = summary.controllers.mapNotNull { it.current }.distinct()
    val nexts = summary.controllers.map { it.next }.distinct()
    if (currents.size > 1 || nexts.size > 1) return null
    val payer = summary.payer
    val current = currents.firstOrNull() ?: payer
    return ControllerCard(current, nexts.firstOrNull(), current != null && current == payer)
}

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

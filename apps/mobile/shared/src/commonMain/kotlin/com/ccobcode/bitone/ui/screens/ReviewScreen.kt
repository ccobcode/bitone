package com.ccobcode.bitone.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.Network
import com.ccobcode.bitone.flow.ReviewStyle
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.networkInfo
import com.ccobcode.bitone.flow.reviewStyle
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.RawLine
import com.ccobcode.bitone.platform.ReviewAmount
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.ReviewRow
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.AddressCard
import com.ccobcode.bitone.ui.components.AssetIcon
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.HoldButton
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.SemiBold
import com.ccobcode.bitone.ui.components.TextButton
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.UnlistedIcon
import com.ccobcode.bitone.ui.components.exitSpec
import com.ccobcode.bitone.ui.components.rememberLatest
import com.ccobcode.bitone.ui.components.stateSpec
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.LineHeight
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Space

private const val FROM_WEB = "BitOne 网页端不会发起这类交易。"

/** 每个输入固定这几行，A6e 要把它们从详情里分出来。 */
private val INPUT_LABELS = setOf("付款账户", "派生路径", "输入金额")
private val PARTY_LABELS = setOf("收款地址", "目标合约", "被授权方")

/** 第一屏直接显示的字段，其余进「详情」。这只是展示分组。 */
private val FIRST_SCREEN_LABELS = setOf("网络", "手续费上限", "手续费", "基础费", "fee_limit")
private const val CURRENT_CONTROLLER = "当前控制方"
private const val NEW_CONTROLLER = "新控制方"

/**
 * A6 到 A6e。分类全部来自核心库，这里按 [reviewStyle] 选样式，字段原样画出来。
 *
 * 第一屏只放金额、对方地址、风险、网络和手续费，进入时不做错开，只随页面一起出现。其余字段放在「详情」里，一项都不省略，普通交易默认折叠，高风险样式默认展开。分组只看标签，风险判断全在核心库。
 */
@Composable
fun ReviewScreen(flow: WalletFlow) {
    val review = rememberLatest(flow.review) ?: return
    val c = BitOneTheme.colors
    val style = reviewStyle(review)
    val network = networkInfo(review.network)
    Page(
        header = { TitleBar("核对交易", onBack = { flow.back() }) { StyleChip(review, style) } },
        contentGap = if (style == ReviewStyle.Plain) Space.s5 else Space.s4,
        bottom = { ReviewActions(flow, review) },
    ) {
        Banner(review, style)
        when (style) {
            ReviewStyle.Plain -> {
                AmountBlock(review, network?.icon, kind = kindLabel(review))
                Counterparty(review)
            }
            ReviewStyle.Approval -> {
                AmountBlock(review, network?.icon, kind = "授权", danger = true)
                Counterparty(review)
            }
            ReviewStyle.Unknown -> {
                if (review.amount != null) InlineAmount(review, network?.icon)
                Counterparty(review, leading = { UnlistedIcon(24.dp) }) { Chip("未知合约", c.warnSoft, c.warn) }
            }
            ReviewStyle.Permission -> {
                // 权限变更可以和转账打包在一笔交易里，金额不能藏起来。
                if (review.amount != null) InlineAmount(review, network?.icon)
                PackedParties(review)
                Controllers(review)
            }
            ReviewStyle.HighRisk -> {
                InlineAmount(review, network?.icon)
                Counterparty(review)
            }
        }
        val inline = Actions(review, style)
        Rows(review.rows.filter { it.label in FIRST_SCREEN_LABELS })
        val rest = inline + review.rows.filterNot { it.label in FIRST_SCREEN_LABELS }
        Details(rest, style, bitcoin = network?.icon == Network.Bitcoin)
        RawData(review.raw, initiallyOpen = style == ReviewStyle.Unknown)
    }
}

/** 普通转账才是绿色。 */
@Composable
private fun StyleChip(review: ReviewResult, style: ReviewStyle) {
    val c = BitOneTheme.colors
    when (style) {
        ReviewStyle.Plain ->
            if (review.risk == Risk.Transfer) {
                Chip("普通转账", c.okSoft, c.ok)
            } else {
                Chip("已识别调用", c.accentSoft, c.accentText)
            }
        ReviewStyle.Unknown -> Chip("无法识别", c.warnSoft, c.warn)
        else -> Chip("高风险", c.dangerSoft, c.danger)
    }
}

private fun kindLabel(review: ReviewResult): String? =
    when {
        review.risk != Risk.Transfer -> review.title
        review.amount?.asset?.endsWith(":native") == true -> "原生币转账"
        review.amount != null -> "代币转账"
        else -> null
    }

/** 风险横幅。A6 没有。告警不为空时都列出来，一条也不省略。 */
@Composable
private fun Banner(review: ReviewResult, style: ReviewStyle) {
    val c = BitOneTheme.colors
    when (style) {
        ReviewStyle.Plain -> Unit
        ReviewStyle.Approval -> {
            val symbol = review.amount?.symbol
            val unlimited = review.amount?.unlimited == true
            RiskBanner(
                title = if (unlimited) "无限额度授权" else "授权交易",
                lines =
                    listOf(
                        when {
                            unlimited && symbol != null -> "对方可随时转走你全部的 $symbol。"
                            unlimited -> "对方可随时转走你全部的这种资产。"
                            else -> "对方可以动用授权额度内的资产。"
                        },
                        FROM_WEB,
                    ) + review.alerts,
            )
        }
        ReviewStyle.Unknown ->
            RiskBanner(
                warn = true,
                title = "本机无法解析这笔交易",
                lines = listOf("只在确认来源可信时签名。", FROM_WEB) + review.alerts,
            )
        ReviewStyle.Permission ->
            RiskBanner(
                title = "账户控制权将被改变",
                lines = listOf("签名后，新控制方可以动用这个账户的全部资产。", FROM_WEB) + review.alerts,
            )
        ReviewStyle.HighRisk ->
            RiskBanner(
                title = "发现高风险项",
                lines = review.alerts,
            )
    }
}

@Composable
private fun RiskBanner(title: String, lines: List<String>, warn: Boolean = false) {
    val c = BitOneTheme.colors
    val tone = if (warn) c.warn else c.danger
    Row(
        Modifier.fillMaxWidth()
            .background(if (warn) c.warnSoft else c.dangerSoft, RoundedCornerShape(Radius.r3))
            .padding(Space.s4),
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
    ) {
        LineIcon(if (warn) Icons.Unknown else Icons.Alert, 20.dp, tone)
        Column(verticalArrangement = Arrangement.spacedBy(Space.s1)) {
            Txt(title, weight = SemiBold, color = tone)
            lines.forEach { Txt(it, size = FontSize.t2) }
        }
    }
}

/** 金额区，靠边线左对齐。 */
@Composable
private fun AmountBlock(
    review: ReviewResult,
    network: Network?,
    kind: String?,
    danger: Boolean = false,
) {
    val c = BitOneTheme.colors
    val amount = review.amount
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
        if (amount == null) {
            Txt(review.title, size = FontSize.t5, weight = SemiBold)
            if (kind != null) KindChip(kind, danger)
        } else {
            Row(
                Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                AssetIcon(amount.icon, network, 40.dp)
                if (kind != null) KindChip(kind, danger)
            }
            AmountText(amount, FontSize.t8, danger)
            if (amount.asset == null) Chip("不在内置清单", c.warnSoft, c.warn)
        }
    }
}

/** 高风险页要放下横幅和几张地址卡片，金额和图标排成一行。主角仍然是金额，只是不占两行。 */
@Composable
private fun InlineAmount(review: ReviewResult, network: Network?) {
    val c = BitOneTheme.colors
    val amount = review.amount ?: return
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s3)) {
        AssetIcon(amount.icon, network, 32.dp)
        Box(Modifier.weight(1f, fill = false)) { AmountText(amount, FontSize.t7, danger = false) }
        if (amount.asset == null) Chip("不在内置清单", c.warnSoft, c.warn)
    }
}

/** 位数很多的原始整数不能截断，字号跟着长度缩小，放不下时换行。 */
@Composable
private fun AmountText(amount: ReviewAmount, emphasis: TextUnit, danger: Boolean) {
    val c = BitOneTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
        if (amount.unlimited) {
            Txt(
                "无限",
                Modifier.alignByBaseline(),
                size = emphasis,
                weight = SemiBold,
                color = if (danger) c.danger else c.text,
            )
        } else {
            Txt(
                amount.text,
                Modifier.weight(1f, fill = false).alignByBaseline(),
                size = amountSize(amount.text, emphasis),
                weight = SemiBold,
            )
        }
        if (amount.symbol != null) {
            Txt(amount.symbol, Modifier.alignByBaseline(), size = FontSize.t4, color = c.text2)
        }
    }
}

@Composable
private fun KindChip(kind: String, danger: Boolean) {
    val c = BitOneTheme.colors
    if (danger) Chip(kind, c.dangerSoft, c.danger) else Chip(kind, c.accentSoft, c.accentText)
}

private fun amountSize(text: String, emphasis: TextUnit): TextUnit =
    when {
        text.length <= 12 -> emphasis
        text.length <= 18 -> FontSize.t6
        else -> FontSize.t4
    }

@Composable
private fun Counterparty(
    review: ReviewResult,
    leading: @Composable (() -> Unit)? = null,
    badge: @Composable (() -> Unit)? = null,
) {
    val address = review.counterparty ?: return
    AddressCard(review.counterpartyLabel.orEmpty(), address, leading = leading, badge = badge)
}

/** 权限变更打包了转账或调用时，对方要放在第一屏。「长按签名」一直在屏幕上，不能让用户滚动后才看到收款方。 */
@Composable
private fun PackedParties(review: ReviewResult) {
    val parties = review.actions.flatMap { it.rows }.filter { it.label in PARTY_LABELS }
    parties.forEach { AddressCard(it.label, it.value) }
}

/** A6d。当前控制方在动作的行里，没有时就是付款账户。 */
@Composable
private fun Controllers(review: ReviewResult) {
    val c = BitOneTheme.colors
    val inner = review.actions.flatMap { it.rows }
    val current =
        inner.firstOrNull { it.label == CURRENT_CONTROLLER }?.value
            ?: review.rows.firstOrNull { it.label == "付款账户" }?.value
    val next = review.counterparty ?: inner.firstOrNull { it.label == NEW_CONTROLLER }?.value
    if (current != null) {
        AddressCard(CURRENT_CONTROLLER, current) { Chip("本账户", c.okSoft, c.ok) }
    }
    if (next != null) {
        AddressCard(NEW_CONTROLLER, next, c.dangerSoft) { Chip("外部地址", c.surface, c.danger) }
    }
}

/** 全部动作。Bitcoin 的动作就是全部输出，多个动作也都留在第一屏，不能折叠起来。控制方已经在上面的卡片里，这里不重复。 只有一个动作时不用标题，返回它的字段，放进「详情」。 */
@Composable
private fun Actions(review: ReviewResult, style: ReviewStyle): List<ReviewRow> {
    val bitcoin = networkInfo(review.network)?.icon == Network.Bitcoin
    val shown =
        review.actions.map { action ->
            if (style == ReviewStyle.Permission) {
                action.rows.filterNot {
                    it.label == CURRENT_CONTROLLER || it.label == NEW_CONTROLLER || it.label in PARTY_LABELS
                }
            } else {
                action.rows
            }
        }
    if (bitcoin || review.actions.size > 1) {
        Section(if (bitcoin) "全部输出" else "全部动作") {
            review.actions.forEachIndexed { index, action -> ActionBlock(action.title, action.risk, shown[index]) }
        }
        return emptyList()
    }
    return shown.flatten()
}

/** 分组标题在外，内容是一块列表组，组内靠行距分开，没有分隔线。 */
@Composable
private fun Section(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        Txt(title, size = FontSize.t2, color = BitOneTheme.colors.text3)
        Card {
            Column(Modifier.padding(Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s5), content = content)
        }
    }
}

@Composable
private fun ActionBlock(title: String, risk: Risk, rows: List<ReviewRow>) {
    val c = BitOneTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            Txt(title, Modifier.weight(1f), weight = Medium)
            when {
                risk >= Risk.Approval -> Chip("高风险", c.dangerSoft, c.danger)
                risk == Risk.Unknown -> Chip("无法识别", c.warnSoft, c.warn)
                title == "找零" -> Chip("本钱包", c.okSoft, c.ok)
            }
        }
        rows.forEach { row -> DetailRow(row.label, row.value, row.mono) }
    }
}

/** 普通交易默认折叠，无法识别、授权、权限变更和 A6e 默认展开。A6e 的 Bitcoin 输入单独分组。 */
@Composable
private fun Details(rows: List<ReviewRow>, style: ReviewStyle, bitcoin: Boolean) {
    if (rows.isEmpty()) return
    Disclosure("详情", initiallyOpen = style != ReviewStyle.Plain) {
        if (style == ReviewStyle.HighRisk && bitcoin) {
            val inputs = rows.filter { it.label in INPUT_LABELS }
            if (inputs.isNotEmpty()) Txt("全部输入", size = FontSize.t2, color = BitOneTheme.colors.text3)
            inputs.chunked(INPUT_LABELS.size).forEach { group -> Rows(group) }
            Rows(rows.filterNot { it.label in INPUT_LABELS })
        } else {
            Rows(rows)
        }
    }
}

/** 标签和值分行，比上面的金额和地址都轻。 */
@Composable
private fun Rows(rows: List<ReviewRow>) {
    if (rows.isEmpty()) return
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
        rows.forEach { row -> DetailRow(row.label, row.value, row.mono) }
    }
}

/** 可展开的一块，高度和透明度同时过渡。 */
@Composable
private fun Disclosure(title: String, initiallyOpen: Boolean, content: @Composable ColumnScope.() -> Unit) {
    val c = BitOneTheme.colors
    var open by remember { mutableStateOf(initiallyOpen) }
    val turn by animateFloatAsState(if (open) 180f else 0f, stateSpec(), label = "chevron")
    Card {
        ListRow(onClick = { open = !open }) {
            Txt(title, Modifier.weight(1f))
            LineIcon(Icons.Chevron, 16.dp, c.text3, Modifier.rotate(turn))
        }
        AnimatedVisibility(
            open,
            enter = fadeIn(stateSpec()) + expandVertically(stateSpec()),
            exit = fadeOut(exitSpec()) + shrinkVertically(exitSpec()),
        ) {
            Column(
                Modifier.padding(start = Space.s4, end = Space.s4, bottom = Space.s4),
                verticalArrangement = Arrangement.spacedBy(Space.s3),
                content = content,
            )
        }
    }
}

/** 原始数据。A6c 默认展开，其他折叠。 */
@Composable
private fun RawData(lines: List<RawLine>, initiallyOpen: Boolean) {
    val c = BitOneTheme.colors
    val visible = lines.filter { it.text.isNotEmpty() }
    Disclosure("原始数据", initiallyOpen) {
        if (visible.isEmpty()) Txt("没有附加数据", size = FontSize.t2, color = c.text3)
        visible.forEachIndexed { index, line ->
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s3)) {
                Txt(index.toString().padStart(2, '0'), Modifier.width(20.dp), size = FontSize.t1, color = c.text3)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(Space.s1)) {
                    Txt(line.text, size = FontSize.t1, color = c.text2, mono = true, lineHeight = LineHeight.t2)
                    line.note?.let { note ->
                        if (note == "大额数值") Chip(note, c.warnSoft, c.warn) else Chip(note, c.accentSoft, c.accentText)
                    }
                }
            }
        }
    }
}

/** 普通交易的主按钮是确认，拒绝是文字按钮。高风险时拒绝是主按钮，长按放在下面。 */
@Composable
private fun ReviewActions(flow: WalletFlow, review: ReviewResult) {
    ActionArea {
        if (review.confirm == ConfirmMode.Tap) {
            PrimaryButton("确认并签名", enabled = !flow.busy, onClick = flow::confirmTap)
            TextButton("拒绝", onClick = flow::exitRequest)
        } else {
            PrimaryButton("拒绝", onClick = flow::exitRequest)
            HoldButton(
                "长按签名",
                flow.holdProgress,
                enabled = !flow.busy,
                onPress = flow::holdStart,
                onRelease = flow::holdCancel,
            )
        }
    }
}

/** A7 顶部的交易摘要，一行列表组，宽度对齐边线。 */
@Composable
internal fun SignSummary(review: ReviewResult) {
    val c = BitOneTheme.colors
    val amount = review.amount
    Card {
        ListRow {
            AssetIcon(amount?.icon, networkInfo(review.network)?.icon, 32.dp)
            Column(Modifier.weight(1f)) {
                Txt(amountLine(review).ifEmpty { review.title }, weight = Medium)
                review.counterparty?.let { address ->
                    Row(horizontalArrangement = Arrangement.spacedBy(Space.s1)) {
                        Txt("至", size = FontSize.t2, color = c.text3)
                        Txt(shortAddress(address), size = FontSize.t2, color = c.text2, mono = true)
                    }
                }
            }
        }
    }
}

/** 无限额度用文字说明。 */
internal fun amountLine(review: ReviewResult): String {
    val amount = review.amount ?: return ""
    val text = if (amount.unlimited) "无限" else amount.text
    return listOfNotNull(text, amount.symbol).joinToString(" ")
}

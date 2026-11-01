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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.CURRENT_CONTROLLER
import com.ccobcode.bitone.flow.NEW_CONTROLLER
import com.ccobcode.bitone.flow.NO_CONTROLLER
import com.ccobcode.bitone.flow.NO_NEW_CONTROLLER
import com.ccobcode.bitone.flow.Network
import com.ccobcode.bitone.flow.ReviewStyle
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.controllerCard
import com.ccobcode.bitone.flow.fieldBlocks
import com.ccobcode.bitone.flow.isHighRisk
import com.ccobcode.bitone.flow.networkInfo
import com.ccobcode.bitone.flow.reviewStyle
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.FieldKind
import com.ccobcode.bitone.platform.FieldRow
import com.ccobcode.bitone.platform.ReviewAmount
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.AddressCard
import com.ccobcode.bitone.ui.components.AssetIcon
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.DangerButton
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
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Space

private const val SCROLL_FIRST = "滚到底部后确认"
private const val TARGET_CONTRACT = "目标合约"

/** 字段列表的缩进层数上限,再深的节点靠 `depth` 的顺序分辨,值的宽度不能被缩进吃掉. */
private const val MAX_INDENT = 4
private val MONO_KINDS = setOf(FieldKind.Address, FieldKind.Hash, FieldKind.Bytes, FieldKind.Script, FieldKind.Opaque)

/**
 * A6 到 A6e.分类全部来自核心库,这里按 [reviewStyle] 选样式,字段原样画出来.
 *
 * 第一屏只放金额,对方地址,风险和网络,进入时不做错开,只随页面一起出现.下面是通用字段列表,一项都不省略,风险判断全在核心库.
 *
 * 主网请求在最上面提示,网络名在金额上方.内容滚到底之后确认按钮才可用.
 */
@Composable
fun ReviewScreen(flow: WalletFlow) {
    val review = rememberLatest(flow.scan.review) ?: return
    val c = BitOneTheme.colors
    val style = reviewStyle(review)
    val network = networkInfo(review.network)
    val scroll = rememberScrollState()
    LaunchedEffect(scroll) { snapshotFlow { scroll.canScrollForward }.collect { flow.scan.reviewScrolled(it) } }
    Page(
        header = { TitleBar("核对交易", onBack = { flow.back() }) { StyleChip(review, style) } },
        contentGap = if (style == ReviewStyle.Plain) Space.s5 else Space.s4,
        bottom = { ReviewActions(flow, review) },
        scroll = scroll,
    ) {
        if (review.mainnet) RiskBanner("主网,真实资产", emptyList(), warn = true)
        Banner(review, style)
        Txt(network?.name ?: review.network, weight = Medium, color = c.text2)
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
                if (review.summary.amount != null) InlineAmount(review, network?.icon)
                val contract = review.summary.counterpartyLabel == TARGET_CONTRACT
                Counterparty(review, leading = { UnlistedIcon(24.dp) }) {
                    if (contract) Chip("未知合约", c.warnSoft, c.warn)
                }
            }
            ReviewStyle.Permission -> {
                // 权限变更可以和转账打包在一笔交易里,金额不能藏起来.
                if (review.summary.amount != null) InlineAmount(review, network?.icon)
                PackedParties(review)
                Controllers(review)
            }
            ReviewStyle.HighRisk -> {
                InlineAmount(review, network?.icon)
                Counterparty(review)
            }
        }
        flow.report?.warnings?.forEach { RiskBanner(it.label, listOf(it.value), warn = true) }
        FieldList(flow, review.fields, style)
    }
}

/** 通用字段列表,不按链,不按字段名分支.全部行都在滚动内容里,滚到底才能确认.画出的行数报给 [flow],签名时核心库要核对. */
@Composable
private fun FieldList(flow: WalletFlow, fields: List<FieldRow>, style: ReviewStyle) {
    val blocks = remember(fields) { fieldBlocks(fields) }
    LaunchedEffect(blocks) { flow.scan.fieldsRendered(blocks.sumOf { it.rows.size }) }
    blocks.forEach { block ->
        if (block.foldable) {
            Disclosure("附加字段", initiallyOpen = style != ReviewStyle.Plain) { block.rows.forEach { FieldLine(it) } }
        } else {
            Card {
                Column(
                    Modifier.fillMaxWidth().padding(Space.s4),
                    verticalArrangement = Arrangement.spacedBy(Space.s3),
                ) {
                    block.rows.forEach { FieldLine(it) }
                }
            }
        }
    }
}

/** 值是链上的内容,不翻译. */
@Composable
private fun FieldLine(row: FieldRow) {
    val c = BitOneTheme.colors
    Column(
        Modifier.fillMaxWidth().padding(start = Space.s3 * minOf(row.depth.toInt(), MAX_INDENT)),
        verticalArrangement = Arrangement.spacedBy(Space.s1),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            Txt(row.label, Modifier.weight(1f), size = FontSize.t2, color = c.text3)
            row.risk?.let { RiskChip(it) }
            if (row.own) Chip("本账户", c.okSoft, c.ok)
        }
        if (row.value.isNotEmpty()) {
            Txt(row.value, size = FontSize.t2, mono = row.kind in MONO_KINDS, translated = false)
        }
        row.note?.let { Txt(it, size = FontSize.t2, color = c.warn) }
    }
}

@Composable
private fun RiskChip(risk: Risk) {
    val c = BitOneTheme.colors
    when {
        risk == Risk.Unknown -> Chip("无法识别", c.warnSoft, c.warn)
        isHighRisk(risk) -> Chip("高风险", c.dangerSoft, c.danger)
    }
}

/** 普通转账才是绿色. */
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
        review.risk != Risk.Transfer -> review.summary.title
        review.summary.amount?.asset?.endsWith(":native") == true -> "原生币转账"
        review.summary.amount != null -> "代币转账"
        else -> null
    }

/** 风险横幅.A6 没有.告警不为空时都列出来,一条也不省略. */
@Composable
private fun Banner(review: ReviewResult, style: ReviewStyle) {
    val c = BitOneTheme.colors
    when (style) {
        ReviewStyle.Plain -> Unit
        ReviewStyle.Approval -> {
            val symbol = review.summary.amount?.symbol
            val unlimited = review.summary.amount?.unlimited == true
            RiskBanner(
                title = if (unlimited) "无限额度授权" else "授权交易",
                lines =
                    listOf(
                        when {
                            unlimited && symbol != null -> "对方可随时转走你全部的 $symbol."
                            unlimited -> "对方可随时转走你全部的这种资产."
                            else -> "对方可以动用授权额度内的资产."
                        }
                    ) + review.alerts,
            )
        }
        ReviewStyle.Unknown ->
            RiskBanner(
                warn = true,
                icon = Icons.Unknown,
                title = "本机无法解析这笔交易",
                lines = listOf("只在确认来源可信时签名.") + review.alerts,
            )
        ReviewStyle.Permission ->
            RiskBanner(
                title = "账户控制权将被改变",
                lines = listOf("签名后,新控制方可以动用这个账户的全部资产.") + review.alerts,
            )
        ReviewStyle.HighRisk ->
            RiskBanner(
                title = "发现高风险项",
                lines = review.alerts,
            )
    }
}

@Composable
private fun RiskBanner(title: String, lines: List<String>, warn: Boolean = false, icon: String = Icons.Alert) {
    val c = BitOneTheme.colors
    val tone = if (warn) c.warn else c.danger
    Row(
        Modifier.fillMaxWidth()
            .background(if (warn) c.warnSoft else c.dangerSoft, RoundedCornerShape(Radius.r3))
            .padding(Space.s4),
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
    ) {
        LineIcon(icon, 20.dp, tone)
        Column(verticalArrangement = Arrangement.spacedBy(Space.s1)) {
            Txt(title, weight = SemiBold, color = tone)
            lines.forEach { Txt(it, size = FontSize.t2) }
        }
    }
}

/** 金额区,靠边线左对齐. */
@Composable
private fun AmountBlock(
    review: ReviewResult,
    network: Network?,
    kind: String?,
    danger: Boolean = false,
) {
    val c = BitOneTheme.colors
    val amount = review.summary.amount
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
        if (amount == null) {
            Txt(review.summary.title, size = FontSize.t5, weight = SemiBold)
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

/** 高风险页要放下横幅和几张地址卡片,金额和图标排成一行.主角仍然是金额,只是不占两行. */
@Composable
private fun InlineAmount(review: ReviewResult, network: Network?) {
    val c = BitOneTheme.colors
    val amount = review.summary.amount ?: return
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s3)) {
        AssetIcon(amount.icon, network, 32.dp)
        Box(Modifier.weight(1f, fill = false)) { AmountText(amount, FontSize.t7, danger = false) }
        if (amount.asset == null) Chip("不在内置清单", c.warnSoft, c.warn)
    }
}

/** 位数很多的原始整数不能截断,字号跟着长度缩小,放不下时换行. */
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
    val address = review.summary.counterparty ?: return
    AddressCard(review.summary.counterpartyLabel.orEmpty(), address, leading = leading, badge = badge)
}

/** 权限变更打包了转账或调用时,对方要放在第一屏."长按签名"一直在屏幕上,不能让用户滚动后才看到收款方. */
@Composable
private fun PackedParties(review: ReviewResult) {
    review.summary.parties.forEach { AddressCard(it.label, it.value) }
}

/** A6d.当前控制方来自核心库的摘要,没有时就是付款账户.控制方不止一组时没有卡片,都在下面的字段列表里. */
@Composable
private fun Controllers(review: ReviewResult) {
    val c = BitOneTheme.colors
    val card = controllerCard(review) ?: return
    if (card.current != null) {
        AddressCard(CURRENT_CONTROLLER, card.current) { if (card.own) Chip("本账户", c.okSoft, c.ok) }
    }
    when (card.next) {
        null -> ControllerNote(NO_NEW_CONTROLLER, c.surface)
        NO_CONTROLLER -> ControllerNote(NO_CONTROLLER, c.dangerSoft)
        else -> AddressCard(NEW_CONTROLLER, card.next, c.dangerSoft) { Chip("外部地址", c.surface, c.danger) }
    }
}

/** 新控制方不是地址时的卡片.文字要翻译,不按地址分组. */
@Composable
private fun ControllerNote(text: String, tint: Color) {
    Card(color = tint) {
        Column(Modifier.padding(Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            Txt(NEW_CONTROLLER, size = FontSize.t2, color = BitOneTheme.colors.text3)
            Txt(text)
        }
    }
}

/** 可展开的一块,高度和透明度同时过渡. */
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

/** 普通交易的主按钮是确认,拒绝是文字按钮.高风险时拒绝是主按钮,长按放在下面.拒绝不用等滚到底. */
@Composable
private fun ReviewActions(flow: WalletFlow, review: ReviewResult) {
    val read = flow.scan.reviewRead
    ActionArea {
        when {
            review.confirm == ConfirmMode.Tap -> {
                PrimaryButton(
                    if (read) "确认并签名" else SCROLL_FIRST,
                    enabled = read && !flow.busy,
                    onClick = flow.scan::confirmTap,
                )
                TextButton("拒绝", onClick = flow.scan::exitRequest)
            }
            flow.scan.awaitingSecondConfirm -> {
                Txt("本机看不到这段内容的含义,签名后无法撤回.", size = FontSize.t2, color = BitOneTheme.colors.danger)
                PrimaryButton("拒绝", onClick = flow.scan::exitRequest)
                DangerButton("再次确认并签名", enabled = !flow.busy, onClick = flow.scan::confirmSecond)
                TextButton("返回核对", onClick = flow.scan::cancelSecondConfirm)
            }
            else -> {
                PrimaryButton("拒绝", onClick = flow.scan::exitRequest)
                HoldButton(
                    if (read) "长按签名" else SCROLL_FIRST,
                    flow.scan.holdProgress,
                    enabled = read && !flow.busy,
                    onPress = flow.scan::holdStart,
                    onRelease = flow.scan::holdCancel,
                )
            }
        }
    }
}

/** A7 顶部的交易摘要,一行列表组,宽度对齐边线. */
@Composable
internal fun SignSummary(review: ReviewResult) {
    val c = BitOneTheme.colors
    val amount = review.summary.amount
    Card {
        ListRow {
            AssetIcon(amount?.icon, networkInfo(review.network)?.icon, 32.dp)
            Column(Modifier.weight(1f)) {
                Txt(amountLine(review).ifEmpty { review.summary.title }, weight = Medium)
                review.summary.counterparty?.let { address ->
                    Row(horizontalArrangement = Arrangement.spacedBy(Space.s1)) {
                        Txt("至", size = FontSize.t2, color = c.text3)
                        Txt(shortAddress(address), size = FontSize.t2, color = c.text2, mono = true)
                    }
                }
            }
        }
    }
}

/** 无限额度用文字说明. */
internal fun amountLine(review: ReviewResult): String {
    val amount = review.summary.amount ?: return ""
    val text = if (amount.unlimited) "无限" else amount.text
    return listOfNotNull(text, amount.symbol).joinToString(" ")
}

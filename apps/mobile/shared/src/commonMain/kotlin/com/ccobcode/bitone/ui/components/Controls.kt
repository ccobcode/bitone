package com.ccobcode.bitone.ui.components

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.i18n.translate
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size
import com.ccobcode.bitone.ui.theme.Space
import com.ccobcode.bitone.ui.theme.elevation

/**
 * 可点击区域,无水波,按下时整体缩放.放在修饰链的最前面,缩放才能带上后面的底色和圆角.
 *
 * 全部按钮和可点的行都走这里,按压反馈全局一致.
 */
fun Modifier.tap(
    enabled: Boolean = true,
    role: Role = Role.Button,
    source: MutableInteractionSource? = null,
    onClick: () -> Unit,
): Modifier = composed {
    val interaction = source ?: remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    pressScale(pressed && enabled)
        .clickable(
            interactionSource = interaction,
            indication = null,
            enabled = enabled,
            role = role,
            onClick = onClick,
        )
}

private enum class Tone {
    Primary,
    Secondary,
    Danger,
}

@Composable
private fun ActionButton(
    text: String,
    tone: Tone,
    enabled: Boolean,
    modifier: Modifier,
    fill: Color = BitOneTheme.colors.surface,
    onClick: () -> Unit,
) {
    val c = BitOneTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val background by
        animateColorAsState(
            when {
                !enabled -> c.surface3
                tone == Tone.Primary -> if (pressed) c.accentHover else c.accent
                tone == Tone.Danger -> c.dangerSoft
                // 次级按钮比所在的底色亮一层.放在 bg 上时是 surface,弹层里换成 surface-3.
                else -> if (pressed) c.surface2 else fill
            },
            stateSpec(Motion.fast),
            label = "button",
        )
    val foreground =
        when {
            !enabled -> c.text3
            tone == Tone.Primary -> c.onAccent
            tone == Tone.Danger -> c.danger
            else -> c.text
        }
    Box(
        modifier
            .tap(enabled, source = source, onClick = onClick)
            .height(Size.hBtn)
            .background(background, RoundedCornerShape(Radius.r2)),
        contentAlignment = Alignment.Center,
    ) {
        Txt(text, weight = Medium, color = foreground)
    }
}

@Composable
fun PrimaryButton(text: String, enabled: Boolean = true, onClick: () -> Unit) =
    ActionButton(text, Tone.Primary, enabled, Modifier.fillMaxWidth(), onClick = onClick)

@Composable
fun SecondaryButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    fill: Color = BitOneTheme.colors.surface,
    onClick: () -> Unit,
) = ActionButton(text, Tone.Secondary, enabled, modifier, fill, onClick)

@Composable
fun DangerButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    onClick: () -> Unit,
) = ActionButton(text, Tone.Danger, enabled, modifier, onClick = onClick)

private val SmallHeight = 44.dp

/** 小按钮,高 44,和网页端手机上的 sm 尺寸相同. */
@Composable
fun SmallButton(text: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    Box(
        Modifier.tap(enabled, onClick = onClick)
            .height(SmallHeight)
            .background(if (enabled) c.surface else c.surface3, RoundedCornerShape(Radius.r2))
            .padding(horizontal = Space.s3),
        contentAlignment = Alignment.Center,
    ) {
        Txt(text, size = FontSize.t2, weight = Medium, color = if (enabled) c.text else c.text3)
    }
}

/** 只有图标的小按钮,边长 44. */
@Composable
fun SmallIconButton(label: String, icon: String, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    Box(
        Modifier.semantics { contentDescription = translate(label) }
            .tap(onClick = onClick)
            .size(SmallHeight)
            .background(c.surface, RoundedCornerShape(Radius.r2)),
        contentAlignment = Alignment.Center,
    ) {
        LineIcon(icon, 16.dp, c.text)
    }
}

/** 次要操作用文字按钮,和主按钮同宽,触控区 44. */
@Composable
fun TextButton(text: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    Box(Modifier.fillMaxWidth().tap(enabled, onClick = onClick).height(44.dp), contentAlignment = Alignment.Center) {
        Txt(text, weight = Medium, color = if (enabled) c.text2 else c.text3)
    }
}

/**
 * 长按确认.按下时调用 [onPress],松手或滑出按钮时调用 [onRelease],计时在流程层,[progress] 是流程给的真实进度.
 *
 * 底色从左向右填满,松手回退.这里只把流程的每一拍补平,不自己计时,系统关闭动画时直接跟着进度走.
 */
@Composable
fun HoldButton(
    text: String,
    progress: Float,
    enabled: Boolean,
    onPress: () -> Unit,
    onRelease: () -> Unit,
) {
    val c = BitOneTheme.colors
    val haptic = LocalHapticFeedback.current
    val shape = RoundedCornerShape(Radius.r2)
    var pressed by remember { mutableStateOf(false) }
    var running by remember { mutableStateOf(false) }
    val fill = remember { Animatable(0f) }
    LaunchedEffect(progress, pressed) {
        when {
            progress > 0f -> {
                running = true
                fill.animateTo(progress, tween(Motion.fast, easing = LinearEasing))
            }
            // 进度归零而手还按着,说明流程走完了.松手的路径会先把 pressed 清掉.
            pressed && running -> {
                running = false
                haptic.performHapticFeedback(HapticFeedbackType.Confirm)
                fill.snapTo(1f)
            }
            !pressed -> {
                running = false
                fill.animateTo(0f, stateSpec())
            }
        }
    }
    Box(
        Modifier.fillMaxWidth()
            .pressScale(pressed)
            .height(Size.hBtn)
            .clip(shape)
            .background(c.dangerSoft)
            .semantics { role = Role.Button }
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                awaitEachGesture {
                    awaitFirstDown()
                    pressed = true
                    onPress()
                    waitForUpOrCancellation()
                    pressed = false
                    onRelease()
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Txt(text, weight = Medium, color = c.danger)
        // 填色层盖在上面,只露出进度以内的部分,文字反色的分界线跟着走.
        Box(
            Modifier.matchParentSize()
                .clearAndSetSemantics {}
                .drawWithContent { clipRect(right = size.width * fill.value) { this@drawWithContent.drawContent() } }
                .background(c.danger),
            contentAlignment = Alignment.Center,
        ) {
            Txt(text, weight = Medium, color = c.bg)
        }
    }
}

/** 吸底操作区.边线在 Page,这里只管上下间距. */
@Composable
fun ActionArea(content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(top = Space.s3, bottom = Space.s4),
        verticalArrangement = Arrangement.spacedBy(Space.s2),
        content = content,
    )
}

enum class NoticeTone {
    Info,
    Warn,
    Danger,
}

/** 告示:用户要停下来看的事,和网页端的 Notice 同一规格.警告和错误读屏会立刻念出来. */
@Composable
fun Notice(text: String, tone: NoticeTone = NoticeTone.Info) {
    val c = BitOneTheme.colors
    val (plate, foreground, icon) =
        when (tone) {
            NoticeTone.Info -> Triple(c.surface2, c.text2, Icons.Unknown)
            NoticeTone.Warn -> Triple(c.warnSoft, c.warn, Icons.Alert)
            NoticeTone.Danger -> Triple(c.dangerSoft, c.danger, Icons.Alert)
        }
    Row(
        Modifier.fillMaxWidth()
            .background(plate, RoundedCornerShape(Radius.r2))
            .padding(horizontal = Space.s3, vertical = Space.s2)
            .semantics {
                liveRegion = if (tone == NoticeTone.Info) LiveRegionMode.Polite else LiveRegionMode.Assertive
            },
        horizontalArrangement = Arrangement.spacedBy(Space.s2),
    ) {
        LineIcon(icon, 16.dp, foreground, Modifier.padding(top = 2.dp))
        Txt(text, size = FontSize.t2, color = foreground)
    }
}

/** 高 24 的徽章. */
@Composable
fun Chip(text: String, background: Color, foreground: Color, icon: String? = null) {
    Row(
        Modifier.heightIn(min = 24.dp)
            .clip(RoundedCornerShape(Radius.rf))
            .background(background)
            .padding(horizontal = Space.s2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s1),
    ) {
        if (icon != null) LineIcon(icon, 12.dp, foreground)
        Txt(text, size = FontSize.t1, weight = Medium, color = foreground)
    }
}

/** 卡片和列表组,无边框,靠明度分层.列表组内部没有分隔线. */
@Composable
fun Card(
    modifier: Modifier = Modifier.fillMaxWidth(),
    color: Color = BitOneTheme.colors.surface,
    content: @Composable ColumnScope.() -> Unit,
) {
    val shape = RoundedCornerShape(Radius.r3)
    Column(
        modifier.elevation(BitOneTheme.elevation.elevation1, shape).clip(shape).background(color),
        content = content,
    )
}

/** 列表行,行高 56.可点时按下显示底色,选中时是 `accent-soft`.[inset] 让底色和弹层边缘留出距离,文字不动. */
@Composable
fun ListRow(
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    selected: Boolean = false,
    inset: Dp = 0.dp,
    content: @Composable RowScope.() -> Unit,
) {
    val c = BitOneTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val plate by
        animateColorAsState(
            when {
                selected -> c.accentSoft
                pressed -> c.surface2
                else -> c.surface2.copy(alpha = 0f)
            },
            stateSpec(Motion.fast),
            label = "row",
        )
    val base = if (onClick != null) modifier.tap(source = source, onClick = onClick) else modifier
    Row(
        base
            .fillMaxWidth()
            .padding(horizontal = inset)
            .heightIn(min = Size.hRow)
            .background(plate, RoundedCornerShape(Radius.r2))
            .padding(horizontal = Space.s4 - inset, vertical = Space.s1),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
        content = content,
    )
}

private val SwitchPad = 2.dp

/** 开关,圆点滑动,轨道变色. */
@Composable
fun SwitchTrack(on: Boolean) {
    val c = BitOneTheme.colors
    val track by animateColorAsState(if (on) c.accent else c.lineStrong, stateSpec(), label = "switch")
    val travel by
        animateDpAsState(if (on) Size.switchW - Size.switchDot - SwitchPad * 2 else 0.dp, stateSpec(), label = "thumb")
    Box(
        Modifier.width(Size.switchW)
            .height(Size.switchH)
            .background(track, RoundedCornerShape(Radius.rf))
            .padding(SwitchPad)
    ) {
        Box(
            Modifier.offset { IntOffset(travel.roundToPx(), 0) }
                .size(Size.switchDot)
                .background(c.onAccent, CircleShape)
        )
    }
}

/** 整行可点的开关. */
@Composable
fun SwitchRow(
    on: Boolean,
    onToggle: () -> Unit,
    content: @Composable RowScope.() -> Unit,
) {
    Row(
        Modifier.semantics { stateDescription = translate(if (on) "已开启" else "已关闭") }
            .tap(role = Role.Switch, onClick = onToggle)
            .fillMaxWidth()
            .heightIn(min = Size.hRow)
            .padding(horizontal = Space.s4, vertical = Space.s2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
    ) {
        content()
        SwitchTrack(on)
    }
}

/** 底色块在等宽的项目之间滑动,不是原地变色.底部导航和分段选择共用. */
@Composable
internal fun SlidingTrack(
    count: Int,
    selected: Int,
    modifier: Modifier,
    indicator: @Composable BoxScope.() -> Unit,
    items: @Composable RowScope.() -> Unit,
) {
    BoxWithConstraints(modifier) {
        val width = maxWidth / count
        val x by animateDpAsState(width * selected, stateSpec(), label = "track")
        Box(Modifier.offset { IntOffset(x.roundToPx(), 0) }.width(width).fillMaxHeight(), content = indicator)
        Row(Modifier.fillMaxSize(), content = items)
    }
}

/** 助记词长度切换. */
@Composable
fun <T> Segmented(options: List<Pair<T, String>>, selected: T, onSelect: (T) -> Unit) {
    val c = BitOneTheme.colors
    val inner = RoundedCornerShape(Radius.r1)
    // 选中块要比轨道亮.浅色里是 surface,深色里 surface 比轨道暗,换成 surface-3.
    val raised = if (c.surface.luminance() > c.surface2.luminance()) c.surface else c.surface3
    SlidingTrack(
        count = options.size,
        selected = options.indexOfFirst { it.first == selected }.coerceAtLeast(0),
        modifier =
            Modifier.fillMaxWidth()
                .height(Size.hInput)
                .background(c.surface2, RoundedCornerShape(Radius.r2))
                .padding(Space.s1),
        indicator = {
            Box(Modifier.fillMaxSize().elevation(BitOneTheme.elevation.elevation1, inner).background(raised, inner))
        },
    ) {
        options.forEach { (value, label) ->
            val foreground by
                animateColorAsState(if (value == selected) c.text else c.text3, stateSpec(Motion.fast), label = "item")
            Box(
                Modifier.weight(1f).fillMaxHeight().tap(role = Role.Tab) { onSelect(value) },
                contentAlignment = Alignment.Center,
            ) {
                Txt(label, size = FontSize.t2, weight = Medium, color = foreground)
            }
        }
    }
}

private val BackTouch = 44.dp
private val BackIcon = 24.dp

/** 图标在 24 网格里的可见左右边缘,含一半线宽. */
private fun glyphEdges(icon: String): Pair<Dp, Dp> = if (icon == Icons.Close) 5.1.dp to 18.9.dp else 8.1.dp to 15.9.dp

/**
 * 返回或关闭按钮.触控区 44,图标的可见左边缘落在边线上,占位只算图标本身,标题紧随其后.
 *
 * 相机取景上用 [tint] 换成白色.
 */
@Composable
fun BackButton(onClick: () -> Unit, icon: String = Icons.Back, tint: Color = BitOneTheme.colors.text) {
    val (left, right) = glyphEdges(icon)
    val iconStart = (BackTouch - BackIcon) / 2
    Box(
        Modifier.layout { measurable, constraints ->
                val placeable = measurable.measure(constraints)
                val advance = (right - left + Space.s3).roundToPx()
                layout(advance, placeable.height) { placeable.place(-(iconStart + left).roundToPx(), 0) }
            }
            .semantics {
                role = Role.Button
                contentDescription = translate(if (icon == Icons.Close) "关闭" else "返回")
            }
            .tap(onClick = onClick)
            .size(BackTouch),
        contentAlignment = Alignment.Center,
    ) {
        LineIcon(icon, BackIcon, tint)
    }
}

/** 弹层标题右边的关闭按钮.触控区 44,图标的可见右边缘落在边线上. */
@Composable
fun CloseButton(onClick: () -> Unit) {
    val (_, right) = glyphEdges(Icons.Close)
    Box(
        Modifier.offset(x = (BackTouch + BackIcon) / 2 - right)
            .semantics {
                role = Role.Button
                contentDescription = translate("关闭")
            }
            .tap(onClick = onClick)
            .size(BackTouch),
        contentAlignment = Alignment.Center,
    ) {
        LineIcon(Icons.Close, BackIcon, BitOneTheme.colors.text)
    }
}

private val StepDot = Space.s5
private const val STEP_COUNT = 3

/**
 * 流程步骤:数字圆点加短连线,走过的换成对勾,和网页端的转账步骤同一规格.放在标题栏右侧,步骤名就是页面标题.
 *
 * 进入这一步时,通向它的连线填上去.
 */
@Composable
fun Steps(current: Int) {
    val c = BitOneTheme.colors
    Row(
        Modifier.semantics { contentDescription = translate("第 $current 步,共 $STEP_COUNT 步") },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s2),
    ) {
        for (step in 1..STEP_COUNT) {
            if (step > 1) StepLine(reached = step <= current, fresh = step == current)
            val done = step < current
            Box(
                Modifier.size(StepDot)
                    .background(
                        when {
                            step == current -> c.accent
                            done -> c.accentSoft
                            else -> c.surface3
                        },
                        CircleShape,
                    ),
                contentAlignment = Alignment.Center,
            ) {
                if (done) {
                    LineIcon(Icons.Check, 12.dp, c.accentText)
                } else {
                    Txt(
                        step.toString(),
                        size = FontSize.t1,
                        weight = Medium,
                        color = if (step == current) c.onAccent else c.text3,
                    )
                }
            }
        }
    }
}

@Composable
private fun StepLine(reached: Boolean, fresh: Boolean) {
    val c = BitOneTheme.colors
    val fill = remember { Animatable(if (reached && !fresh) 1f else 0f) }
    LaunchedEffect(reached) { if (reached) fill.animateTo(1f, tween(Motion.slow, easing = Motion.emphasizedEasing)) }
    val shape = RoundedCornerShape(Radius.rf)
    Box(Modifier.width(Space.s4).height(2.dp).clip(shape).background(c.surface3)) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(fill.value).background(c.accent, shape))
    }
}

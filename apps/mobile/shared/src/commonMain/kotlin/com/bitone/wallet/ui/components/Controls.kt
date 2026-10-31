package com.bitone.wallet.ui.components

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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import com.bitone.wallet.i18n.translate
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Size
import com.bitone.wallet.ui.theme.Space
import com.bitone.wallet.ui.theme.elevation

/**
 * 可点击区域，无水波，按下时整体缩放。放在修饰链的最前面，缩放才能带上后面的底色和圆角。
 *
 * 全部按钮和可点的行都走这里，按压反馈全局一致。
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
                tone == Tone.Danger -> c.danger
                // 次级按钮大多放在 bg 上，surface-2 和 bg 太接近，看不出是按钮。
                else -> if (pressed) c.surface2 else c.surface
            },
            stateSpec(Motion.fast),
            label = "button",
        )
    val foreground =
        when {
            !enabled -> c.text3
            tone == Tone.Primary -> c.onAccent
            tone == Tone.Danger -> c.bg
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
fun PrimaryButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    onClick: () -> Unit,
) = ActionButton(text, Tone.Primary, enabled, modifier, onClick)

@Composable
fun SecondaryButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    onClick: () -> Unit,
) = ActionButton(text, Tone.Secondary, enabled, modifier, onClick)

@Composable
fun DangerButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    onClick: () -> Unit,
) = ActionButton(text, Tone.Danger, enabled, modifier, onClick)

/** 次要操作用文字按钮，和主按钮同宽，触控区 44。 */
@Composable
fun TextButton(
    text: String,
    modifier: Modifier = Modifier.fillMaxWidth(),
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    val c = BitOneTheme.colors
    Box(modifier.tap(enabled, onClick = onClick).height(44.dp), contentAlignment = Alignment.Center) {
        Txt(text, weight = Medium, color = if (enabled) c.text2 else c.text3)
    }
}

/**
 * 长按确认。按下时调用 [onPress]，松手或滑出按钮时调用 [onRelease]，计时在流程层，[progress] 是流程给的真实进度。
 *
 * 底色从左向右填满，松手回退。这里只把流程的每一拍补平，不自己计时，系统关闭动画时直接跟着进度走。
 */
@Composable
fun HoldButton(
    text: String,
    progress: Float,
    enabled: Boolean,
    onPress: () -> Unit,
    onRelease: () -> Unit,
    modifier: Modifier = Modifier.fillMaxWidth(),
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
            // 进度归零而手还按着，说明流程走完了。松手的路径会先把 pressed 清掉。
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
        modifier
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
        // 填色层盖在上面，只露出进度以内的部分，文字反色的分界线跟着走。
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

/** 吸底操作区。边线在 Page，这里只管上下间距。 */
@Composable
fun ActionArea(content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(top = Space.s3, bottom = Space.s4),
        verticalArrangement = Arrangement.spacedBy(Space.s2),
        content = content,
    )
}

/** 高 24 的徽章。 */
@Composable
fun Chip(text: String, background: Color, foreground: Color, mono: Boolean = false, icon: String? = null) {
    Row(
        Modifier.height(24.dp)
            .clip(RoundedCornerShape(Radius.rf))
            .background(background)
            .padding(horizontal = Space.s2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s1),
    ) {
        if (icon != null) LineIcon(icon, 12.dp, foreground, strokeWidth = 2.2f)
        Txt(text, size = FontSize.t1, weight = Medium, color = foreground, mono = mono)
    }
}

@Composable
private fun Surface(radius: Dp, color: Color, modifier: Modifier, content: @Composable ColumnScope.() -> Unit) {
    val shape = RoundedCornerShape(radius)
    Column(
        modifier.elevation(BitOneTheme.elevation.elevation1, shape).clip(shape).background(color),
        content = content,
    )
}

/** 卡片，无边框。浅色靠柔和的阴影，深色靠明度。 */
@Composable
fun Card(
    modifier: Modifier = Modifier.fillMaxWidth(),
    color: Color = BitOneTheme.colors.surface,
    content: @Composable ColumnScope.() -> Unit,
) = Surface(Radius.r4, color, modifier, content)

/** 列表组，内部没有分隔线。 */
@Composable
fun ListGroup(modifier: Modifier = Modifier.fillMaxWidth(), content: @Composable ColumnScope.() -> Unit) =
    Surface(Radius.r3, BitOneTheme.colors.surface, modifier, content)

/** 列表行，行高 56。可点时按下显示底色。 */
@Composable
fun ListRow(
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    content: @Composable RowScope.() -> Unit,
) {
    val c = BitOneTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val plate by
        animateColorAsState(
            if (pressed) c.surface2 else c.surface2.copy(alpha = 0f),
            stateSpec(Motion.fast),
            label = "row",
        )
    val base = if (onClick != null) modifier.tap(source = source, onClick = onClick) else modifier
    Row(
        base
            .fillMaxWidth()
            .heightIn(min = Size.hRow)
            .background(plate)
            .padding(horizontal = Space.s4, vertical = Space.s1),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
        content = content,
    )
}

private val TrackWidth = 48.dp
private val TrackHeight = 28.dp
private val ThumbSize = 20.dp

/** 开关，圆点滑动，轨道变色。 */
@Composable
fun SwitchTrack(on: Boolean) {
    val c = BitOneTheme.colors
    val track by animateColorAsState(if (on) c.accent else c.lineStrong, stateSpec(), label = "switch")
    val travel by
        animateDpAsState(if (on) TrackWidth - ThumbSize - Space.s1 * 2 else 0.dp, stateSpec(), label = "thumb")
    Box(
        Modifier.width(TrackWidth)
            .height(TrackHeight)
            .background(track, RoundedCornerShape(Radius.rf))
            .padding(Space.s1)
    ) {
        Box(Modifier.offset { IntOffset(travel.roundToPx(), 0) }.size(ThumbSize).background(c.onAccent, CircleShape))
    }
}

/** 整行可点的开关。 */
@Composable
fun SwitchRow(
    on: Boolean,
    onToggle: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable RowScope.() -> Unit,
) {
    Row(
        modifier
            .semantics { stateDescription = translate(if (on) "已开启" else "已关闭") }
            .tap(role = Role.Switch, onClick = onToggle)
            .fillMaxWidth()
            .heightIn(min = Size.hRow)
            .padding(horizontal = Space.s4),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
    ) {
        content()
        SwitchTrack(on)
    }
}

/** 底色块在等宽的项目之间滑动，不是原地变色。底部导航和分段选择共用。 */
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

/** 助记词长度切换。 */
@Composable
fun <T> Segmented(options: List<Pair<T, String>>, selected: T, onSelect: (T) -> Unit, modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    val inner = RoundedCornerShape(Radius.r1)
    // 选中块要比轨道亮。浅色里是 surface，深色里 surface 比轨道暗，换成 surface-3。
    val raised = if (c.surface.luminance() > c.surface2.luminance()) c.surface else c.surface3
    SlidingTrack(
        count = options.size,
        selected = options.indexOfFirst { it.first == selected }.coerceAtLeast(0),
        modifier =
            modifier
                .fillMaxWidth()
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

/** 图标在 24 网格里的可见左右边缘，含一半线宽。 */
private fun glyphEdges(icon: String): Pair<Dp, Dp> = if (icon == Icons.Close) 5.1.dp to 18.9.dp else 8.1.dp to 15.9.dp

/**
 * 返回或关闭按钮。触控区 44，图标的可见左边缘落在边线上，占位只算图标本身，标题紧随其后。
 *
 * 相机取景上用 [tint] 换成白色。
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

/** 标题栏右边的图标按钮。触控区 44，图标的可见右边缘落在边线上。 */
@Composable
fun HeaderIconButton(label: String, icon: String, onClick: () -> Unit) {
    val (_, right) = glyphEdges(icon)
    Box(
        Modifier.offset(x = (BackTouch + BackIcon) / 2 - right)
            .semantics {
                role = Role.Button
                contentDescription = label
            }
            .tap(onClick = onClick)
            .size(BackTouch),
        contentAlignment = Alignment.Center,
    ) {
        LineIcon(icon, BackIcon, BitOneTheme.colors.text)
    }
}

/** 流程步骤标记，如 1/3。 */
@Composable
fun StepChip(text: String) {
    val c = BitOneTheme.colors
    Chip(text, c.accentSoft, c.accentText)
}

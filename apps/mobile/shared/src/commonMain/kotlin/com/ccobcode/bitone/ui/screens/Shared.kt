package com.ccobcode.bitone.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.keyframes
import androidx.compose.animation.core.spring
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.PIN_LENGTH
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.i18n.translate
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.AddressChunks
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.Skeleton
import com.ccobcode.bitone.ui.components.SwitchRow
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.WordText
import com.ccobcode.bitone.ui.components.exitSpec
import com.ccobcode.bitone.ui.components.stateSpec
import com.ccobcode.bitone.ui.components.tap
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Space

private val DotSize = 12.dp
private const val DOT_POP_FROM = 0.5f
private const val DOT_POP_DAMPING = 0.4f
private const val DOT_POP_STIFFNESS = 1000f
private const val SHAKE_OFFSET = 8f

/** 超过这个长度的等宽值标签和值分两行，右对齐的长地址折行后不好核对。 */
private const val STACK_LENGTH = 26

/** 展示二维码和扫码的页面保持常亮。[maximize] 只管亮度，关掉自动亮度后屏幕照样不灭。 */
@Composable
internal fun KeepScreenOn(brightness: Brightness, maximize: Boolean) {
    DisposableEffect(maximize) {
        brightness.hold(maximize)
        onDispose { brightness.restore() }
    }
}

/** 二维码页面用：常亮，并按设置调到最高亮度。 */
@Composable
internal fun ShowingQr(flow: WalletFlow, brightness: Brightness) = KeepScreenOn(brightness, flow.autoBrightness)

/** PIN 圆点。填入时弹一下，[errors] 增加时左右抖动并短促震动。 */
@Composable
internal fun PinDots(length: Int, errors: Int = 0) {
    val haptic = LocalHapticFeedback.current
    val shake = remember { Animatable(0f) }
    var seen by remember { mutableIntStateOf(errors) }
    LaunchedEffect(errors) {
        val wrong = errors > seen
        seen = errors
        if (wrong) {
            haptic.performHapticFeedback(HapticFeedbackType.Reject)
            shake.animateTo(
                0f,
                keyframes {
                    durationMillis = Motion.slow
                    -SHAKE_OFFSET at Motion.slow / 6
                    SHAKE_OFFSET at Motion.slow * 2 / 6
                    -SHAKE_OFFSET / 2 at Motion.slow * 3 / 6
                    SHAKE_OFFSET / 2 at Motion.slow * 4 / 6
                    -SHAKE_OFFSET / 4 at Motion.slow * 5 / 6
                },
            )
        }
    }
    Row(
        Modifier.graphicsLayer { translationX = shake.value.dp.toPx() }
            .semantics { contentDescription = translate("已输入 $length 位") },
        horizontalArrangement = Arrangement.spacedBy(Space.s4),
    ) {
        repeat(PIN_LENGTH) { index -> PinDot(index < length, shake.isRunning) }
    }
}

@Composable
private fun PinDot(filled: Boolean, error: Boolean) {
    val c = BitOneTheme.colors
    val scale = remember { Animatable(1f) }
    LaunchedEffect(filled) {
        if (filled) {
            scale.snapTo(DOT_POP_FROM)
            scale.animateTo(1f, spring(DOT_POP_DAMPING, DOT_POP_STIFFNESS))
        }
    }
    val fill by
        animateColorAsState(
            when {
                error -> c.danger
                filled -> c.accent
                else -> c.lineStrong
            },
            stateSpec(Motion.fast),
            label = "dot",
        )
    Box(
        Modifier.size(DotSize)
            .graphicsLayer {
                scaleX = scale.value
                scaleY = scale.value
            }
            .background(fill, CircleShape)
    )
}

/** 助记词格子。整组放在一张卡片上，格子本身没有边框，A1b 用 [highlight] 标出当前位。 */
@Composable
internal fun WordCell(
    number: Int,
    word: String,
    height: Dp,
    modifier: Modifier = Modifier,
    highlight: Color = Color.Transparent,
    wordColor: Color = BitOneTheme.colors.text,
    loading: Boolean = false,
    onClick: (() -> Unit)? = null,
) {
    val c = BitOneTheme.colors
    val plate by animateColorAsState(highlight, stateSpec(Motion.fast), label = "cell")
    val base = if (onClick != null) modifier.tap(onClick = onClick) else modifier
    Row(
        base.height(height).background(plate, RoundedCornerShape(Radius.r1)).padding(horizontal = Space.s2),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s2),
    ) {
        Txt(number.toString().padStart(2, '0'), size = FontSize.t1, color = c.text3)
        if (loading) {
            Skeleton(Modifier.weight(1f).height(Space.s3))
        } else {
            WordText(word, wordColor, Modifier.weight(1f))
        }
    }
}

/** 助记词网格，三列，无边框。 */
@Composable
internal fun WordGrid(words: List<String>, rowHeight: Dp, loading: Boolean = false) {
    Card {
        Column(Modifier.padding(Space.s3)) {
            ThreeColumns(words) { index, word, modifier ->
                WordCell(index + 1, word, rowHeight, modifier, loading = loading)
            }
        }
    }
}

/** A2 和 A1b 共用的 BIP39 密码开关。 */
@Composable
internal fun PassphraseSwitch(on: Boolean, onToggle: () -> Unit) {
    val c = BitOneTheme.colors
    Column(Modifier.fillMaxWidth()) {
        Card {
            SwitchRow(on, onToggle) {
                Txt("BIP39 密码", Modifier.weight(1f))
                Chip("可选", c.surface2, c.text2)
            }
        }
        AnimatedVisibility(
            on,
            enter = fadeIn(stateSpec()) + expandVertically(stateSpec()),
            exit = fadeOut(exitSpec()) + shrinkVertically(exitSpec()),
        ) {
            Txt("密码不写入设备，导出和签名时都要输入。", Modifier.padding(top = Space.s2), size = FontSize.t2, color = c.text3)
        }
    }
}

/** 三列排布。 */
@Composable
internal fun <T> ThreeColumns(items: List<T>, cell: @Composable (Int, T, Modifier) -> Unit) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s1)) {
        items.chunked(3).forEachIndexed { row, chunk ->
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s1)) {
                chunk.forEachIndexed { column, item -> cell(row * 3 + column, item, Modifier.weight(1f)) }
            }
        }
    }
}

/** 详情的一行。标签 text-3，值 text，都比主内容小一号，存在感低但一项不少。长的地址和哈希换到下一行，4 个字符一组。 */
@Composable
internal fun DetailRow(label: String, value: String, mono: Boolean = false) {
    val c = BitOneTheme.colors
    val whole = value.none { it.isWhitespace() }
    if (mono && value.length > STACK_LENGTH) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s1)) {
            Txt(label, size = FontSize.t2, color = c.text3)
            if (whole) {
                AddressChunks(value, size = FontSize.t2)
            } else {
                Txt(value, size = FontSize.t2, mono = true)
            }
        }
    } else {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s4)) {
            Txt(label, size = FontSize.t2, color = c.text3)
            Txt(value, Modifier.weight(1f), size = FontSize.t2, mono = mono, align = TextAlign.End)
        }
    }
}

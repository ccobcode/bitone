package com.ccobcode.bitone.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.i18n.translate
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Space

// 内置键盘。助记词、PIN、BIP39 密码都不走系统输入法，全应用没有文本输入框。

private val KeySize = 72.dp
private val ShortKeySize = 64.dp
private val KeyGapX = Space.s6
private val KeyGapY = Space.s4

/** 屏幕矮于这个高度时按键缩小，间距按比例缩小，见界面规范 §4.7。 */
private val ShortWindow = 700.dp
private val LetterHeight = 44.dp

/** 半个按键的缩进，第二、三行字母键和标准键盘一样错开。 */
private const val HALF_KEY = 0.5f
private const val WIDE_KEY = 1.5f

/**
 * PIN 键盘，A2c、A7、修改 PIN、解锁共用。
 *
 * 整块水平居中，不贴屏幕两边，距底部安全区 64，矮屏 32，拇指不用压到屏幕下沿。[action] 不为空时显示在左下角的空位，输入过程中布局不跳动。
 */
@Composable
fun PinPad(
    enabled: Boolean,
    onDigit: (Char) -> Unit,
    onDelete: () -> Unit,
    action: String? = null,
    actionEnabled: Boolean = true,
    onAction: () -> Unit = {},
) {
    val alpha by animateFloatAsState(if (enabled) 1f else 0.4f, stateSpec(), label = "pad")
    val windowHeight = with(LocalDensity.current) { LocalWindowInfo.current.containerSize.height.toDp() }
    val short = windowHeight < ShortWindow
    val key = if (short) ShortKeySize else KeySize
    val ratio = key / KeySize
    Box(
        Modifier.fillMaxWidth().padding(top = Space.s2, bottom = if (short) Space.s8 else Space.s16),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier.width(key * 3 + KeyGapX * ratio * 2).alpha(alpha),
            verticalArrangement = Arrangement.spacedBy(KeyGapY * ratio),
        ) {
            listOf("123", "456", "789").forEach { row ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    row.forEach { digit -> DigitKey(digit, key, enabled, onDigit) }
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                ActionKey(action, key, actionEnabled, onAction)
                DigitKey('0', key, enabled, onDigit)
                Box(
                    Modifier.semantics { contentDescription = translate("删除") }
                        .tap(enabled, onClick = onDelete)
                        .size(key),
                    contentAlignment = Alignment.Center,
                ) {
                    LineIcon(Icons.Delete, 24.dp, BitOneTheme.colors.text)
                }
            }
        }
    }
}

@Composable
private fun DigitKey(digit: Char, size: Dp, enabled: Boolean, onDigit: (Char) -> Unit) {
    val c = BitOneTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(if (pressed) c.surface2 else c.surface, stateSpec(Motion.fast), label = "key")
    Box(
        Modifier.tap(enabled, source = source) { onDigit(digit) }.size(size).background(fill, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Txt(digit.toString(), size = FontSize.t7)
    }
}

@Composable
private fun ActionKey(label: String?, size: Dp, enabled: Boolean, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    val shown = rememberLatest(label)
    Box(Modifier.size(size)) {
        AnimatedVisibility(label != null, enter = fadeIn(enterSpec()), exit = fadeOut(exitSpec(Motion.fast))) {
            Box(
                Modifier.tap(enabled, onClick = onClick)
                    .size(size)
                    .background(if (enabled) c.accent else c.surface3, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Txt(shown.orEmpty(), size = FontSize.t2, weight = Medium, color = if (enabled) c.onAccent else c.text3)
            }
        }
    }
}

@Composable
private fun LetterKey(
    label: String,
    modifier: Modifier,
    background: Color = BitOneTheme.colors.surface,
    onClick: () -> Unit,
) {
    val c = BitOneTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val fill by animateColorAsState(if (pressed) c.surface3 else background, stateSpec(Motion.fast), label = "key")
    Box(
        modifier
            .tap(source = source, onClick = onClick)
            .height(LetterHeight)
            .background(fill, RoundedCornerShape(Radius.r1)),
        contentAlignment = Alignment.Center,
    ) {
        Txt(label)
    }
}

@Composable
private fun DeleteKey(modifier: Modifier, onDelete: () -> Unit) {
    val c = BitOneTheme.colors
    Box(
        modifier
            .semantics { contentDescription = translate("删除") }
            .tap(onClick = onDelete)
            .height(LetterHeight)
            .background(c.surface3, RoundedCornerShape(Radius.r1)),
        contentAlignment = Alignment.Center,
    ) {
        LineIcon(Icons.Delete, 22.dp, c.text)
    }
}

@Composable
private fun KeyRow(content: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s1), content = content)
}

/** 助记词键盘，只有小写字母。左右贴着边线。 */
@Composable
fun WordKeyboard(onLetter: (Char) -> Unit, onDelete: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        KeyRow { "qwertyuiop".forEach { LetterKey(it.toString(), Modifier.weight(1f)) { onLetter(it) } } }
        KeyRow {
            Spacer(Modifier.weight(HALF_KEY))
            "asdfghjkl".forEach { LetterKey(it.toString(), Modifier.weight(1f)) { onLetter(it) } }
            Spacer(Modifier.weight(HALF_KEY))
        }
        KeyRow {
            Spacer(Modifier.weight(WIDE_KEY))
            "zxcvbnm".forEach { LetterKey(it.toString(), Modifier.weight(1f)) { onLetter(it) } }
            DeleteKey(Modifier.weight(WIDE_KEY), onDelete)
        }
    }
}

private val SymbolRows = listOf("1234567890", "!@#$%^&*()", "-_=+[]{}:\"", ",.?/\\|~`'<>")

/** BIP39 密码键盘。密码可以含大小写、数字和符号，只有小写的键盘不够用。只支持 ASCII。 */
@Composable
fun PassphraseKeyboard(onChar: (Char) -> Unit, onDelete: () -> Unit, onDone: () -> Unit) {
    var upper by remember { mutableStateOf(false) }
    var symbols by remember { mutableStateOf(false) }
    val c = BitOneTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        if (symbols) {
            SymbolRows.forEach { row ->
                KeyRow { row.forEach { LetterKey(it.toString(), Modifier.weight(1f)) { onChar(it) } } }
            }
        } else {
            listOf("qwertyuiop", "asdfghjkl", "zxcvbnm").forEachIndexed { index, row ->
                KeyRow {
                    if (index > 0) Spacer(Modifier.weight(if (index == 1) HALF_KEY else WIDE_KEY))
                    row.forEach {
                        val char = if (upper) it.uppercaseChar() else it
                        LetterKey(char.toString(), Modifier.weight(1f)) { onChar(char) }
                    }
                    if (index > 0) Spacer(Modifier.weight(if (index == 1) HALF_KEY else WIDE_KEY))
                }
            }
        }
        KeyRow {
            if (!symbols) {
                LetterKey(if (upper) "abc" else "ABC", Modifier.weight(WIDE_KEY), c.surface3) { upper = !upper }
            }
            LetterKey(if (symbols) "abc" else "#+=", Modifier.weight(WIDE_KEY), c.surface3) { symbols = !symbols }
            LetterKey("空格", Modifier.weight(3f)) { onChar(' ') }
            DeleteKey(Modifier.weight(WIDE_KEY), onDelete)
            LetterKey("完成", Modifier.weight(WIDE_KEY), c.accentSoft, onDone)
        }
    }
}

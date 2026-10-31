package com.bitone.wallet.ui.components

import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import com.bitone.wallet.i18n.translate
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize

// 字重只用 400、500、600。标题 600，正文和标签 400，强调 500。
val Medium = FontWeight.W500
val SemiBold = FontWeight.W600

private const val BODY_LINE = 1.4f
private const val TITLE_LINE = 1.25f
private val TightFrom = FontSize.t6

/** 24 及以上字距收紧 1%。行高固定比例，文字在行内垂直居中。 */
internal fun textStyle(
    size: TextUnit,
    weight: FontWeight,
    color: Color,
    family: FontFamily,
    align: TextAlign = TextAlign.Start,
    lineHeight: Float? = null,
): TextStyle {
    val tight = size.value >= TightFrom.value
    return TextStyle(
        color = color,
        fontSize = size,
        fontWeight = weight,
        fontFamily = family,
        textAlign = align,
        letterSpacing = if (tight) (-0.01).em else TextUnit.Unspecified,
        lineHeight = size * (lineHeight ?: if (tight) TITLE_LINE else BODY_LINE),
        lineHeightStyle = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.None),
    )
}

/** 等宽字体只用在地址、哈希、路径上，金额和普通数字都用 Inter，字体文件里的数字已经是等宽的。 */
@Composable
fun Txt(
    text: String,
    modifier: Modifier = Modifier,
    size: TextUnit = FontSize.t3,
    weight: FontWeight = FontWeight.Normal,
    color: Color = BitOneTheme.colors.text,
    mono: Boolean = false,
    align: TextAlign = TextAlign.Start,
    lineHeight: Float? = null,
) {
    val fonts = BitOneTheme.fonts
    BasicText(
        text = translate(text),
        modifier = modifier,
        style = textStyle(size, weight, color, if (mono) fonts.mono else fonts.sans, align, lineHeight),
    )
}

/** 助记词不能被截断，放不下时缩小字号。 */
@Composable
fun WordText(word: String, color: Color, modifier: Modifier = Modifier) {
    BasicText(
        text = word,
        modifier = modifier,
        style = TextStyle(color = color, fontWeight = Medium, fontFamily = BitOneTheme.fonts.sans),
        maxLines = 1,
        softWrap = false,
        overflow = TextOverflow.Clip,
        autoSize = TextAutoSize.StepBased(minFontSize = 10.sp, maxFontSize = FontSize.t3),
    )
}

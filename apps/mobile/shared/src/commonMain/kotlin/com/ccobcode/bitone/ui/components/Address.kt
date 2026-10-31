package com.ccobcode.bitone.ui.components

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import com.ccobcode.bitone.flow.addressChunks
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space

/** 完整地址,4 个字符一组.奇数组用 text,偶数组用 text-2,不加底色块.同一个文本节点,读屏时不会被拆开. */
@Composable
fun AddressChunks(
    address: String,
    size: TextUnit = FontSize.t4,
    minLines: Int = 1,
) {
    val c = BitOneTheme.colors
    val odd = c.text
    val even = c.text2
    val text =
        remember(address, odd, even) {
            buildAnnotatedString {
                addressChunks(address).forEachIndexed { index, chunk ->
                    if (index > 0) append(' ')
                    withStyle(SpanStyle(color = if (index % 2 == 0) odd else even)) { append(chunk) }
                }
            }
        }
    BasicText(
        text,
        Modifier.fillMaxWidth(),
        style =
            textStyle(
                size,
                FontWeight.Normal,
                odd,
                BitOneTheme.fonts.mono,
                lineHeight = (lineHeightOf(size).value + Space.s1.value).sp,
            ),
        minLines = minLines,
    )
}

/** 地址卡片.[tint] 用来突出新控制方这类地址,不用边框. */
@Composable
fun AddressCard(
    label: String,
    address: String,
    tint: Color = BitOneTheme.colors.surface,
    leading: @Composable (() -> Unit)? = null,
    badge: @Composable (() -> Unit)? = null,
) {
    Card(color = tint) {
        Column(Modifier.padding(Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            Row(
                Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.s2),
            ) {
                leading?.invoke()
                Txt(label, Modifier.weight(1f), size = FontSize.t2, color = BitOneTheme.colors.text3)
                badge?.invoke()
            }
            AddressChunks(address)
        }
    }
}

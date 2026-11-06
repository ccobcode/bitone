package com.ccobcode.bitone.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.Dp
import com.ccobcode.bitone.ui.theme.DarkColors

/** 品牌标志的深色主版,160 网格.图标不随主题变化. */
@Composable
fun BrandMark(size: Dp) {
    Canvas(Modifier.size(size)) {
        val ratio = this.size.width / 160f
        scale(ratio, pivot = Offset.Zero) {
            drawRoundRect(DarkColors.bg, size = Size(160f, 160f), cornerRadius = CornerRadius(36f))
            for (x in listOf(44f, 116f)) {
                drawLine(DarkColors.text, Offset(x, 52f), Offset(x, 108f), strokeWidth = 16f, cap = StrokeCap.Round)
            }
            drawRoundRect(
                DarkColors.accent,
                topLeft = Offset(62f, 62f),
                size = Size(36f, 36f),
                cornerRadius = CornerRadius(9f),
            )
        }
    }
}

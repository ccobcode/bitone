package com.bitone.wallet.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import com.bitone.wallet.ui.theme.DarkColors

private const val FULL =
    "M56 46H48A10 10 0 0 0 38 56V104A10 10 0 0 0 48 114H56M104 46H112A10 10 0 0 1 122 56V104A10 10 0 0 1 112 114H104"

/** 品牌标志的深色主版，160 网格。图标不随主题变化。 */
@Composable
fun BrandMark(size: Dp) {
    val brackets = remember { PathParser().parsePathString(FULL).toPath() }
    Canvas(Modifier.size(size)) {
        val ratio = this.size.width / 160f
        scale(ratio, pivot = Offset.Zero) {
            drawRoundRect(DarkColors.bg, size = Size(160f, 160f), cornerRadius = CornerRadius(36f))
            drawPath(brackets, DarkColors.text, style = Stroke(12f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            drawRoundRect(
                DarkColors.accent,
                topLeft = Offset(66f, 66f),
                size = Size(28f, 28f),
                cornerRadius = CornerRadius(7f),
            )
        }
    }
}

package com.ccobcode.bitone.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp

private const val STROKE_WIDTH = 1.8f

/** 线宽按 24 网格计算,随图标尺寸缩放. */
@Composable
fun LineIcon(path: String, size: Dp, color: Color, modifier: Modifier = Modifier) {
    val parsed = remember(path) { PathParser().parsePathString(path).toPath() }
    Canvas(modifier.size(size)) {
        val ratio = this.size.width / 24f
        scale(ratio, pivot = Offset.Zero) {
            drawPath(parsed, color, style = Stroke(STROKE_WIDTH, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

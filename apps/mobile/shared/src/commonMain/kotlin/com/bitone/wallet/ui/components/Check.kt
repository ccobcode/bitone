package com.bitone.wallet.ui.components

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import com.bitone.wallet.ui.theme.Motion

private const val ICON_RATIO = 0.58f

/** 完成时的对勾，描线画出来，时长 `slow`。圆底 [background]，线色 [color]。 */
@Composable
fun DrawnCheck(size: Dp, color: Color, background: Color, strokeWidth: Float = 2.6f, modifier: Modifier = Modifier) {
    val check = remember { PathParser().parsePathString(Icons.Check).toPath() }
    val measure = remember { PathMeasure().apply { setPath(check, false) } }
    val progress = remember { Animatable(0f) }
    LaunchedEffect(Unit) { progress.animateTo(1f, stateSpec(Motion.slow)) }
    Box(modifier.size(size).background(background, CircleShape), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(size * ICON_RATIO)) {
            val drawn = Path()
            measure.getSegment(0f, measure.length * progress.value, drawn, true)
            scale(this.size.width / 24f, pivot = Offset.Zero) {
                drawPath(drawn, color, style = Stroke(strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}

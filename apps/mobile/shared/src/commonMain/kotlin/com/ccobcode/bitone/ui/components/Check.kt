package com.ccobcode.bitone.ui.components

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
import androidx.compose.ui.draw.drawBehind
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
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Space

private const val ICON_RATIO = 0.58f
private const val RING_ALPHA = 0.5f

/** 完成时的对勾，描线画出来，时长 `slow`，外扩一圈 [ring] 色的光，和网页端的 `ring-once` 相同。圆底 [background]，线色 [color]。 */
@Composable
fun DrawnCheck(
    size: Dp,
    color: Color,
    background: Color,
    ring: Color = color,
    strokeWidth: Float = 2.6f,
) {
    val check = remember { PathParser().parsePathString(Icons.Check).toPath() }
    val measure = remember { PathMeasure().apply { setPath(check, false) } }
    val progress = remember { Animatable(0f) }
    val spread = remember { Animatable(0f) }
    LaunchedEffect(Unit) { progress.animateTo(1f, stateSpec(Motion.slow)) }
    LaunchedEffect(Unit) { spread.animateTo(1f, stateSpec(Motion.slow * 2)) }
    Box(
        Modifier.size(size)
            .drawBehind {
                drawCircle(
                    ring.copy(alpha = RING_ALPHA * (1f - spread.value)),
                    radius = this.size.minDimension / 2 + Space.s2.toPx() * spread.value,
                )
            }
            .background(background, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(size * ICON_RATIO)) {
            val drawn = Path()
            measure.getSegment(0f, measure.length * progress.value, drawn, true)
            scale(this.size.width / 24f, pivot = Offset.Zero) {
                drawPath(drawn, color, style = Stroke(strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}

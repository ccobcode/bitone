package com.ccobcode.bitone.ui.screens

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.BackButton
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.contentColumn
import com.ccobcode.bitone.ui.components.stateSpec
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size as TokenSize
import com.ccobcode.bitone.ui.theme.Space

private val SweepInset = 24.dp
private const val HINT_ALPHA = 0.78f

/** A5 扫描。全屏相机，永远是深色。只有关闭按钮、取景框和一句提示，收齐后短促震动并直接进入下一页。扫码时对着屏幕，常亮，不调亮度。 */
@Composable
fun ScanScreen(flow: WalletFlow, brightness: Brightness) {
    KeepScreenOn(brightness, maximize = false)
    val c = BitOneTheme.colors
    val haptic = LocalHapticFeedback.current
    val done = flow.scanProgress >= 1f
    LaunchedEffect(done) { if (done) haptic.performHapticFeedback(HapticFeedbackType.Confirm) }
    Box(Modifier.fillMaxSize().background(c.camera)) {
        flow.scanner.Viewfinder(Modifier.fillMaxSize(), flow::onScanned, flow::cameraFailed)
        Column(
            Modifier.fillMaxSize()
                .windowInsetsPadding(
                    WindowInsets.statusBars.union(WindowInsets.displayCutout.only(WindowInsetsSides.Top))
                )
                .navigationBarsPadding(),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                Modifier.contentColumn().height(TokenSize.hRow).padding(horizontal = TokenSize.gutter),
                contentAlignment = Alignment.CenterStart,
            ) {
                BackButton({ flow.back() }, Icons.Close, c.onAccent)
            }
            Column(
                Modifier.weight(1f).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(Space.s6, Alignment.CenterVertically),
            ) {
                ScanFrame(done)
                ScanStatus(flow.scanFrames, flow.scanProgress)
            }
        }
    }
}

@Composable
private fun ScanFrame(done: Boolean) {
    val c = BitOneTheme.colors
    val color by animateColorAsState(if (done) c.ok else c.onAccent, stateSpec(), label = "frame")
    val sweep by
        rememberInfiniteTransition(label = "scan")
            .animateFloat(
                initialValue = 0f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(Motion.slow * 6, easing = LinearEasing), RepeatMode.Reverse),
                label = "sweep",
            )
    Canvas(Modifier.size(TokenSize.scanFrame)) {
        val stroke = TokenSize.scanStroke.toPx()
        val arm = TokenSize.scanArm.toPx()
        val radius = CornerRadius(Radius.r3.toPx())
        val end = size.width - arm
        // 四个角：画一个圆角方形，用裁剪只留下角。
        listOf(Offset.Zero, Offset(end, 0f), Offset(0f, end), Offset(end, end)).forEach { corner ->
            clipRect(corner.x, corner.y, corner.x + arm, corner.y + arm) {
                val inset = stroke / 2
                drawRoundRect(
                    color,
                    topLeft = Offset(inset, inset),
                    size = Size(size.width - stroke, size.height - stroke),
                    cornerRadius = radius,
                    style = Stroke(stroke),
                )
            }
        }
        if (!done) {
            val margin = SweepInset.toPx()
            val y = margin + (size.height - margin * 2) * sweep
            drawLine(c.accent, Offset(margin, y), Offset(size.width - margin, y), strokeWidth = 2.dp.toPx())
        }
    }
}

/** 框下一句提示，多帧时换成细进度条和帧序号。 */
@Composable
private fun ScanStatus(total: Int, progress: Float) {
    val c = BitOneTheme.colors
    val muted = c.onAccent.copy(alpha = HINT_ALPHA)
    if (total <= 1) {
        Txt("对准网页端的二维码", size = FontSize.t2, color = muted)
        return
    }
    val got = (progress * total).toInt().coerceIn(0, total)
    val fill by animateFloatAsState(progress, stateSpec(Motion.slow), label = "frames")
    val bar = RoundedCornerShape(Radius.rf)
    Row(
        Modifier.width(TokenSize.scanFrame),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.s3),
    ) {
        Box(Modifier.weight(1f).height(Space.s1).background(c.onAccent.copy(alpha = 0.24f), bar)) {
            Box(Modifier.fillMaxHeight().fillMaxWidth(fill).background(c.ok, bar))
        }
        Txt("$got/$total", size = FontSize.t2, color = muted)
    }
}

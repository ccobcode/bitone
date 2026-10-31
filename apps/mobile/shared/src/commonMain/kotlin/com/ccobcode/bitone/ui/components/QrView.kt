package com.ccobcode.bitone.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.i18n.translate
import com.ccobcode.bitone.platform.QrMatrix
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.LightColors
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Qr
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size as TokenSize
import com.ccobcode.bitone.ui.theme.Space

/** 屏幕高度里留给标题、帧进度和吸底按钮的部分。横屏时二维码按剩下的高度收窄，一屏看得全。 */
private val QrChrome = TokenSize.hRow * 3 + Space.s8

/** 全屏时只有帧进度和播放控制。 */
private val FullscreenChrome = TokenSize.hRow * 2

/**
 * 动态二维码，规格见 docs/06 §4.5。任何主题下都是白底深色码点，所以固定用浅色令牌。
 *
 * [total] 不超过 1 时是静态码，不显示帧进度。
 */
@Composable
fun QrView(
    matrix: QrMatrix?,
    frame: Int,
    total: Int,
    label: String,
    chrome: Dp = QrChrome,
) {
    val c = BitOneTheme.colors
    BoxWithConstraints(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        val density = LocalDensity.current
        val windowPx = LocalWindowInfo.current.containerSize.height
        val sidePx = minOf(constraints.maxWidth, windowPx - with(density) { chrome.roundToPx() })
        val side = with(density) { sidePx.toDp() }
        val shape = RoundedCornerShape(Radius.r3)
        Column(Modifier.width(side), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            if (matrix == null) {
                Skeleton(Modifier.size(side))
            } else {
                val modulePx = sidePx / (matrix.size + Qr.quiet * 2)
                val inset = (sidePx - modulePx * matrix.size) / 2
                val description = translate(label)
                Canvas(Modifier.size(side).clip(shape).semantics { contentDescription = description }) {
                    drawRect(LightColors.surface)
                    val module = Size(modulePx.toFloat(), modulePx.toFloat())
                    for (y in 0 until matrix.size) {
                        for (x in 0 until matrix.size) {
                            if (matrix.modules[y * matrix.size + x]) {
                                drawRect(
                                    LightColors.text,
                                    topLeft =
                                        Offset((inset + x * modulePx).toFloat(), (inset + y * modulePx).toFloat()),
                                    size = module,
                                )
                            }
                        }
                    }
                }
            }
            if (total > 1) {
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.s3),
                ) {
                    FrameBar(frame, total, Modifier.weight(1f))
                    Txt("$frame/$total", size = FontSize.t2, color = c.text2)
                }
            }
        }
    }
}

/** 帧数不多时每帧一段，当前帧是主色。太多了画不下，换成一条连续的进度条。 */
@Composable
private fun FrameBar(frame: Int, total: Int, modifier: Modifier) {
    val c = BitOneTheme.colors
    val bar = RoundedCornerShape(Radius.rf)
    if (total > Qr.maxSegments) {
        Box(modifier.height(Space.s1).clip(bar).background(c.lineStrong)) {
            Box(Modifier.fillMaxHeight().fillMaxWidth(frame.toFloat() / total).background(c.accent))
        }
    } else {
        Row(modifier, horizontalArrangement = Arrangement.spacedBy(Space.s1)) {
            for (segment in 1..total) {
                Box(
                    Modifier.weight(1f)
                        .height(Space.s1)
                        .background(if (segment == frame) c.accent else c.lineStrong, bar)
                )
            }
        }
    }
}

/** 动态二维码加播放控制，可暂停、调速、全屏，和网页端的 AnimatedQr 同一套。只有一帧时暂停和调速没有意义。 */
@Composable
fun QrPlayer(flow: WalletFlow, label: String, fullscreen: Boolean, onFullscreen: (Boolean) -> Unit) {
    val single = flow.frameTotal <= 1
    Column(
        Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Space.s4),
    ) {
        QrView(
            flow.frame,
            flow.frameIndex,
            flow.frameTotal,
            label,
            chrome = if (fullscreen) FullscreenChrome else QrChrome,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            SmallButton(if (flow.framesPaused) "播放" else "暂停", enabled = !single, onClick = flow::togglePaused)
            SmallButton("速度 ${SPEEDS[flow.frameSpeed]}", enabled = !single, onClick = flow::nextSpeed)
            SmallIconButton(if (fullscreen) "退出全屏" else "全屏显示二维码", Icons.Expand) { onFullscreen(!fullscreen) }
        }
    }
}

/** 顺序和令牌里的间隔一一对应。 */
private val SPEEDS = listOf("标准", "慢", "快")

/** 全屏显示二维码，盖住页面上其他内容，返回键先退出全屏。 */
@Composable
fun BoxScope.QrFullscreen(flow: WalletFlow, label: String, open: Boolean, onFullscreen: (Boolean) -> Unit) {
    CloseOnBack(flow, open) { onFullscreen(false) }
    AnimatedVisibility(
        open,
        Modifier.matchParentSize(),
        enter = fadeIn(stateSpec()),
        exit = fadeOut(exitSpec(Motion.fast)),
    ) {
        Box(
            Modifier.fillMaxSize()
                .background(BitOneTheme.colors.bg)
                .clickable(interactionSource = null, indication = null) {}
                .statusBarsPadding()
                .padding(horizontal = TokenSize.gutter),
            contentAlignment = Alignment.Center,
        ) {
            QrPlayer(flow, label, fullscreen = true, onFullscreen)
        }
    }
}

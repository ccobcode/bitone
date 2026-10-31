package com.ccobcode.bitone.ui.components

import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.Density
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius

// 动效只用 docs/06 §5 的令牌。系统关闭动画时 Compose 直接跳到终点，这里的一切都不等动画结束，流程不受影响。
// 金额和地址不走这里的任何数值动画，用户看到的始终是真实值。

private const val SHIMMER_FROM = 0.3f
private const val SHIMMER_PEAK = 0.5f
private const val SHIMMER_TO = 0.7f

/** 进入。 */
fun <T> enterSpec(duration: Int = Motion.normal, delay: Int = 0): FiniteAnimationSpec<T> =
    tween(duration, delay, Motion.emphasizedEasing)

/** 状态变化。 */
fun <T> stateSpec(duration: Int = Motion.normal): FiniteAnimationSpec<T> =
    tween(duration, easing = Motion.standardEasing)

/** 离开。 */
fun <T> exitSpec(duration: Int = Motion.normal): FiniteAnimationSpec<T> = tween(duration, easing = Motion.exitEasing)

/**
 * 前进时新页从右侧 24 滑入，旧页淡出并左移 8。返回时方向相反。总时长 `normal`。
 *
 * 旧页先淡出，新页稍后淡入，两页的文字不会叠在一起。
 */
fun pageTransition(back: Boolean, density: Density): ContentTransform {
    val near = with(density) { Motion.pageIn.roundToPx() }
    val far = with(density) { Motion.pageOut.roundToPx() }
    val handover = Motion.fast / 2
    val enter =
        slideInHorizontally(enterSpec()) { if (back) -far else near } +
            fadeIn(enterSpec(Motion.normal - handover, delay = handover))
    val exit = slideOutHorizontally(exitSpec()) { if (back) near else -far } + fadeOut(exitSpec(Motion.fast))
    return enter togetherWith exit
}

/** 动画中的旧页还在树上，但流程已经离开它。页面的回调都默认当前页就是自己，所以旧页不再接收触摸。 */
fun Modifier.ignoreTouches(ignore: Boolean): Modifier =
    pointerInput(ignore) {
        if (!ignore) return@pointerInput
        awaitPointerEventScope {
            while (true) {
                awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
            }
        }
    }

/** 流程清掉数据后，退场中的页面继续显示离开时的内容，不闪空白。 */
@Composable
fun <T : Any> rememberLatest(value: T?): T? {
    var latest by remember { mutableStateOf(value) }
    if (value != null) latest = value
    return latest
}

/** 按下缩到 0.97，松开回到原位，和网页端的曲线相同。 */
fun Modifier.pressScale(pressed: Boolean): Modifier = composed {
    val scale by
        animateFloatAsState(
            if (pressed) Motion.pressScale else 1f,
            if (pressed) stateSpec(Motion.fast) else enterSpec(),
            label = "press",
        )
    graphicsLayer {
        scaleX = scale
        scaleY = scale
    }
}

/** 列表逐项淡入上移 8，错开 30，最多错开 8 项。 */
fun Modifier.staggerIn(index: Int): Modifier = composed {
    val progress = remember { Animatable(0f) }
    LaunchedEffect(Unit) {
        val delay = minOf(index, Motion.staggerMax) * Motion.stagger
        progress.animateTo(1f, enterSpec(delay = delay))
    }
    graphicsLayer {
        alpha = progress.value
        translationY = (1f - progress.value) * Motion.rise.toPx()
    }
}

/** 骨架屏微光，一条光带匀速扫过，周期和网页端相同。不用转圈。 */
@Composable
fun Skeleton(modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    val sweep by
        rememberInfiniteTransition(label = "skeleton")
            .animateFloat(
                initialValue = 0f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(Motion.slow * 4, easing = LinearEasing), RepeatMode.Restart),
                label = "sweep",
            )
    Box(
        modifier.clip(RoundedCornerShape(Radius.r1)).drawBehind {
            val tile = size.width * 2
            val start = tile * sweep
            drawRect(
                Brush.linearGradient(
                    SHIMMER_FROM to c.surface2,
                    SHIMMER_PEAK to c.surface3,
                    SHIMMER_TO to c.surface2,
                    start = Offset(start, 0f),
                    end = Offset(start + tile, 0f),
                    tileMode = TileMode.Repeated,
                )
            )
        }
    )
}

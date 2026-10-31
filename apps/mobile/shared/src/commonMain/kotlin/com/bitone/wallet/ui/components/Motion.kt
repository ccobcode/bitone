package com.bitone.wallet.ui.components

import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FiniteAnimationSpec
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
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
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius

// 动效只用 docs/06 §5 的令牌。系统关闭动画时 Compose 直接跳到终点，这里的一切都不等动画结束，流程不受影响。
// 金额和地址不走这里的任何数值动画，用户看到的始终是真实值。

private val PageEnterOffset = 24.dp
private val PageExitOffset = 8.dp
private val ListEnterOffset = 8.dp
private const val LIST_MAX_STAGGERED = 8
private const val PRESS_RELEASE_DAMPING = 0.55f
private const val PRESS_RELEASE_STIFFNESS = 900f

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
    val near = with(density) { PageEnterOffset.roundToPx() }
    val far = with(density) { PageExitOffset.roundToPx() }
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

/** 按下缩到 0.97，松手回弹。 */
fun Modifier.pressScale(pressed: Boolean): Modifier = composed {
    val scale by
        animateFloatAsState(
            if (pressed) Motion.pressScale else 1f,
            if (pressed) {
                stateSpec(Motion.fast)
            } else {
                spring(PRESS_RELEASE_DAMPING, PRESS_RELEASE_STIFFNESS)
            },
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
        val delay = minOf(index, LIST_MAX_STAGGERED) * Motion.stagger
        progress.animateTo(1f, enterSpec(delay = delay))
    }
    graphicsLayer {
        alpha = progress.value
        translationY = (1f - progress.value) * ListEnterOffset.toPx()
    }
}

/** 骨架屏微光，不用转圈。 */
@Composable
fun Skeleton(modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    val light by
        rememberInfiniteTransition(label = "skeleton")
            .animateFloat(
                initialValue = 0f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(Motion.slow * 2, easing = LinearEasing), RepeatMode.Reverse),
                label = "light",
            )
    Box(modifier.background(lerp(c.surface2, c.surface3, light), RoundedCornerShape(Radius.r1)))
}

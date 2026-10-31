package com.ccobcode.bitone.ui.components

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size
import com.ccobcode.bitone.ui.theme.Space
import com.ccobcode.bitone.ui.theme.elevation

/** 页面的内容列：限宽、避开左右挖孔。底部导航这类页面之外的元素也用它，才能和页面对齐。 */
@Composable
fun Modifier.contentColumn(): Modifier =
    widthIn(max = Size.contentMax)
        .fillMaxWidth()
        .windowInsetsPadding(WindowInsets.displayCutout.only(WindowInsetsSides.Horizontal))

/**
 * 页面骨架，自上而下：状态栏安全区、标题区、可滚动内容、吸底区、导航栏安全区。 安全区全部来自 WindowInsets，背景铺满到屏幕边缘。
 *
 * 左右边线只在这里实现：标题区、内容和吸底区都缩进 [Size.gutter]，页面和组件不再各写边距。内容没有滚到底时，用底色渐隐过渡到吸底区，不画分隔线。
 *
 * [floatingSpace] 是悬浮在页面之上的底部导航占的高度，内容末尾留出这段，滚到底时不会被盖住。
 */
@Composable
fun Page(
    header: @Composable (ColumnScope.() -> Unit)? = null,
    bottom: @Composable (ColumnScope.() -> Unit)? = null,
    floatingSpace: Dp = 0.dp,
    contentGap: Dp = Space.s5,
    contentAlignment: Alignment.Horizontal = Alignment.Start,
    contentAnchor: Alignment.Vertical = Alignment.Top,
    overlay: @Composable (BoxScope.() -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = BitOneTheme.colors
    val scroll = rememberScrollState()
    val navigationBar = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val floating = if (floatingSpace > 0.dp) floatingSpace + navigationBar else 0.dp
    Box(Modifier.fillMaxSize().background(c.bg), contentAlignment = Alignment.TopCenter) {
        Column(
            Modifier.contentColumn()
                .fillMaxHeight()
                .windowInsetsPadding(
                    WindowInsets.statusBars.union(WindowInsets.displayCutout.only(WindowInsetsSides.Top))
                )
        ) {
            if (header != null) {
                Column(Modifier.fillMaxWidth().padding(horizontal = Size.gutter), content = header)
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                Column(
                    Modifier.fillMaxSize()
                        .verticalScroll(scroll)
                        .padding(
                            start = Size.gutter,
                            end = Size.gutter,
                            top = Space.s2,
                            bottom = Space.s6 + floating,
                        ),
                    horizontalAlignment = contentAlignment,
                    verticalArrangement = Arrangement.spacedBy(contentGap, contentAnchor),
                    content = content,
                )
                if (bottom != null) BottomFade(scroll, Modifier.align(Alignment.BottomCenter))
            }
            Column(Modifier.fillMaxWidth().padding(horizontal = Size.gutter)) {
                bottom?.invoke(this)
                if (floatingSpace == 0.dp) Spacer(Modifier.windowInsetsBottomHeight(WindowInsets.navigationBars))
            }
        }
        overlay?.invoke(this)
    }
}

@Composable
private fun BottomFade(scroll: ScrollState, modifier: Modifier) {
    val bg = BitOneTheme.colors.bg
    val alpha by animateFloatAsState(if (scroll.canScrollForward) 1f else 0f, stateSpec(Motion.fast), label = "fade")
    Box(
        modifier
            .fillMaxWidth()
            .height(Space.s8)
            .graphicsLayer { this.alpha = alpha }
            .background(Brush.verticalGradient(listOf(Color.Transparent, bg)))
    )
}

/** 标题栏，高 56。返回图标的左边缘在边线上，右侧内容的右边缘也在边线上。流程的终点页把 [backIcon] 换成关闭。 */
@Composable
fun TitleBar(
    title: String,
    onBack: (() -> Unit)?,
    backIcon: String = Icons.Back,
    leading: @Composable (() -> Unit)? = null,
    trailing: @Composable (() -> Unit)? = null,
) {
    Row(Modifier.fillMaxWidth().height(Size.hRow), verticalAlignment = Alignment.CenterVertically) {
        if (onBack != null) BackButton(onBack, backIcon)
        if (leading != null) {
            leading()
            Spacer(Modifier.width(Space.s2))
        }
        Txt(title, Modifier.weight(1f), size = FontSize.t4, weight = SemiBold)
        trailing?.invoke()
    }
}

/** 一级页面的大标题。 */
@Composable
fun LargeTitle(title: String) {
    Row(Modifier.fillMaxWidth().height(Size.hRow), verticalAlignment = Alignment.CenterVertically) {
        Txt(title, size = FontSize.t6, weight = SemiBold)
    }
}

/** 提示和倒计时变化时新值淡入，不做滚动。 */
@Composable
fun FadeText(text: String, color: Color) {
    AnimatedContent(
        text,
        transitionSpec = { (fadeIn(stateSpec(Motion.fast)) togetherWith fadeOut(exitSpec(Motion.fast))).using(null) },
        label = "value",
    ) { value ->
        Txt(value, size = FontSize.t2, color = color)
    }
}

/** 提示，和网页端手机上的同一规格：贴着屏幕上方，从上方淡入上移。画在同一窗口里，不用系统 Toast。 */
@Composable
fun BoxScope.Toast(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val shown = rememberLatest(flow.notice)
    val failed = flow.noticeFailed
    val rise = with(LocalDensity.current) { Motion.rise.roundToPx() }
    val shape = RoundedCornerShape(Radius.r3)
    AnimatedVisibility(
        flow.notice != null,
        Modifier.align(Alignment.TopCenter)
            .contentColumn()
            .statusBarsPadding()
            .padding(horizontal = Size.gutter, vertical = Space.s2),
        enter = fadeIn(enterSpec()) + slideInVertically(enterSpec()) { rise },
        exit = fadeOut(exitSpec(Motion.fast)),
    ) {
        Row(
            Modifier.fillMaxWidth()
                .elevation(BitOneTheme.elevation.elevation2, shape)
                .background(c.surface, shape)
                .padding(horizontal = Space.s4, vertical = Space.s3),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.s2),
        ) {
            LineIcon(if (failed) Icons.Alert else Icons.Check, 16.dp, if (failed) c.danger else c.accent)
            Txt(shown.orEmpty(), Modifier.weight(1f))
        }
    }
}

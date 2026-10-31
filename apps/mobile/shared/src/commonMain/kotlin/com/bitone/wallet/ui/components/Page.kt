package com.bitone.wallet.ui.components

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
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
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Size
import com.bitone.wallet.ui.theme.Space

/** 横屏和折叠屏时内容最大宽度，见界面规范 §7。 */
private val MaxContentWidth = 560.dp

/** 提示浮在吸底按钮和底部导航之上。 */
private val NoticeLift = 84.dp

/** 页面的内容列：限宽、避开左右挖孔。底部导航这类页面之外的元素也用它，才能和页面对齐。 */
@Composable
fun Modifier.contentColumn(): Modifier =
    widthIn(max = MaxContentWidth)
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
    modifier: Modifier = Modifier,
    header: @Composable (ColumnScope.() -> Unit)? = null,
    bottom: @Composable (ColumnScope.() -> Unit)? = null,
    floatingSpace: Dp = 0.dp,
    contentGap: Dp = Space.s5,
    contentAlignment: Alignment.Horizontal = Alignment.Start,
    contentCentered: Boolean = false,
    contentAtBottom: Boolean = false,
    scroll: ScrollState = rememberScrollState(),
    overlay: @Composable (BoxScope.() -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = BitOneTheme.colors
    val navigationBar = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val floating = if (floatingSpace > 0.dp) floatingSpace + navigationBar else 0.dp
    Box(modifier.fillMaxSize().background(c.bg), contentAlignment = Alignment.TopCenter) {
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
                    verticalArrangement =
                        when {
                            contentAtBottom -> Arrangement.spacedBy(contentGap, Alignment.Bottom)
                            contentCentered -> Arrangement.spacedBy(contentGap, Alignment.CenterVertically)
                            else -> Arrangement.spacedBy(contentGap)
                        },
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
    tint: Color = BitOneTheme.colors.text,
    leading: @Composable (() -> Unit)? = null,
    trailing: @Composable (() -> Unit)? = null,
) {
    Row(Modifier.fillMaxWidth().height(Size.hRow), verticalAlignment = Alignment.CenterVertically) {
        if (onBack != null) BackButton(onBack, backIcon, tint)
        if (leading != null) {
            leading()
            Spacer(Modifier.width(Space.s2))
        }
        Txt(title, Modifier.weight(1f), size = FontSize.t4, weight = SemiBold, color = tint)
        trailing?.invoke()
    }
}

/** 一级页面的大标题。 */
@Composable
fun LargeTitle(title: String, trailing: @Composable (() -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().height(Size.hRow), verticalAlignment = Alignment.CenterVertically) {
        Txt(title, Modifier.weight(1f), size = FontSize.t6, weight = SemiBold)
        trailing?.invoke()
    }
}

/** 提示和倒计时变化时新值淡入，不做滚动。 */
@Composable
fun FadeText(text: String, color: Color, size: TextUnit = FontSize.t2) {
    AnimatedContent(
        text,
        transitionSpec = { (fadeIn(stateSpec(Motion.fast)) togetherWith fadeOut(exitSpec(Motion.fast))).using(null) },
        label = "value",
    ) { value ->
        Txt(value, size = size, color = color)
    }
}

/** 悬浮提示，画在同一窗口里，不用系统 Toast。 */
@Composable
fun BoxScope.NoticePill(text: String?) {
    val c = BitOneTheme.colors
    val shown = rememberLatest(text)
    AnimatedVisibility(
        text != null,
        Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = NoticeLift),
        enter = fadeIn(enterSpec()),
        exit = fadeOut(exitSpec(Motion.fast)),
    ) {
        Box(
            Modifier.background(c.text, RoundedCornerShape(Radius.rf))
                .padding(horizontal = Space.s4, vertical = Space.s2)
        ) {
            Txt(shown.orEmpty(), size = FontSize.t2, weight = Medium, color = c.bg)
        }
    }
}

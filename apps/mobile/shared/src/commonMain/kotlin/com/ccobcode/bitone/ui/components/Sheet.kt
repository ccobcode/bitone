package com.ccobcode.bitone.ui.components

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size
import com.ccobcode.bitone.ui.theme.Space
import com.ccobcode.bitone.ui.theme.elevation

/** 打开时登记到流程里，返回键先关掉它，再离开页面。 */
@Composable
fun CloseOnBack(flow: WalletFlow, open: Boolean, onClose: () -> Unit) {
    val close by rememberUpdatedState(onClose)
    DisposableEffect(open) {
        if (!open) return@DisposableEffect onDispose {}
        val layer = { close() }
        flow.openLayer(layer)
        onDispose { flow.closeLayer(layer) }
    }
}

/**
 * 底部弹层，和网页端手机上的浮层同一规格：从底部升起，上方圆角 24，标题加关闭按钮，遮罩点一下也能关。
 *
 * [list] 时内容贴着弹层两边，列表行自己留边距，文字和标题对齐。画在同一窗口里，不用 Dialog：新窗口不继承 FLAG_SECURE。
 */
@Composable
internal fun BoxScope.Sheet(
    flow: WalletFlow,
    visible: Boolean,
    title: String,
    onClose: () -> Unit,
    list: Boolean = false,
    content: @Composable ColumnScope.() -> Unit,
) {
    CloseOnBack(flow, visible, onClose)
    val c = BitOneTheme.colors
    val shape = RoundedCornerShape(topStart = Radius.r4, topEnd = Radius.r4)
    val rise = with(LocalDensity.current) { Motion.rise.roundToPx() }
    AnimatedVisibility(
        visible,
        Modifier.matchParentSize(),
        enter = fadeIn(stateSpec()),
        exit = fadeOut(exitSpec(Motion.fast)),
    ) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            Box(
                Modifier.fillMaxSize()
                    .background(c.camera.copy(alpha = Motion.scrim))
                    .clickable(interactionSource = null, indication = null, onClick = onClose)
            )
            Box(Modifier.statusBarsPadding().padding(top = Space.s8), contentAlignment = Alignment.BottomCenter) {
                Column(
                    Modifier.contentColumn()
                        .animateEnterExit(
                            enter = slideInVertically(enterSpec()) { rise } + scaleIn(enterSpec(), Motion.popScale),
                            exit = scaleOut(exitSpec(Motion.fast), Motion.popScale),
                        )
                        .elevation(BitOneTheme.elevation.elevation3, shape)
                        .background(c.surface2, shape)
                        .clickable(interactionSource = null, indication = null) {}
                        .verticalScroll(rememberScrollState())
                        .navigationBarsPadding()
                        .padding(top = Space.s2, bottom = Space.s4),
                    verticalArrangement = Arrangement.spacedBy(Space.s3),
                ) {
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = Size.hRow).padding(horizontal = Size.gutter),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Txt(title, Modifier.weight(1f), size = FontSize.t5, weight = SemiBold)
                        CloseButton(onClose)
                    }
                    Column(
                        Modifier.fillMaxWidth().padding(horizontal = if (list) Space.s1 else Size.gutter),
                        verticalArrangement = Arrangement.spacedBy(Space.s3),
                        content = content,
                    )
                }
            }
        }
    }
}

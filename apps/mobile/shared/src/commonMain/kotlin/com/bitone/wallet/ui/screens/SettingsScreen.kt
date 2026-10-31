package com.bitone.wallet.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.i18n.I18n
import com.bitone.wallet.i18n.Lang
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.DangerButton
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.LargeTitle
import com.bitone.wallet.ui.components.LineIcon
import com.bitone.wallet.ui.components.ListGroup
import com.bitone.wallet.ui.components.ListRow
import com.bitone.wallet.ui.components.NoticePill
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.SemiBold
import com.bitone.wallet.ui.components.SwitchRow
import com.bitone.wallet.ui.components.TextButton
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.contentColumn
import com.bitone.wallet.ui.components.enterSpec
import com.bitone.wallet.ui.components.exitSpec
import com.bitone.wallet.ui.components.staggerIn
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Size
import com.bitone.wallet.ui.theme.Space
import com.bitone.wallet.ui.theme.elevation

private const val SHEET_INITIAL_SCALE = 0.96f

/** A10 设置。分组列表，组内没有分隔线。抹除确认在 [MainTabs]，它要盖住底部导航。 */
@Composable
fun SettingsScreen(flow: WalletFlow, onWipe: () -> Unit) {
    val c = BitOneTheme.colors
    LaunchedEffect(Unit) { flow.loadManifestVersion() }
    Page(
        header = { LargeTitle("设置") },
        contentGap = Space.s4,
        floatingSpace = NavSpace,
        overlay = { NoticePill(flow.notice) },
    ) {
        ListGroup(Modifier.staggerIn(0).fillMaxWidth()) {
            NavRow("修改 PIN", flow::startChangePin)
            NavRow("查看助记词", flow::startViewWords)
            NavRow("内置清单", flow::openManifest) {
                flow.manifestVersion?.let { Txt(manifestLabel(it), size = FontSize.t2, color = c.text2) }
            }
        }
        ListGroup(Modifier.staggerIn(1).fillMaxWidth()) {
            SwitchRow(flow.autoBrightness, flow::toggleAutoBrightness) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Txt("自动亮度")
                    Txt("展示二维码时调到最亮", size = FontSize.t2, color = c.text3)
                }
            }
            NavRow("语言", flow::toggleLanguage) {
                Txt(if (I18n.lang == Lang.Zh) "中文" else "English", size = FontSize.t2, color = c.text2)
            }
        }
        ListGroup(Modifier.staggerIn(2).fillMaxWidth()) {
            ListRow {
                Txt("应用版本", Modifier.weight(1f))
                Txt(flow.appVersion, size = FontSize.t2, color = c.text2)
            }
            Column(
                Modifier.padding(start = Space.s4, end = Space.s4, top = Space.s1, bottom = Space.s4),
                verticalArrangement = Arrangement.spacedBy(Space.s2),
            ) {
                Txt("签名证书指纹")
                Txt(flow.certificate, size = FontSize.t1, color = c.text2, mono = true, lineHeight = 1.6f)
            }
        }
        if (flow.canWipe) {
            ListGroup(Modifier.staggerIn(3).fillMaxWidth()) {
                ListRow(onClick = onWipe) { Txt("抹除钱包", Modifier.weight(1f), color = c.danger) }
            }
        }
    }
}

@Composable
private fun NavRow(label: String, onClick: () -> Unit, trailing: @Composable () -> Unit = {}) {
    ListRow(onClick = onClick) {
        Txt(label, Modifier.weight(1f))
        trailing()
        LineIcon(Icons.Chevron, 18.dp, BitOneTheme.colors.text3, strokeWidth = 2f)
    }
}

/** A10 查看助记词。解锁后才有内容，离开页面或退到后台立刻清掉。 */
@Composable
fun ViewWordsScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val rowHeight = if (flow.viewWords.size <= 12) 40.dp else 32.dp
    Page(
        header = { TitleBar("查看助记词", onBack = { flow.back() }) },
        contentGap = Space.s4,
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow::leaveViewWords) } },
    ) {
        Txt("只抄在纸上", size = FontSize.t2, color = c.text3)
        WordGrid(flow.viewWords, rowHeight)
    }
}

/** 抹除的二次确认。 */
@Composable
internal fun BoxScope.WipeSheet(visible: Boolean, busy: Boolean, onCancel: () -> Unit, onConfirm: () -> Unit) {
    val c = BitOneTheme.colors
    BottomSheet(visible, onCancel) {
        Txt("抹除钱包", size = FontSize.t5, weight = SemiBold)
        Txt("抹除后只能用助记词恢复。", color = c.text2)
        Spacer(Modifier)
        DangerButton("确认抹除", enabled = !busy, onClick = onConfirm)
        TextButton("取消", enabled = !busy, onClick = onCancel)
    }
}

/** 底部弹层。从 96% 放大并淡入。画在同一窗口里，不用 Dialog：新窗口不继承 FLAG_SECURE。 */
@Composable
internal fun BoxScope.BottomSheet(
    visible: Boolean,
    onDismiss: () -> Unit,
    padding: Dp = Space.s5,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = BitOneTheme.colors
    val shape = RoundedCornerShape(Radius.r4)
    AnimatedVisibility(
        visible,
        Modifier.matchParentSize(),
        enter = fadeIn(enterSpec()),
        exit = fadeOut(exitSpec(Motion.fast)),
    ) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
            Box(
                Modifier.fillMaxSize()
                    .background(c.camera.copy(alpha = 0.6f))
                    .clickable(interactionSource = null, indication = null, onClick = onDismiss)
            )
            Column(
                Modifier.contentColumn()
                    .navigationBarsPadding()
                    .padding(start = Size.gutter, end = Size.gutter, bottom = Space.s4)
                    .animateEnterExit(
                        enter = scaleIn(enterSpec(), SHEET_INITIAL_SCALE),
                        exit = scaleOut(exitSpec(Motion.fast), SHEET_INITIAL_SCALE),
                    )
                    .elevation(BitOneTheme.elevation.elevation3, shape)
                    .background(c.surface, shape)
                    .clickable(interactionSource = null, indication = null) {}
                    .padding(padding),
                verticalArrangement = Arrangement.spacedBy(Space.s3),
                content = content,
            )
        }
    }
}

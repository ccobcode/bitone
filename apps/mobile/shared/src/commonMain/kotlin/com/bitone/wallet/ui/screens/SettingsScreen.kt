package com.bitone.wallet.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.i18n.I18n
import com.bitone.wallet.i18n.Lang
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.Card
import com.bitone.wallet.ui.components.DangerButton
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.LargeTitle
import com.bitone.wallet.ui.components.LineIcon
import com.bitone.wallet.ui.components.ListRow
import com.bitone.wallet.ui.components.Notice
import com.bitone.wallet.ui.components.NoticeTone
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.SecondaryButton
import com.bitone.wallet.ui.components.Sheet
import com.bitone.wallet.ui.components.SwitchRow
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Toast
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.staggerIn
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.LineHeight
import com.bitone.wallet.ui.theme.Space

/** A10 设置。分组列表，组内没有分隔线。抹除确认在 [MainTabs]，它要盖住底部导航。 */
@Composable
fun SettingsScreen(flow: WalletFlow, onWipe: () -> Unit) {
    val c = BitOneTheme.colors
    LaunchedEffect(Unit) { flow.loadManifestVersion() }
    Page(
        header = { LargeTitle("设置") },
        contentGap = Space.s4,
        floatingSpace = NavSpace,
        overlay = { Toast(flow) },
    ) {
        Card(Modifier.staggerIn(0).fillMaxWidth()) {
            NavRow("修改 PIN", flow::startChangePin)
            NavRow("查看助记词", flow::startViewWords)
            NavRow("内置清单", flow::openManifest) {
                flow.manifestVersion?.let { Txt(manifestLabel(it), size = FontSize.t2, color = c.text2) }
            }
        }
        Card(Modifier.staggerIn(1).fillMaxWidth()) {
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
        Card(Modifier.staggerIn(2).fillMaxWidth()) {
            ListRow {
                Txt("应用版本", Modifier.weight(1f))
                Txt(flow.appVersion, size = FontSize.t2, color = c.text2)
            }
            Column(
                Modifier.padding(start = Space.s4, end = Space.s4, top = Space.s1, bottom = Space.s4),
                verticalArrangement = Arrangement.spacedBy(Space.s2),
            ) {
                Txt("签名证书指纹")
                Txt(flow.certificate, size = FontSize.t1, color = c.text2, mono = true, lineHeight = LineHeight.t2)
            }
        }
        if (flow.canWipe) {
            Card(Modifier.staggerIn(3).fillMaxWidth()) {
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
        LineIcon(Icons.Caret, 16.dp, BitOneTheme.colors.text3)
    }
}

/** A10 查看助记词。解锁后才有内容，离开页面或退到后台立刻清掉。 */
@Composable
fun ViewWordsScreen(flow: WalletFlow) {
    val rowHeight = if (flow.viewWords.size <= 12) 40.dp else 32.dp
    Page(
        header = { TitleBar("查看助记词", onBack = { flow.back() }) },
        contentGap = Space.s4,
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow::leaveViewWords) } },
    ) {
        Notice("只抄在纸上", NoticeTone.Warn)
        WordGrid(flow.viewWords, rowHeight)
    }
}

/** 抹除的二次确认，和网页端的确认浮层同一写法：放弃在左，确认在右。 */
@Composable
internal fun BoxScope.WipeSheet(flow: WalletFlow, visible: Boolean, onCancel: () -> Unit, onConfirm: () -> Unit) {
    val c = BitOneTheme.colors
    Sheet(flow, visible, "抹除钱包", onCancel) {
        Txt("抹除后只能用助记词恢复。", color = c.text2)
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            SecondaryButton("取消", Modifier.weight(1f), enabled = !flow.busy, fill = c.surface3, onClick = onCancel)
            DangerButton("确认抹除", Modifier.weight(1f), enabled = !flow.busy, onClick = onConfirm)
        }
    }
}

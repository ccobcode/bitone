package com.ccobcode.bitone.ui.screens

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
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.i18n.Lang
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.DangerButton
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LargeTitle
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.Notice
import com.ccobcode.bitone.ui.components.NoticeTone
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.SecondaryButton
import com.ccobcode.bitone.ui.components.Sheet
import com.ccobcode.bitone.ui.components.SwitchRow
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Toast
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.staggerIn
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.LineHeight
import com.ccobcode.bitone.ui.theme.Space

/** A10 设置.分组列表,组内没有分隔线.语言选择和抹除确认在 [MainTabs],它们要盖住底部导航. */
@Composable
fun SettingsScreen(flow: WalletFlow, onLanguage: () -> Unit, onWipe: () -> Unit) {
    val c = BitOneTheme.colors
    LaunchedEffect(Unit) { flow.manifest.loadVersion() }
    Page(
        header = { LargeTitle("设置") },
        contentGap = Space.s4,
        floatingSpace = NavSpace,
        overlay = { Toast(flow) },
    ) {
        Card(Modifier.staggerIn(0).fillMaxWidth()) {
            NavRow("修改 PIN", flow.settings::startChangePin)
            NavRow("查看助记词", flow.settings::startViewWords)
            NavRow("内置清单", flow.manifest::open) {
                flow.manifest.version?.let { Txt(manifestLabel(it), size = FontSize.t2, color = c.text2) }
            }
        }
        Card(Modifier.staggerIn(1).fillMaxWidth()) {
            SwitchRow(flow.settings.autoBrightness, flow.settings::toggleAutoBrightness) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Txt("自动亮度")
                    Txt("展示二维码时调到最亮", size = FontSize.t2, color = c.text3)
                }
            }
            NavRow("语言", onLanguage) {
                Txt(languageLabel(flow.settings.languageChoice), size = FontSize.t2, color = c.text2)
            }
        }
        Card(Modifier.staggerIn(2).fillMaxWidth()) {
            ListRow {
                Txt("应用版本", Modifier.weight(1f))
                Txt(flow.settings.appVersion, size = FontSize.t2, color = c.text2)
            }
            Column(
                Modifier.padding(start = Space.s4, end = Space.s4, top = Space.s1, bottom = Space.s4),
                verticalArrangement = Arrangement.spacedBy(Space.s2),
            ) {
                Txt("签名证书指纹")
                Txt(
                    flow.settings.certificate,
                    size = FontSize.t1,
                    color = c.text2,
                    mono = true,
                    lineHeight = LineHeight.t2,
                )
            }
        }
        Card(Modifier.staggerIn(3).fillMaxWidth()) {
            ListRow(onClick = onWipe) { Txt("抹除钱包", Modifier.weight(1f), color = c.danger) }
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

private fun languageLabel(choice: Lang?) = choice?.label ?: "自动"

/** 语言选择,立即生效. */
@Composable
internal fun BoxScope.LanguageSheet(visible: Boolean, flow: WalletFlow, onClose: () -> Unit) {
    Sheet(flow, visible, "语言", onClose, list = true) {
        Column {
            (listOf(null) + Lang.entries).forEach { choice ->
                ListRow(
                    selected = choice == flow.settings.languageChoice,
                    inset = Space.s1,
                    onClick = {
                        flow.settings.chooseLanguage(choice)
                        onClose()
                    },
                ) {
                    Txt(languageLabel(choice), Modifier.weight(1f), weight = Medium)
                }
            }
        }
    }
}

/** A10 查看助记词,启用了 BIP39 密码时密码在词下面.解锁后才有内容,离开页面或退到后台立刻清掉. */
@Composable
fun ViewWordsScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val rowHeight = if (flow.settings.viewWords.size <= 12) 40.dp else 32.dp
    val passphrase = flow.settings.viewPassphrase
    Page(
        header = { TitleBar("查看助记词", onBack = { flow.back() }) },
        contentGap = Space.s4,
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow.settings::leaveViewWords) } },
    ) {
        Notice("只抄在纸上", NoticeTone.Warn)
        WordGrid(flow.settings.viewWords, rowHeight)
        if (passphrase.isNotEmpty()) {
            Card {
                Column(
                    Modifier.fillMaxWidth().padding(Space.s4),
                    verticalArrangement = Arrangement.spacedBy(Space.s2),
                ) {
                    Txt("BIP39 密码", size = FontSize.t2, color = c.text3)
                    // 密码不是文案,不翻译.
                    Txt(passphrase, mono = true, translated = false)
                }
            }
        }
    }
}

/** 抹除的二次确认,和网页端的确认浮层同一写法:放弃在左,确认在右. */
@Composable
internal fun BoxScope.WipeSheet(flow: WalletFlow, visible: Boolean, onCancel: () -> Unit, onConfirm: () -> Unit) {
    val c = BitOneTheme.colors
    Sheet(flow, visible, "抹除钱包", onCancel) {
        Txt("抹除后只能用助记词恢复.", color = c.text2)
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            SecondaryButton("取消", Modifier.weight(1f), enabled = !flow.busy, fill = c.surface3, onClick = onCancel)
            DangerButton("确认抹除", Modifier.weight(1f), enabled = !flow.busy, onClick = onConfirm)
        }
    }
}

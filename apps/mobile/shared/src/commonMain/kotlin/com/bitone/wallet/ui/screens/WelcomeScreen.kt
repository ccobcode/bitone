package com.bitone.wallet.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.CheckState
import com.bitone.wallet.flow.SelfCheckReport
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.BrandMark
import com.bitone.wallet.ui.components.Card
import com.bitone.wallet.ui.components.Chip
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.LineIcon
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.Notice
import com.bitone.wallet.ui.components.NoticeTone
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.SecondaryButton
import com.bitone.wallet.ui.components.SemiBold
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.staggerIn
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Space

/** A1 欢迎与自检。 */
@Composable
fun WelcomeScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val ready = flow.report?.blocked == false && !flow.busy
    Page(
        bottom = {
            ActionArea {
                PrimaryButton("创建新钱包", enabled = ready, onClick = flow::startCreate)
                SecondaryButton("导入助记词", enabled = ready, onClick = flow::startImport)
            }
        }
    ) {
        Column(Modifier.padding(top = Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s4)) {
            BrandMark(48.dp)
            Column(verticalArrangement = Arrangement.spacedBy(Space.s1)) {
                Txt("BitOne", size = FontSize.t7, weight = SemiBold)
                Txt("多链冷钱包", color = c.text3)
            }
            Notice("BitOne 只在专用手机上运行：恢复出厂，不登录账号，只装本应用。")
        }
        flow.report?.let { SelfCheckList(it) }
        Notice("移除屏幕锁后，钱包只能用助记词恢复。", NoticeTone.Warn)
    }
}

@Composable
private fun SelfCheckList(report: SelfCheckReport) {
    val c = BitOneTheme.colors
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Txt("离线自检", weight = Medium)
            val warned = report.warned > 0
            Chip(report.summary, if (warned) c.warnSoft else c.okSoft, if (warned) c.warn else c.ok)
        }
        Card {
            Column(Modifier.padding(vertical = Space.s2)) {
                report.items.forEachIndexed { index, item ->
                    val warn = item.state != CheckState.Pass
                    Row(
                        Modifier.staggerIn(index).fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = Space.s4),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(Space.s3),
                    ) {
                        LineIcon(
                            if (warn) Icons.Warn else Icons.Check,
                            18.dp,
                            if (warn) c.warn else c.ok,
                        )
                        Txt(item.label, Modifier.weight(1f))
                        Txt(item.value, size = FontSize.t2, color = if (warn) c.warn else c.text3)
                    }
                }
            }
        }
    }
}

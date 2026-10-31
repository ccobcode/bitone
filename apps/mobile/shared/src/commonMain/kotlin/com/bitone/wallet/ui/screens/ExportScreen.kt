package com.bitone.wallet.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.Network
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.platform.Brightness
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.Card
import com.bitone.wallet.ui.components.Chip
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.NetworkIcon
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.QrFullscreen
import com.bitone.wallet.ui.components.QrPlayer
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Toast
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Space

private const val LABEL = "观察钱包二维码"

/** A4 导出观察钱包。进入时已解锁，种子已清除，这里只有公钥帧。 */
@Composable
fun ExportScreen(flow: WalletFlow, brightness: Brightness) {
    val c = BitOneTheme.colors
    var fullscreen by remember { mutableStateOf(false) }
    ShowingQr(flow, brightness)
    LaunchedEffect(Unit) { flow.playFrames() }
    Page(
        header = { TitleBar("连接网页端", onBack = { flow.back() }) },
        contentAlignment = Alignment.CenterHorizontally,
        contentGap = Space.s4,
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow::finishExport) } },
        overlay = {
            Toast(flow)
            QrFullscreen(flow, LABEL, fullscreen) { fullscreen = it }
        },
    ) {
        QrPlayer(flow, LABEL, fullscreen) { fullscreen = it }
        Card {
            Column(Modifier.padding(Space.s5), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
                // 启用 BIP39 密码时只有这里能核对是否输错，输错不会报错。
                if (flow.passphraseRequired) {
                    LabelRow("主密钥指纹") {
                        Txt(flow.fingerprint.orEmpty(), size = FontSize.t2, color = c.text2, mono = true)
                    }
                }
                LabelRow("包含的账户") { Chip("只含公钥", c.okSoft, c.ok, icon = Icons.Shield) }
                Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
                    Network.entries.forEach { NetworkIcon(it, 28.dp) }
                }
            }
        }
    }
}

@Composable
private fun LabelRow(label: String, value: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Txt(label, size = FontSize.t2, color = BitOneTheme.colors.text3)
        value()
    }
}

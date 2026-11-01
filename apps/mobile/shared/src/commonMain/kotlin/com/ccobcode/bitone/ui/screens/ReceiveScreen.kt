package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.networkName
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.AddressChunks
import com.ccobcode.bitone.ui.components.AssetIcon
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LargeTitle
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.NetworkIcon
import com.ccobcode.bitone.ui.components.Notice
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.QrView
import com.ccobcode.bitone.ui.components.Sheet
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Toast
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.tap
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space

/** 最长的地址占三行,短地址也留出同样的高度,换账户时下面的内容不跳. */
private const val ADDRESS_LINES = 3

/** A11 收款.地址从本机公钥算出,不经过网页端,也不显示派生路径.冷钱包不用剪贴板,所以没有复制按钮.版式和网页端的收款相同. */
@Composable
fun ReceiveScreen(
    flow: WalletFlow,
    brightness: Brightness,
    onPickAsset: () -> Unit,
    onPickAccount: () -> Unit,
) {
    val c = BitOneTheme.colors
    ShowingQr(flow, brightness)
    Page(
        header = { LargeTitle("收款") },
        floatingSpace = NavSpace,
        contentGap = Space.s4,
        contentAlignment = Alignment.CenterHorizontally,
        overlay = { Toast(flow) },
    ) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            Select(Modifier.weight(2f), onPickAsset) {
                flow.receive.asset?.let {
                    AssetIcon(it.icon, null, 24.dp)
                    Txt(it.symbol, Modifier.weight(1f), weight = Medium)
                }
            }
            Select(Modifier.weight(3f), onPickAccount) {
                flow.receive.account?.let {
                    NetworkIcon(it.network, 24.dp)
                    Txt(it.name, Modifier.weight(1f), weight = Medium)
                }
            }
        }
        QrView(flow.receive.qr, frame = 0, total = 0, label = "收款二维码")
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s1)) {
            Row(
                Modifier.fillMaxWidth().heightIn(min = 44.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.s3),
            ) {
                Txt("地址", size = FontSize.t2, color = c.text3)
                if (flow.receive.hasNext) {
                    Txt(
                        "地址列表",
                        Modifier.tap(onClick = flow.receive::openAddresses),
                        size = FontSize.t2,
                        color = c.accentText,
                    )
                }
            }
            AddressChunks(flow.receive.address.orEmpty(), size = FontSize.t3, minLines = ADDRESS_LINES)
        }
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            flow.receive.account?.let { DetailRow("网络", networkName(it.network)) }
            // 启用 BIP39 密码时只有这里能核对是否输错,输错不会报错.
            if (flow.session.passphraseRequired) DetailRow("主密钥指纹", flow.session.fingerprint.orEmpty(), mono = true)
        }
    }
}

/** 顶部的资产或网络选择,点开底部弹层.弹层放在 [MainTabs],要盖住导航. */
@Composable
private fun Select(modifier: Modifier, onClick: () -> Unit, content: @Composable RowScope.() -> Unit) {
    val c = BitOneTheme.colors
    Card(modifier) {
        ListRow(onClick = onClick) {
            content()
            LineIcon(Icons.Chevron, 16.dp, c.text3)
        }
    }
}

/** 资产列表.选中后回到收款页,收不了这个币的账户跟着换. */
@Composable
internal fun BoxScope.AssetSheet(visible: Boolean, flow: WalletFlow, onClose: () -> Unit) {
    Sheet(flow, visible, "资产", onClose, list = true) {
        Column {
            flow.receive.tokens.forEach { entry ->
                ListRow(
                    selected = entry.icon == flow.receive.asset?.icon,
                    inset = Space.s1,
                    onClick = {
                        flow.receive.pickAsset(entry)
                        onClose()
                    },
                ) {
                    AssetIcon(entry.icon, null, 24.dp)
                    Txt(entry.symbol, Modifier.weight(1f), weight = Medium)
                }
            }
        }
    }
}

/** 能收当前资产的账户.选中后回到收款页. */
@Composable
internal fun BoxScope.AccountSheet(visible: Boolean, flow: WalletFlow, onClose: () -> Unit) {
    Sheet(flow, visible, "网络", onClose, list = true) {
        Column {
            flow.receive.accounts.forEach { account ->
                ListRow(
                    selected = account == flow.receive.account,
                    inset = Space.s1,
                    onClick = {
                        flow.receive.select(account)
                        onClose()
                    },
                ) {
                    NetworkIcon(account.network, 24.dp)
                    Txt(account.name, Modifier.weight(1f), weight = Medium)
                }
            }
        }
    }
}

/** A11b 发放过的地址.点一行回到收款页显示它的二维码,序号不变."使用新地址"在这里,发完回到收款页. */
@Composable
fun ReceiveAddressesScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    Page(
        header = { TitleBar("地址列表", onBack = { flow.back() }) },
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                PrimaryButton(
                    "使用新地址",
                    enabled = !flow.busy && flow.receive.address != null,
                    onClick = flow.receive::nextAddress,
                )
            }
        },
    ) {
        Card {
            flow.receive.history.forEachIndexed { index, address ->
                ListRow(selected = address == flow.receive.address, onClick = { flow.receive.show(address) }) {
                    Txt(shortAddress(address), Modifier.weight(1f), mono = true)
                    if (index == 0) Chip("最新", c.accentSoft, c.accentText)
                }
            }
        }
        Notice("使用新地址后,旧地址仍然可以收款.")
    }
}

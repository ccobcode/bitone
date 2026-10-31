package com.bitone.wallet.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.AccountRow
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.flow.shortAddress
import com.bitone.wallet.platform.Brightness
import com.bitone.wallet.ui.components.AddressChunks
import com.bitone.wallet.ui.components.HeaderIconButton
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.LargeTitle
import com.bitone.wallet.ui.components.LineIcon
import com.bitone.wallet.ui.components.ListGroup
import com.bitone.wallet.ui.components.ListRow
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.NetworkIcon
import com.bitone.wallet.ui.components.NoticePill
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.QrView
import com.bitone.wallet.ui.components.SecondaryButton
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Space

/** 最长的地址占四行，短地址也留出同样的高度，换账户时下面的内容不跳。 */
private const val ADDRESS_LINES = 4

/** A11 收款。地址从本机公钥算出，不经过网页端，也不显示派生路径。亮度调到最高。冷钱包不用剪贴板，所以没有复制按钮。 */
@Composable
fun ReceiveScreen(flow: WalletFlow, brightness: Brightness, onPickAccount: () -> Unit) {
    val c = BitOneTheme.colors
    DisposableEffect(Unit) {
        if (flow.autoBrightness) brightness.maximize()
        onDispose { brightness.restore() }
    }
    Page(
        header = {
            LargeTitle("收款") {
                if (flow.receiveHasNext) {
                    HeaderIconButton("地址列表", Icons.History, flow::openReceiveAddresses)
                }
            }
        },
        floatingSpace = NavSpace,
        contentGap = Space.s4,
        contentAlignment = Alignment.CenterHorizontally,
        overlay = { NoticePill(flow.notice) },
    ) {
        AccountSelect(flow.receiveAccount, onPickAccount)
        QrView(flow.receiveQr, frame = 0, total = 0, label = "收款二维码")
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            AddressChunks(flow.receiveAddress.orEmpty(), minLines = ADDRESS_LINES)
            Txt("只向这个网络转账", size = FontSize.t2, color = c.text3)
            // 启用 BIP39 密码时只有这里能核对是否输错，输错不会报错。
            if (flow.passphraseRequired) DetailRow("主密钥指纹", flow.fingerprint.orEmpty(), mono = true)
        }
        if (flow.receiveHasNext) {
            SecondaryButton(
                "使用新地址",
                enabled = !flow.busy && flow.receiveAddress != null,
                onClick = flow::nextReceiveAddress,
            )
            Txt("使用新地址后，旧地址仍然可以收款", size = FontSize.t2, color = c.text3)
        }
    }
}

/** 顶部的账户选择，点开底部弹层。弹层放在 [MainTabs]，要盖住导航。 */
@Composable
private fun AccountSelect(account: AccountRow?, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    ListGroup {
        ListRow(onClick = onClick) {
            account?.let {
                NetworkIcon(it.network, 24.dp)
                Txt(it.name, Modifier.weight(1f), weight = Medium)
            }
            LineIcon(Icons.Down, 18.dp, c.text3)
        }
    }
}

/** 账户列表。选中后回到收款页。 */
@Composable
internal fun BoxScope.AccountSheet(visible: Boolean, flow: WalletFlow, onClose: () -> Unit) {
    val c = BitOneTheme.colors
    BottomSheet(visible, onClose, padding = Space.s2) {
        Column {
            flow.accounts.forEach { account ->
                ListRow(
                    onClick = {
                        flow.selectReceive(account)
                        onClose()
                    }
                ) {
                    NetworkIcon(account.network, 24.dp)
                    Txt(account.name, Modifier.weight(1f), weight = Medium)
                    if (account == flow.receiveAccount) LineIcon(Icons.Check, 18.dp, c.accentText)
                }
            }
        }
    }
}

/** A11b 发放过的地址。点一行回到收款页显示它的二维码，序号不变。 */
@Composable
fun ReceiveAddressesScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    Page(header = { TitleBar("地址列表", onBack = { flow.back() }) }) {
        ListGroup {
            flow.receiveHistory.forEach { address ->
                ListRow(onClick = { flow.showReceive(address) }) {
                    Txt(shortAddress(address), Modifier.weight(1f), mono = true)
                    if (address == flow.receiveAddress) LineIcon(Icons.Check, 18.dp, c.accentText)
                }
            }
        }
    }
}

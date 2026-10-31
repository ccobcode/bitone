package com.bitone.wallet.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.AccountRow
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.flow.shortAddress
import com.bitone.wallet.ui.components.Chip
import com.bitone.wallet.ui.components.Icons
import com.bitone.wallet.ui.components.LargeTitle
import com.bitone.wallet.ui.components.LineIcon
import com.bitone.wallet.ui.components.ListGroup
import com.bitone.wallet.ui.components.ListRow
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.NetworkIcon
import com.bitone.wallet.ui.components.NoticePill
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.SemiBold
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.staggerIn
import com.bitone.wallet.ui.components.tap
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Space
import com.bitone.wallet.ui.theme.elevation

private val TileHeight = 120.dp
private val AccountIcon = 32.dp

/** 主操作和次操作的宽度比，扫码要明显更大。 */
private const val SCAN_WEIGHT = 3f
private const val CONNECT_WEIGHT = 2f

/** A3 首页。账户第二行是短地址，派生路径只在核对页出现。 */
@Composable
fun HomeScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    LaunchedEffect(Unit) { flow.loadAddresses() }
    Page(
        header = { HomeHeader(flow) },
        floatingSpace = NavSpace,
        overlay = { NoticePill(flow.notice) },
    ) {
        Row(Modifier.fillMaxWidth().height(TileHeight), horizontalArrangement = Arrangement.spacedBy(Space.s3)) {
            ActionTile(
                "扫码",
                Icons.Scan,
                32.dp,
                FontSize.t5,
                c.accent,
                c.onAccent,
                Modifier.weight(SCAN_WEIGHT),
                flow::openScan,
            )
            ActionTile(
                "连接网页端",
                Icons.Qr,
                24.dp,
                FontSize.t3,
                c.surface,
                c.text,
                Modifier.weight(CONNECT_WEIGHT),
                flow::openExport,
            )
        }
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
            Txt("账户", size = FontSize.t2, color = c.text3)
            ListGroup {
                flow.accounts.forEachIndexed { index, account ->
                    AccountItem(account, flow.addresses[account.path], Modifier.staggerIn(index)) {
                        flow.openReceive(account)
                    }
                }
            }
        }
    }
}

@Composable
private fun HomeHeader(flow: WalletFlow) {
    val c = BitOneTheme.colors
    Column(Modifier.fillMaxWidth().padding(bottom = Space.s3)) {
        LargeTitle("钱包")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
            val fingerprint = flow.fingerprint
            if (fingerprint != null) {
                Txt(fingerprint, size = FontSize.t2, color = c.text2, mono = true)
            } else {
                Txt("解锁后显示指纹", size = FontSize.t2, color = c.text3)
            }
            Chip("离线", c.okSoft, c.ok, icon = Icons.Plane)
        }
    }
}

@Composable
private fun AccountItem(account: AccountRow, address: String?, modifier: Modifier, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    ListRow(modifier, onClick = onClick) {
        NetworkIcon(account.network, AccountIcon)
        Column(Modifier.weight(1f)) {
            Txt(account.name, weight = Medium)
            if (address != null) Txt(shortAddress(address), size = FontSize.t2, color = c.text3, mono = true)
        }
        LineIcon(Icons.Chevron, 18.dp, c.text3, strokeWidth = 2f)
    }
}

@Composable
private fun ActionTile(
    label: String,
    icon: String,
    iconSize: Dp,
    textSize: TextUnit,
    background: Color,
    foreground: Color,
    modifier: Modifier,
    onClick: () -> Unit,
) {
    val shape = RoundedCornerShape(Radius.r4)
    Column(
        modifier
            .tap(onClick = onClick)
            .fillMaxHeight()
            .elevation(BitOneTheme.elevation.elevation1, shape)
            .background(background, shape)
            .padding(Space.s5),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        LineIcon(icon, iconSize, foreground)
        Txt(label, size = textSize, weight = if (textSize == FontSize.t5) SemiBold else Medium, color = foreground)
    }
}

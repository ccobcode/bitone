package com.bitone.wallet.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.ScanResult
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.flow.networkInfo
import com.bitone.wallet.flow.shortAddress
import com.bitone.wallet.platform.ManifestEntry
import com.bitone.wallet.platform.NATIVE_CONTRACT
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.AssetIcon
import com.bitone.wallet.ui.components.Chip
import com.bitone.wallet.ui.components.ListGroup
import com.bitone.wallet.ui.components.ListRow
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.NoticePill
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.SemiBold
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Space

/** 版本号本身是只增不减的整数，防回滚和验签都用它，这里只管显示。 */
internal fun manifestLabel(version: UInt) = "v$version.0.0"

/** A12 内置清单。「更新清单」才打开相机，扫到新包后回到这页，确认时要自检和 PIN。 */
@Composable
fun ManifestScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val pending = (flow.scanResult as? ScanResult.Manifest)?.version
    LaunchedEffect(Unit) { flow.loadManifest() }
    Page(
        header = { TitleBar("内置清单", onBack = { flow.back() }) },
        bottom = {
            ActionArea {
                if (pending != null) {
                    PrimaryButton("确认更新", enabled = !flow.busy, onClick = flow::applyManifest)
                } else {
                    PrimaryButton("更新清单", enabled = !flow.busy, onClick = flow::openScan)
                }
            }
        },
        overlay = { NoticePill(flow.notice) },
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s3)) {
            flow.manifestVersion?.let { Txt(manifestLabel(it), size = FontSize.t6, weight = SemiBold) }
            if (pending != null) Chip("新版本 ${manifestLabel(pending)}", c.accentSoft, c.accentText)
        }
        flow.manifestEntries
            .groupBy { it.networkLabel }
            .forEach { (label, entries) ->
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
                    Txt(label, size = FontSize.t2, color = c.text3)
                    ListGroup { entries.forEach { EntryRow(it) } }
                }
            }
    }
}

@Composable
private fun EntryRow(entry: ManifestEntry) {
    val c = BitOneTheme.colors
    ListRow {
        AssetIcon(entry.icon, networkInfo(entry.network)?.icon, 32.dp)
        Column(Modifier.weight(1f)) {
            Txt(entry.symbol, weight = Medium)
            if (entry.contract != NATIVE_CONTRACT) {
                Txt(shortAddress(entry.contract), size = FontSize.t2, color = c.text3, mono = true)
            }
        }
    }
}

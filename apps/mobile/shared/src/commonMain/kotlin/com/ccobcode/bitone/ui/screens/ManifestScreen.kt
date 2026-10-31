package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.ScanResult
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.networkInfo
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.ManifestEntry
import com.ccobcode.bitone.platform.NATIVE_CONTRACT
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.AssetIcon
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.SemiBold
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Toast
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space

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
        overlay = { Toast(flow) },
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
                    Card { entries.forEach { EntryRow(it) } }
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

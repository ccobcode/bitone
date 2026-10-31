package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.networkInfo
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.AddressCard
import com.ccobcode.bitone.ui.components.DrawnCheck
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.NetworkIcon
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.QrFullscreen
import com.ccobcode.bitone.ui.components.QrPlayer
import com.ccobcode.bitone.ui.components.SemiBold
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Toast
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.rememberLatest
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space

private const val LABEL = "已签名交易二维码"

/** A8 已签名.帧里只有签名,种子已经清除. */
@Composable
fun SignedScreen(flow: WalletFlow, brightness: Brightness) {
    val review = rememberLatest(flow.scan.review) ?: return
    val c = BitOneTheme.colors
    var fullscreen by remember { mutableStateOf(false) }
    ShowingQr(flow, brightness)
    LaunchedEffect(Unit) { flow.player.play() }
    Page(
        header = {
            TitleBar(
                "已签名",
                onBack = { flow.back() },
                backIcon = Icons.Close,
                leading = { DrawnCheck(24.dp, c.ok, c.okSoft) },
            )
        },
        contentAlignment = Alignment.CenterHorizontally,
        contentGap = Space.s4,
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow.scan::finishSigned) } },
        overlay = {
            Toast(flow)
            QrFullscreen(flow, LABEL, fullscreen) { fullscreen = it }
        },
    ) {
        QrPlayer(flow, LABEL, fullscreen) { fullscreen = it }
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            amountLine(review).takeIf { it.isNotEmpty() }?.let { DetailRow("金额", it) }
            review.counterparty?.let { DetailRow(review.counterpartyLabel.orEmpty(), shortAddress(it), mono = true) }
            review.rows
                .firstOrNull { it.label == "付款账户" }
                ?.let {
                    DetailRow("签名账户", shortAddress(it.value), mono = true)
                }
        }
    }
}

/** A9 核对地址. */
@Composable
fun AddressScreen(flow: WalletFlow) {
    val result = rememberLatest(flow.scan.addressReview) ?: return
    val c = BitOneTheme.colors
    val info = networkInfo(result.network)
    val tone = if (result.belongs) c.ok else c.danger
    Page(
        header = { TitleBar("核对地址", onBack = { flow.back() }, backIcon = Icons.Close) },
        bottom = { ActionArea { PrimaryButton("完成", onClick = flow.scan::exitRequest) } },
    ) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            if (result.belongs) {
                DrawnCheck(48.dp, c.surface, tone, ring = tone, strokeWidth = 2.4f)
            } else {
                Box(Modifier.size(48.dp).background(tone, CircleShape), contentAlignment = Alignment.Center) {
                    LineIcon(Icons.Close, 28.dp, c.surface)
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(Space.s1)) {
                Txt(if (result.belongs) "属于本钱包" else "不属于本钱包", size = FontSize.t6, weight = SemiBold, color = tone)
                Txt(if (result.belongs) "与网页端逐段对比后再收款." else "不要用这个地址收款.", size = FontSize.t2, color = c.text2)
            }
        }
        AddressCard(
            "二维码里的地址",
            result.address,
            leading = { if (info != null) NetworkIcon(info.icon, 24.dp) },
        )
        if (!result.belongs) AddressCard("本机从这条路径算出的地址", result.derived)
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            DetailRow("路径", result.path, mono = true)
            DetailRow("网络", info?.name ?: result.network)
            DetailRow("地址类型", result.kind)
        }
    }
}

package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.CheckState
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.BrandMark
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.SemiBold
import com.ccobcode.bitone.ui.components.TextButton
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.staggerIn
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space

private class StateRow(val label: String, val value: String, val color: Color, val mono: Boolean = false)

/** AS 异常态的共用版式：标题、64 的图标圆、说明、明细列表组、底部按钮。 */
@Composable
private fun StatePage(
    flow: WalletFlow,
    title: String,
    icon: String,
    heading: String,
    body: String,
    action: String,
    onAction: () -> Unit,
    warn: Boolean = false,
    rows: List<StateRow> = emptyList(),
    secondary: String? = null,
    onSecondary: () -> Unit = {},
    notes: List<String> = emptyList(),
) {
    val c = BitOneTheme.colors
    Page(
        header = { TitleBar(title, onBack = null) },
        contentAlignment = Alignment.CenterHorizontally,
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                PrimaryButton(action, enabled = !flow.busy, onClick = onAction)
                if (secondary != null) TextButton(secondary, enabled = !flow.busy, onClick = onSecondary)
            }
        },
    ) {
        Box(
            Modifier.padding(top = Space.s4)
                .size(64.dp)
                .background(if (warn) c.warnSoft else c.dangerSoft, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            LineIcon(icon, 32.dp, if (warn) c.warn else c.danger)
        }
        Txt(heading, size = FontSize.t5, weight = SemiBold, align = TextAlign.Center)
        Txt(body, color = c.text2, align = TextAlign.Center)
        if (rows.isNotEmpty()) {
            Card {
                rows.forEachIndexed { index, row ->
                    ListRow(Modifier.staggerIn(index)) {
                        Txt(row.label, Modifier.weight(1f), size = FontSize.t2, color = c.text3)
                        Txt(row.value, size = FontSize.t2, color = row.color, mono = row.mono, align = TextAlign.End)
                    }
                }
            }
        }
        if (notes.isNotEmpty()) {
            Card {
                Column(Modifier.padding(vertical = Space.s2)) {
                    notes.forEachIndexed { index, note ->
                        Box(
                            Modifier.staggerIn(index)
                                .fillMaxWidth()
                                .heightIn(min = 40.dp)
                                .padding(horizontal = Space.s4),
                            contentAlignment = Alignment.CenterStart,
                        ) {
                            Txt(note, size = FontSize.t2)
                        }
                    }
                }
            }
        }
    }
}

/** AS 自检失败。 */
@Composable
fun SelfCheckFailedScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val rows =
        flow.report?.items.orEmpty().map { item ->
            val color =
                when (item.state) {
                    CheckState.Pass -> c.text3
                    CheckState.Warn -> c.warn
                    CheckState.Fail -> c.danger
                }
            StateRow(item.label, item.value, color)
        }
    StatePage(
        flow = flow,
        title = "离线自检",
        icon = Icons.Alert,
        heading = "签名已锁定",
        body = "处理下面的问题后重新检测。",
        rows = rows,
        action = "重新检测",
        onAction = flow::retryCheck,
    )
}

/** AS 指纹不匹配。只有启用 BIP39 密码的钱包才能重新输入，其他钱包只能换一个二维码。 */
@Composable
fun FingerprintMismatchScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val passphrase = flow.passphraseRequired
    StatePage(
        flow = flow,
        title = "扫描",
        icon = Icons.Key,
        heading = "这笔请求不属于本钱包",
        body = if (passphrase) "启用了 BIP39 密码时，请检查是否输错。" else "请在网页端换成本钱包再发起。",
        rows =
            listOfNotNull(
                flow.fingerprint?.let { StateRow("本机指纹", it, c.text, mono = true) },
                flow.mismatchFingerprint?.let { StateRow("请求指纹", it, c.danger, mono = true) },
            ),
        action = if (passphrase) "重新输入" else "重新扫描",
        onAction = if (passphrase) flow::retryPassphrase else flow::rescan,
        secondary = "退出",
        onSecondary = flow::exitRequest,
    )
}

/** AS 解析失败。 */
@Composable
fun ParseFailedScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    StatePage(
        flow = flow,
        title = "扫描",
        icon = Icons.Unknown,
        warn = true,
        heading = "无法解析这个二维码",
        body = "数据不完整，或格式不受支持。",
        rows = flow.scanFailure.takeIf { it.isNotEmpty() }?.let { listOf(StateRow("原因", it, c.warn)) }.orEmpty(),
        action = "重新扫描",
        onAction = flow::rescan,
        secondary = "退出",
        onSecondary = flow::exitRequest,
    )
}

private val ForbiddenRules =
    listOf(
        "没有 Chain ID 的旧式交易",
        "chain_id 为 0 的 EIP-7702 授权",
        "裸哈希、消息签名和未支持的 EIP-712",
        "路径不在白名单，或与网络冲突",
        "地址与路径不符",
        "缺少必需的前序交易",
    )

/** AS 禁止签名。[WalletFlow.refusal] 是核心库给的实际原因，下面是全部规则。 */
@Composable
fun ForbiddenScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    StatePage(
        flow = flow,
        title = "核对交易",
        icon = Icons.Refused,
        heading = "这笔请求不能签名",
        body = "触发以下任何一项时，冷钱包拒绝签名。",
        rows = listOf(StateRow("原因", flow.refusal, c.danger)),
        action = "退出",
        onAction = flow::exitRequest,
        notes = ForbiddenRules,
    )
}

/** AS 摄像头被拒。相机打不开时不用去系统设置，重新打开一次。 */
@Composable
fun CameraDeniedScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val broken = flow.cameraBroken
    StatePage(
        flow = flow,
        title = "扫描",
        icon = Icons.CameraOff,
        warn = true,
        heading = if (broken) "摄像头打不开" else "需要摄像头权限",
        body = "扫码是这台手机唯一的输入方式。",
        action = if (broken) "重新打开" else "打开设置",
        onAction = if (broken) flow::rescan else flow::openCameraSettings,
        secondary = "退出",
        onSecondary = flow::exitRequest,
    )
}

/** AS 需恢复。 */
@Composable
fun NeedsRecoveryScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    StatePage(
        flow = flow,
        title = "钱包",
        icon = Icons.Key,
        heading = "钱包需要恢复",
        body = "屏幕锁被移除后，硬件密钥已失效。",
        rows =
            listOf(
                StateRow("第 1 步", "重新设置屏幕锁", c.text),
                StateRow("第 2 步", "用助记词恢复", c.text),
            ),
        action = "导入助记词",
        onAction = flow::startImport,
    )
}

/** 错误 10 次之后。 */
@Composable
fun WipedScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    StatePage(
        flow = flow,
        title = "钱包",
        icon = Icons.Alert,
        heading = "钱包已抹除",
        body = "PIN 错误达到 10 次，可用助记词恢复。",
        action = "重新开始",
        onAction = flow::restartAfterWipe,
    )
}

/** 第一次自检完成之前。 */
@Composable
fun BootScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    Page(
        contentAlignment = Alignment.CenterHorizontally,
        contentAnchor = Alignment.CenterVertically,
        contentGap = Space.s4,
    ) {
        BrandMark(56.dp)
        Txt("离线自检", color = c.text3)
    }
}

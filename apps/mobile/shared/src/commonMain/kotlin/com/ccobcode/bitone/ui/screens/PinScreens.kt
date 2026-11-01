package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.PIN_LENGTH
import com.ccobcode.bitone.flow.UnlockPurpose
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.FadeText
import com.ccobcode.bitone.ui.components.Icons
import com.ccobcode.bitone.ui.components.LineIcon
import com.ccobcode.bitone.ui.components.ListRow
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PassphraseKeyboard
import com.ccobcode.bitone.ui.components.PinPad
import com.ccobcode.bitone.ui.components.SemiBold
import com.ccobcode.bitone.ui.components.Steps
import com.ccobcode.bitone.ui.components.TextButton
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.rememberLatest
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space
import kotlinx.coroutines.delay

/** 标题,圆点,提示成一组贴近键盘,上面的留白由弹性间距吸收.A2c,A7,修改 PIN,解锁都用这一个版式. */
@Composable
private fun PinPage(
    prompt: String,
    length: Int,
    errors: Int,
    hint: String,
    hintColor: Color,
    header: @Composable ColumnScope.() -> Unit,
    above: @Composable (() -> Unit)? = null,
    below: @Composable (() -> Unit)? = null,
    keyboard: @Composable ColumnScope.() -> Unit,
) {
    Page(
        header = header,
        contentAlignment = Alignment.CenterHorizontally,
        contentAnchor = Alignment.Bottom,
        contentGap = Space.s3,
        bottom = keyboard,
    ) {
        above?.invoke()
        Txt(prompt, size = FontSize.t5, weight = SemiBold)
        PinDots(length, errors)
        FadeText(hint, hintColor)
        below?.invoke()
    }
}

/** A2c 设置 PIN,输入两次. */
@Composable
fun SetPinScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val done = flow.pin.done
    val (note, noteColor) =
        when {
            flow.pin.error != null -> flow.pin.error.orEmpty() to c.danger
            flow.pin.weak -> "PIN 太简单,请换一个" to c.danger
            flow.pin.mismatch -> "两次不一致,请重新设置" to c.danger
            flow.busy -> "正在加密" to c.text3
            done && flow.settings.changingPin -> "下一步输入当前 PIN" to c.text3
            done -> "PIN 已设置" to c.ok
            else -> "" to c.text3
        }
    val prompt =
        when {
            flow.pin.confirming -> "再输一次"
            flow.settings.changingPin -> "输入新 PIN"
            else -> "输入 6 位 PIN"
        }
    PinPage(
        prompt = prompt,
        length = flow.pin.input.length,
        errors = if (flow.pin.mismatch || flow.pin.weak) 1 else 0,
        hint = note,
        hintColor = noteColor,
        header = {
            TitleBar(if (flow.settings.changingPin) "修改 PIN" else "设置 PIN", onBack = { flow.back() }) {
                if (!flow.setup.isImportFlow && !flow.settings.changingPin) Steps(3)
            }
        },
        keyboard = {
            PinPad(
                enabled = !flow.busy,
                onDigit = flow.pin::digit,
                onDelete = flow.pin::delete,
                action =
                    when {
                        !done -> null
                        flow.settings.changingPin -> "下一步"
                        else -> "完成"
                    },
                actionEnabled = !flow.busy,
                onAction = if (flow.settings.changingPin) flow.settings::continueChangePin else flow.setup::finishSetup,
            )
        },
    )
}

private class UnlockCopy(val title: String, val prompt: String, val action: String, val working: String)

private fun unlockCopy(purpose: UnlockPurpose) =
    when (purpose) {
        UnlockPurpose.Sign -> UnlockCopy("确认签名", "输入 PIN", "签名", "正在签名")
        UnlockPurpose.ChangePin -> UnlockCopy("修改 PIN", "输入当前 PIN", "确认修改", "正在加密")
        UnlockPurpose.Wipe -> UnlockCopy("抹除钱包", "输入 PIN", "抹除", "正在抹除")
        UnlockPurpose.ApplyManifest -> UnlockCopy("更新清单", "输入 PIN", "更新", "正在写入")
        else -> UnlockCopy("解锁钱包", "输入 PIN", "解锁", "正在解锁")
    }

/** A7.每次签名都输 PIN,导出,读公钥,查看助记词,修改 PIN,抹除和更新清单也走这页. */
@Composable
fun UnlockScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    var editing by remember { mutableStateOf(false) }
    val waiting = flow.unlock.waitMs > 0
    val copy = unlockCopy(flow.unlock.purpose)
    // 只有签名时显示交易摘要.退场动画期间流程可能已经清掉请求,这时继续显示离开时的内容.
    val review = rememberLatest(flow.scan.review).takeIf { flow.unlock.purpose == UnlockPurpose.Sign }

    // 倒计时每秒重读单调时钟,界面只负责显示.
    LaunchedEffect(waiting) {
        while (flow.unlock.waitMs > 0) {
            delay(1_000)
            flow.unlock.refreshStatus()
        }
    }

    val (hint, hintColor) =
        when {
            flow.busy -> copy.working to c.text3
            flow.unlock.error != null -> flow.unlock.error.orEmpty() to c.danger
            waiting -> "${waitText(flow.unlock.waitMs)} 后可重试" to c.text2
            else -> (if (flow.unlock.failures == 0) "" else "已错 ${flow.unlock.failures} 次") to c.warn
        }
    PinPage(
        prompt = copy.prompt,
        length = flow.unlock.pin.length,
        errors = flow.unlock.failures,
        hint = hint,
        hintColor = hintColor,
        header = {
            TitleBar(copy.title, onBack = { flow.back() })
            if (review != null) SignSummary(review)
        },
        above =
            if (flow.unlock.needsPassphrase) {
                { PassphraseField(flow.unlock.passphrase, editing) { editing = !editing } }
            } else {
                null
            },
        // 延时中也能点:这时最需要这条路.硬件报错或密文损坏时重试永远不会成功,靠它出去.
        below = { TextButton("忘记 PIN,用助记词恢复", enabled = !flow.busy, onClick = flow::forgotPin) },
        keyboard = {
            if (editing) {
                Box(Modifier.fillMaxWidth().padding(top = Space.s3, bottom = Space.s4)) {
                    PassphraseKeyboard(flow.unlock::passphraseInput, flow.unlock::passphraseDelete) { editing = false }
                }
            } else {
                PinPad(
                    enabled = !waiting && !flow.busy,
                    onDigit = flow.unlock::digit,
                    onDelete = flow.unlock::delete,
                    action = copy.action.takeIf { flow.unlock.pin.length == PIN_LENGTH },
                    actionEnabled = flow.unlock.canSubmit,
                    onAction = flow.unlock::submit,
                )
            }
        },
    )
}

/** 向上取整,还剩 1 毫秒时也显示 1 秒. */
private fun waitText(ms: Long): String {
    val total = (ms + 999) / 1000
    val minutes = total / 60
    val seconds = total % 60
    return if (minutes > 0) "$minutes 分 $seconds 秒" else "$seconds 秒"
}

/** BIP39 密码输入行.键盘打开时明文显示,输错不会报错,只会得到另一个钱包,所以要让人看见. */
@Composable
private fun PassphraseField(value: String, editing: Boolean, onToggle: () -> Unit) {
    val c = BitOneTheme.colors
    Card {
        ListRow(onClick = onToggle) {
            Txt("BIP39 密码")
            Row(
                Modifier.weight(1f),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                when {
                    editing -> Txt(value, size = FontSize.t2, color = c.text2, mono = true)
                    value.isEmpty() -> Chip("未输入", c.warnSoft, c.warn)
                    else -> Chip("${value.length} 位", c.surface2, c.text2)
                }
            }
            LineIcon(Icons.Keyboard, 20.dp, c.text3)
        }
    }
}

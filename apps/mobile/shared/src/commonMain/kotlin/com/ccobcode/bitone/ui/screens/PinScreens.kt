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
import com.ccobcode.bitone.flow.MAX_PIN_FAILURES
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
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.rememberLatest
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Space
import kotlinx.coroutines.delay

/** 标题、圆点、提示成一组贴近键盘，上面的留白由弹性间距吸收。A2c、A7、修改 PIN、解锁都用这一个版式。 */
@Composable
private fun PinPage(
    prompt: String,
    length: Int,
    errors: Int,
    hint: String,
    hintColor: Color,
    header: @Composable ColumnScope.() -> Unit,
    above: @Composable (() -> Unit)? = null,
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
    }
}

/** A2c 设置 PIN，输入两次。 */
@Composable
fun SetPinScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val done = flow.pinDone
    val (note, noteColor) =
        when {
            flow.setupError != null -> flow.setupError.orEmpty() to c.danger
            flow.pinWeak -> "PIN 太简单，请换一个" to c.danger
            flow.pinMismatch -> "两次不一致，请重新设置" to c.danger
            flow.busy -> "正在加密" to c.text3
            done && flow.changingPin -> "下一步输入当前 PIN" to c.text3
            done -> "PIN 已设置" to c.ok
            else -> "" to c.text3
        }
    val prompt =
        when {
            flow.pinConfirming -> "再输一次"
            flow.changingPin -> "输入新 PIN"
            else -> "输入 6 位 PIN"
        }
    PinPage(
        prompt = prompt,
        length = flow.pinInput.length,
        errors = if (flow.pinMismatch || flow.pinWeak) 1 else 0,
        hint = note,
        hintColor = noteColor,
        header = {
            TitleBar(if (flow.changingPin) "修改 PIN" else "设置 PIN", onBack = { flow.back() }) {
                if (!flow.isImportFlow && !flow.changingPin) Steps(3)
            }
        },
        keyboard = {
            PinPad(
                enabled = !flow.busy,
                onDigit = flow::setPinDigit,
                onDelete = flow::setPinDelete,
                action =
                    when {
                        !done -> null
                        flow.changingPin -> "下一步"
                        else -> "完成"
                    },
                actionEnabled = !flow.busy,
                onAction = if (flow.changingPin) flow::continueChangePin else flow::finishSetup,
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

/** A7。每次签名都输 PIN，导出、读公钥、查看助记词、修改 PIN、抹除和更新清单也走这页。 */
@Composable
fun UnlockScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    var editing by remember { mutableStateOf(false) }
    val waiting = flow.waitMs > 0
    val copy = unlockCopy(flow.unlockPurpose)
    // 只有签名时显示交易摘要。退场动画期间流程可能已经清掉请求，这时继续显示离开时的内容。
    val review = rememberLatest(flow.review).takeIf { flow.unlockPurpose == UnlockPurpose.Sign }

    // 倒计时每秒重读单调时钟，界面只负责显示。
    LaunchedEffect(waiting) {
        while (flow.waitMs > 0) {
            delay(1_000)
            flow.refreshPinStatus()
        }
    }

    val (hint, hintColor) =
        when {
            flow.busy -> copy.working to c.text3
            flow.unlockError != null -> flow.unlockError.orEmpty() to c.danger
            waiting -> "${waitText(flow.waitMs)} 后可重试" to c.text2
            else -> failureHint(flow.pinFailures, flow.canWipe)
        }
    PinPage(
        prompt = copy.prompt,
        length = flow.unlockPin.length,
        errors = flow.pinFailures,
        hint = hint,
        hintColor = hintColor,
        header = {
            TitleBar(copy.title, onBack = { flow.back() })
            if (review != null) SignSummary(review)
        },
        above =
            if (flow.unlockNeedsPassphrase) {
                { PassphraseField(flow.unlockPassphrase, editing) { editing = !editing } }
            } else {
                null
            },
        keyboard = {
            if (editing) {
                Box(Modifier.fillMaxWidth().padding(top = Space.s3, bottom = Space.s4)) {
                    PassphraseKeyboard(flow::passphraseInput, flow::passphraseDelete) { editing = false }
                }
            } else {
                PinPad(
                    enabled = !waiting && !flow.busy,
                    onDigit = flow::unlockDigit,
                    onDelete = flow::unlockDelete,
                    action = copy.action.takeIf { flow.unlockPin.length == PIN_LENGTH },
                    actionEnabled = flow.canSubmitUnlock,
                    onAction = flow::submitUnlock,
                )
            }
        },
    )
}

@Composable
private fun failureHint(failures: Int, canWipe: Boolean): Pair<String, Color> {
    val c = BitOneTheme.colors
    val remain = MAX_PIN_FAILURES - failures
    return when {
        !canWipe -> (if (failures == 0) "" else "已错 $failures 次") to c.warn
        failures == 0 -> "错误 10 次将抹除钱包" to c.text3
        remain == 1 -> "再错 1 次将抹除钱包" to c.danger
        else -> "已错 $failures 次，再错 $remain 次将抹除钱包" to c.warn
    }
}

/** 向上取整，还剩 1 毫秒时也显示 1 秒。 */
private fun waitText(ms: Long): String {
    val total = (ms + 999) / 1000
    val minutes = total / 60
    val seconds = total % 60
    return if (minutes > 0) "$minutes 分 $seconds 秒" else "$seconds 秒"
}

/** BIP39 密码输入行。键盘打开时明文显示，输错不会报错，只会得到另一个钱包，所以要让人看见。 */
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

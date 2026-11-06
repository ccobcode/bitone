package com.ccobcode.bitone.ui.screens

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import com.ccobcode.bitone.flow.PIN_LENGTH
import com.ccobcode.bitone.flow.PassphraseEntry
import com.ccobcode.bitone.flow.UnlockPurpose
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.FadeText
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
import com.ccobcode.bitone.ui.theme.Size
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

/** A7.每次签名都输 PIN,导出,读公钥,查看助记词,修改 PIN,抹除和更新清单也走这页.旧格式的 BIP39 密码钱包先输两遍密码. */
@Composable
fun UnlockScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
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

    if (flow.unlock.asksPassphrase) {
        PassphrasePage(
            entry = flow.passphrase,
            hint = if (flow.unlock.passphraseOptional) "没有设置过就留空" else "密码会存进本机,以后只输 PIN",
            header = { TitleBar(copy.title, onBack = { flow.back() }) },
            onDone = flow.unlock::passphraseDone,
        )
        return
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
        // 延时中也能点:这时最需要这条路.硬件报错或密文损坏时重试永远不会成功,靠它出去.
        below = { TextButton("忘记 PIN,用助记词恢复", enabled = !flow.busy, onClick = flow::forgotPin) },
        keyboard = {
            PinPad(
                enabled = !waiting && !flow.busy,
                onDigit = flow.unlock::digit,
                onDelete = flow.unlock::delete,
                action = copy.action.takeIf { flow.unlock.pin.length == PIN_LENGTH },
                actionEnabled = flow.unlock.canSubmit,
                onAction = flow.unlock::submit,
            )
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

/** A2d 建钱包和导入时的 BIP39 密码. */
@Composable
fun PassphraseScreen(flow: WalletFlow) =
    PassphrasePage(
        entry = flow.passphrase,
        hint = "",
        header = { TitleBar("BIP39 密码", onBack = { flow.back() }) },
        onDone = flow.setup::passphraseDone,
    )

/**
 * BIP39 密码输两次,A2d 和 A7 的迁移共用.键盘的"完成"交一遍.
 *
 * 明文显示:输错不会报错,只会得到另一个钱包,要看得见才能和纸上的备份对上.密码不是文案,不翻译.
 */
@Composable
private fun PassphrasePage(
    entry: PassphraseEntry,
    hint: String,
    header: @Composable ColumnScope.() -> Unit,
    onDone: () -> Unit,
) {
    val c = BitOneTheme.colors
    Page(
        header = header,
        contentAlignment = Alignment.CenterHorizontally,
        contentAnchor = Alignment.Bottom,
        contentGap = Space.s3,
        bottom = {
            Box(Modifier.fillMaxWidth().padding(top = Space.s3, bottom = Space.s4)) {
                PassphraseKeyboard(entry::type, entry::delete, onDone)
            }
        },
    ) {
        Txt(if (entry.confirming) "再输一次" else "输入 BIP39 密码", size = FontSize.t5, weight = SemiBold)
        Card {
            Box(
                Modifier.fillMaxWidth().heightIn(min = Size.hInput).padding(horizontal = Space.s4),
                contentAlignment = Alignment.CenterStart,
            ) {
                Txt(entry.input, mono = true, translated = false)
            }
        }
        val (note, noteColor) = if (entry.mismatch) "两次不一致,请重新输入" to c.danger else hint to c.text3
        FadeText(note, noteColor)
    }
}

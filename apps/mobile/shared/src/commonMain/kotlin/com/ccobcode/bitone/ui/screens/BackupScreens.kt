package com.ccobcode.bitone.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.ccobcode.bitone.flow.VerifyState
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.QuizItem
import com.ccobcode.bitone.ui.components.ActionArea
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.Notice
import com.ccobcode.bitone.ui.components.NoticeTone
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.Segmented
import com.ccobcode.bitone.ui.components.Steps
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.stateSpec
import com.ccobcode.bitone.ui.components.tap
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Size
import com.ccobcode.bitone.ui.theme.Space

/** 新建只用这两种长度,导入接受 BIP39 的全部长度. */
internal val WordCountOptions = listOf(12 to "12 词", 24 to "24 词")
internal val ImportWordCountOptions = listOf(12 to "12 词", 15 to "15 词", 18 to "18 词", 21 to "21 词", 24 to "24 词")

/** A2 备份助记词. */
@Composable
fun BackupScreen(flow: WalletFlow) {
    val rowHeight = if (flow.setup.wordCount == 12) 40.dp else 32.dp
    val loading = flow.setup.backupWords.isEmpty()
    Page(
        header = { TitleBar("备份助记词", onBack = { flow.back() }) { Steps(1) } },
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                PrimaryButton(
                    "开始验证",
                    enabled = !flow.busy && flow.setup.backupWords.isNotEmpty(),
                    onClick = flow.setup::startVerify,
                )
            }
        },
    ) {
        Segmented(WordCountOptions, flow.setup.wordCount, flow.setup::setBackupWordCount)
        Notice("只抄在纸上", NoticeTone.Warn)
        WordGrid(if (loading) List(flow.setup.wordCount) { "" } else flow.setup.backupWords, rowHeight, loading)
        PassphraseSwitch(flow.setup.passphraseEnabled, flow.setup::togglePassphrase)
    }
}

/** A2b 验证助记词.全部答完才判定,判错后只能回 A2. */
@Composable
fun VerifyScreen(flow: WalletFlow) {
    Page(
        header = { TitleBar("验证助记词", onBack = { flow.back() }) { Steps(2) } },
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                when (flow.setup.verifyState) {
                    VerifyState.Passed -> PrimaryButton("继续", onClick = flow.setup::continueAfterVerify)
                    VerifyState.Failed -> PrimaryButton("重新抄写", onClick = flow.setup::backToBackup)
                    VerifyState.Pending -> PrimaryButton("继续", enabled = false) {}
                }
            }
        },
    ) {
        Notice("对照纸上的备份,选出对应序号的词")
        flow.setup.quiz.forEach { item ->
            QuizCard(item, flow.setup.picks[item.index], flow.setup.verifyState != VerifyState.Failed) {
                flow.setup.pick(item.index, it)
            }
        }
        AnimatedVisibility(
            flow.setup.verifyState == VerifyState.Failed,
            enter = fadeIn(stateSpec()) + expandVertically(stateSpec()),
        ) {
            Notice("有词选错了,回到上一步重新核对备份.", NoticeTone.Danger)
        }
    }
}

/** Rust 的 quiz 每题给 4 个选项,原型画的是 3 个,这里排成两行两列. */
@Composable
private fun QuizCard(item: QuizItem, picked: String?, enabled: Boolean, onPick: (String) -> Unit) {
    Card {
        Column(Modifier.padding(Space.s4), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            Txt("第 ${item.index + 1} 个词", weight = Medium)
            item.options.chunked(2).forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(Space.s2)) {
                    row.forEach { word ->
                        QuizOption(word, word == picked, enabled, Modifier.weight(1f)) { onPick(word) }
                    }
                }
            }
        }
    }
}

@Composable
private fun QuizOption(word: String, on: Boolean, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val c = BitOneTheme.colors
    val plate by animateColorAsState(if (on) c.accentSoft else c.surface2, stateSpec(Motion.fast), label = "option")
    val text by animateColorAsState(if (on) c.accentText else c.text, stateSpec(Motion.fast), label = "word")
    Box(
        modifier.tap(enabled, onClick = onClick).height(Size.hInput).background(plate, RoundedCornerShape(Radius.r2)),
        contentAlignment = Alignment.Center,
    ) {
        Txt(word, weight = Medium, color = text)
    }
}

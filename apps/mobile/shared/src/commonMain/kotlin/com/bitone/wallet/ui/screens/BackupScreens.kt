package com.bitone.wallet.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.VerifyState
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.platform.QuizItem
import com.bitone.wallet.ui.components.ActionArea
import com.bitone.wallet.ui.components.Card
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.Segmented
import com.bitone.wallet.ui.components.SemiBold
import com.bitone.wallet.ui.components.StepChip
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.stateSpec
import com.bitone.wallet.ui.components.tap
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Size
import com.bitone.wallet.ui.theme.Space

internal val WordCountOptions = listOf(12 to "12 词", 24 to "24 词")

/** A2 备份助记词。 */
@Composable
fun BackupScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val rowHeight = if (flow.wordCount == 12) 40.dp else 32.dp
    val loading = flow.backupWords.isEmpty()
    Page(
        header = { TitleBar("备份助记词", onBack = { flow.back() }) { StepChip("1/3") } },
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                PrimaryButton(
                    "开始验证",
                    enabled = !flow.busy && flow.backupWords.isNotEmpty(),
                    onClick = flow::startVerify,
                )
            }
        },
    ) {
        Segmented(WordCountOptions, flow.wordCount, flow::setBackupWordCount)
        Txt("只抄在纸上", size = FontSize.t2, color = c.text3)
        WordGrid(if (loading) List(flow.wordCount) { "" } else flow.backupWords, rowHeight, loading)
        PassphraseSwitch(flow.passphraseEnabled, flow::togglePassphrase)
    }
}

/** A2b 验证助记词。全部答完才判定，判错后只能回 A2。 */
@Composable
fun VerifyScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    Page(
        header = { TitleBar("验证助记词", onBack = { flow.back() }) { StepChip("2/3") } },
        contentGap = Space.s4,
        bottom = {
            ActionArea {
                when (flow.verifyState) {
                    VerifyState.Passed -> PrimaryButton("继续", onClick = flow::continueAfterVerify)
                    VerifyState.Failed -> PrimaryButton("重新抄写", onClick = flow::backToBackup)
                    VerifyState.Pending -> PrimaryButton("继续", enabled = false) {}
                }
            }
        },
    ) {
        Txt("对照纸上的备份，选出对应序号的词", size = FontSize.t2, color = c.text3)
        flow.quiz.forEach { item ->
            QuizCard(item, flow.picks[item.index], flow.verifyState != VerifyState.Failed) { flow.pick(item.index, it) }
        }
        AnimatedVisibility(
            flow.verifyState == VerifyState.Failed,
            enter = fadeIn(stateSpec()) + expandVertically(stateSpec()),
        ) {
            Column(
                Modifier.fillMaxWidth()
                    .background(c.dangerSoft, RoundedCornerShape(Radius.r3))
                    .padding(Space.s4)
                    .semantics { liveRegion = LiveRegionMode.Assertive },
                verticalArrangement = Arrangement.spacedBy(Space.s1),
            ) {
                Txt("有词选错了", weight = SemiBold, color = c.danger)
                Txt("回到上一步重新核对备份。", size = FontSize.t2)
            }
        }
    }
}

/** Rust 的 quiz 每题给 4 个选项，原型画的是 3 个，这里排成两行两列。 */
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

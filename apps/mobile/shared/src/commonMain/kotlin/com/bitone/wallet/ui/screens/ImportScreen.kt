package com.bitone.wallet.ui.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.bitone.wallet.flow.ChecksumState
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.ui.components.Card
import com.bitone.wallet.ui.components.Chip
import com.bitone.wallet.ui.components.Medium
import com.bitone.wallet.ui.components.Page
import com.bitone.wallet.ui.components.PrimaryButton
import com.bitone.wallet.ui.components.Segmented
import com.bitone.wallet.ui.components.TitleBar
import com.bitone.wallet.ui.components.Txt
import com.bitone.wallet.ui.components.WordKeyboard
import com.bitone.wallet.ui.components.tap
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Space

/** A1b 导入助记词。输入全部经过内置键盘。 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ImportScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val current = flow.importedWords.size
    val rowHeight = if (flow.wordCount == 12) 36.dp else 32.dp
    val requester = remember { BringIntoViewRequester() }
    LaunchedEffect(current) { requester.bringIntoView() }
    Page(
        header = { TitleBar("导入助记词", onBack = { flow.back() }) { Chip("内置键盘", c.accentSoft, c.accentText) } },
        contentGap = Space.s4,
        bottom = { ImportPanel(flow) },
    ) {
        Segmented(WordCountOptions, flow.wordCount, flow::setImportWordCount)
        Card {
            Column(Modifier.padding(Space.s3)) {
                ThreeColumns(List(flow.wordCount) { it }) { index, _, modifier ->
                    val active = index == current
                    val invalid = active && flow.bufferInvalid
                    WordCell(
                        number = index + 1,
                        word =
                            when {
                                index < current -> flow.importedWords[index]
                                active -> flow.importBuffer
                                else -> ""
                            },
                        height = rowHeight,
                        modifier = if (active) modifier.bringIntoViewRequester(requester) else modifier,
                        highlight =
                            when {
                                invalid -> c.dangerSoft
                                active -> c.accentSoft
                                else -> Color.Transparent
                            },
                        wordColor = if (invalid) c.danger else c.text,
                    )
                }
            }
        }
        PassphraseSwitch(flow.passphraseEnabled, flow::togglePassphrase)
    }
}

@Composable
private fun ImportPanel(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val (status, color) =
        when {
            flow.importComplete && flow.checksum == ChecksumState.Valid -> "校验和正确" to c.ok
            flow.importComplete && flow.checksum == ChecksumState.Invalid -> "校验和错误，请检查助记词" to c.danger
            flow.importComplete -> "校验中" to c.text3
            flow.bufferInvalid -> "不在词表中" to c.danger
            else -> "第 ${flow.importedWords.size + 1} 个词" to c.text3
        }
    Column(Modifier.fillMaxWidth().padding(bottom = Space.s3), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.s2),
        ) {
            Txt(status, Modifier.weight(1f), size = FontSize.t2, color = color)
            flow.suggestions.forEach { word ->
                Box(
                    Modifier.tap { flow.importPick(word) }
                        .height(36.dp)
                        .background(c.accentSoft, RoundedCornerShape(Radius.rf))
                        .padding(horizontal = Space.s3),
                    contentAlignment = Alignment.Center,
                ) {
                    Txt(word, size = FontSize.t2, weight = Medium, color = c.accentText)
                }
            }
        }
        WordKeyboard(flow::importLetter, flow::importDelete)
        Box(Modifier.padding(top = Space.s1)) {
            PrimaryButton(
                "继续",
                enabled = flow.checksum == ChecksumState.Valid && !flow.busy,
                onClick = flow::continueAfterImport,
            )
        }
    }
}

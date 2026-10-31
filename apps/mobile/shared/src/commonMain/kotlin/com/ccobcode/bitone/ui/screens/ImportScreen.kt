package com.ccobcode.bitone.ui.screens

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
import com.ccobcode.bitone.flow.ChecksumState
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.ui.components.Card
import com.ccobcode.bitone.ui.components.Chip
import com.ccobcode.bitone.ui.components.Medium
import com.ccobcode.bitone.ui.components.Page
import com.ccobcode.bitone.ui.components.PrimaryButton
import com.ccobcode.bitone.ui.components.Segmented
import com.ccobcode.bitone.ui.components.TitleBar
import com.ccobcode.bitone.ui.components.Txt
import com.ccobcode.bitone.ui.components.WordKeyboard
import com.ccobcode.bitone.ui.components.tap
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.FontSize
import com.ccobcode.bitone.ui.theme.Radius
import com.ccobcode.bitone.ui.theme.Space

/** 词格可以点回去改,高度按触控区算. */
private val CellHeight = 44.dp

/** A1b 导入助记词.输入全部经过内置键盘. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ImportScreen(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val current = flow.setup.importCurrent
    val requester = remember { BringIntoViewRequester() }
    LaunchedEffect(current) { requester.bringIntoView() }
    Page(
        header = { TitleBar("导入助记词", onBack = { flow.back() }) { Chip("内置键盘", c.accentSoft, c.accentText) } },
        contentGap = Space.s4,
        bottom = { ImportPanel(flow) },
    ) {
        Segmented(WordCountOptions, flow.setup.wordCount, flow.setup::setImportWordCount)
        Card {
            Column(Modifier.padding(Space.s3)) {
                ThreeColumns(List(flow.setup.wordCount) { it }) { index, _, modifier ->
                    val active = index == current
                    val invalid = active && flow.setup.bufferInvalid
                    val filled = index < flow.setup.importedWords.size
                    WordCell(
                        number = index + 1,
                        word =
                            when {
                                active -> flow.setup.importBuffer
                                filled -> flow.setup.importedWords[index]
                                else -> ""
                            },
                        height = CellHeight,
                        modifier = if (active) modifier.bringIntoViewRequester(requester) else modifier,
                        highlight =
                            when {
                                invalid -> c.dangerSoft
                                active -> c.accentSoft
                                else -> Color.Transparent
                            },
                        wordColor = if (invalid) c.danger else c.text,
                        onClick = if (filled) ({ flow.setup.importEdit(index) }) else null,
                    )
                }
            }
        }
        PassphraseSwitch(flow.setup.passphraseEnabled, flow.setup::togglePassphrase)
    }
}

@Composable
private fun ImportPanel(flow: WalletFlow) {
    val c = BitOneTheme.colors
    val (status, color) =
        when {
            flow.setup.bufferInvalid -> "不在词表中" to c.danger
            flow.setup.importEditing != null -> "第 ${flow.setup.importCurrent + 1} 个词" to c.text3
            flow.setup.importComplete && flow.setup.checksum == ChecksumState.Valid -> "校验和正确" to c.ok
            flow.setup.importComplete && flow.setup.checksum == ChecksumState.Invalid -> "校验和错误,点词可以修改" to c.danger
            flow.setup.importComplete -> "校验中" to c.text3
            else -> "第 ${flow.setup.importCurrent + 1} 个词" to c.text3
        }
    Column(Modifier.fillMaxWidth().padding(bottom = Space.s3), verticalArrangement = Arrangement.spacedBy(Space.s2)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(Space.s2),
        ) {
            Txt(status, Modifier.weight(1f), size = FontSize.t2, color = color)
            flow.setup.suggestions.forEach { word ->
                Box(
                    Modifier.tap { flow.setup.importPick(word) }
                        .height(36.dp)
                        .background(c.accentSoft, RoundedCornerShape(Radius.rf))
                        .padding(horizontal = Space.s3),
                    contentAlignment = Alignment.Center,
                ) {
                    Txt(word, size = FontSize.t2, weight = Medium, color = c.accentText)
                }
            }
        }
        WordKeyboard(flow.setup::importLetter, flow.setup::importDelete)
        Box(Modifier.padding(top = Space.s1)) {
            PrimaryButton(
                "继续",
                enabled = flow.setup.checksum == ChecksumState.Valid && !flow.busy,
                onClick = flow.setup::continueAfterImport,
            )
        }
    }
}

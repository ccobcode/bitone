package com.ccobcode.bitone

import androidx.compose.ui.geometry.Size
import androidx.compose.ui.test.ComposeUiTest
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runSkikoComposeUiTest
import com.ccobcode.bitone.flow.Screen
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle

private const val PIN = "123456"

/** 360x800 的手机去掉状态栏和导航栏后的可用区域. */
private val Phone = Size(360f, 740f)

private const val LABEL = "BIP39 密码"
private const val ASK = "输入 BIP39 密码"
private const val AGAIN = "再输一次"
private const val PIN_PAGE = "输入 PIN"

/** BIP39 密码只在建钱包,导入和旧钱包迁移时输,A7 只有 PIN.查看助记词页在词下面显示保存的密码. */
@OptIn(ExperimentalTestApi::class, ExperimentalCoroutinesApi::class)
class PassphraseScreenTest {
    private val flowScope = TestScope()

    private fun ComposeUiTest.settle() {
        flowScope.advanceUntilIdle()
        waitForIdle()
    }

    private fun ComposeUiTest.tap(label: String) {
        onNodeWithText(label).performClick()
        settle()
    }

    private fun ComposeUiTest.absent(vararg texts: String) = texts.forEach { text ->
        assertEquals(0, onAllNodesWithText(text).fetchSemanticsNodes().size, text)
    }

    private fun viewingWords(passphrase: String, checks: ComposeUiTest.() -> Unit) =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN, passphrase)
            rig.flow.openSettings()
            rig.flow.settings.startViewWords()
            flowScope.advanceUntilIdle()
            flowScope.submit(rig.flow, PIN)
            assertEquals(Screen.ViewWords, rig.flow.screen)
            setContent { App(rig.flow, FakeBrightness()) }
            checks()
        }

    @Test
    fun theViewWordsPageShowsTheStoredPassphraseBelowTheWords() =
        viewingWords(PASSPHRASE) {
            val word = onNodeWithText(Words12.last()).assertIsDisplayed().fetchSemanticsNode()
            val label = onNodeWithText(LABEL).assertIsDisplayed().fetchSemanticsNode()
            onNodeWithText(PASSPHRASE).assertIsDisplayed()
            assertTrue(label.positionInRoot.y > word.positionInRoot.y)
        }

    @Test
    fun aWalletWithoutAPassphraseShowsOnlyTheWords() =
        viewingWords("") {
            onNodeWithText(Words12.last()).assertIsDisplayed()
            absent(LABEL)
        }

    @Test
    fun theUnlockPageAsksOnlyForThePin() =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN, PASSPHRASE)
            rig.flow.export.open()
            flowScope.advanceUntilIdle()
            setContent { App(rig.flow, FakeBrightness()) }
            onNodeWithText(PIN_PAGE).assertIsDisplayed()
            absent(LABEL, ASK)
        }

    @Test
    fun anOutdatedWalletEntersThePassphraseTwiceBeforeThePin() =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN) { it.hasOutdatedPassphraseWallet(entropyOnlyVault(PIN)) }
            rig.flow.export.open()
            flowScope.advanceUntilIdle()
            setContent { App(rig.flow, FakeBrightness()) }
            onNodeWithText(ASK).assertIsDisplayed()
            absent(PIN_PAGE)

            listOf("a", "b", "完成").forEach { tap(it) }
            onNodeWithText(AGAIN).assertIsDisplayed()
            listOf("a", "b", "完成").forEach { tap(it) }
            onNodeWithText(PIN_PAGE).assertIsDisplayed()

            PIN.forEach { tap(it.toString()) }
            tap("解锁")
            assertEquals(Screen.Export, rig.flow.screen)
            assertContentEquals(boundVault(PIN, "ab"), rig.store.vault)
        }

    @Test
    fun aMismatchOnTheSetupPageSaysSo() =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN)
            rig.flow.setup.startImport()
            flowScope.advanceUntilIdle()
            rig.flow.setup.confirmOverwrite()
            Words12.forEach {
                rig.flow.setup.importPick(it)
                flowScope.advanceUntilIdle()
            }
            rig.flow.setup.togglePassphrase()
            rig.flow.setup.continueAfterImport()
            assertEquals(Screen.Passphrase, rig.flow.screen)
            setContent { App(rig.flow, FakeBrightness()) }
            onNodeWithText(ASK).assertIsDisplayed()

            listOf("a", "b", "完成").forEach { tap(it) }
            listOf("a", "完成").forEach { tap(it) }
            onNodeWithText("两次不一致,请重新输入").assertIsDisplayed()
            onNodeWithText(ASK).assertIsDisplayed()
        }
}

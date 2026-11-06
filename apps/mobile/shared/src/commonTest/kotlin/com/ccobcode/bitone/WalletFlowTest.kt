package com.ccobcode.bitone

import androidx.compose.runtime.snapshots.Snapshot
import com.ccobcode.bitone.flow.CheckState
import com.ccobcode.bitone.flow.ChecksumState
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.VerifyState
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.changedLockDelayMs
import com.ccobcode.bitone.flow.evaluate
import com.ccobcode.bitone.flow.pinDelayMs
import com.ccobcode.bitone.platform.DefiniteResult
import com.ccobcode.bitone.platform.HardwareLevel
import com.ccobcode.bitone.platform.PinRecord
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.platform.firstBound
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Qr
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

private const val PIN = "123456"
private const val BAD_PIN = "000000"

@OptIn(ExperimentalCoroutinesApi::class)
class WalletFlowTest {

    private fun TestScope.rig(vararg setup: (Rig) -> Unit) = newRig().also { rig -> setup.forEach { it(rig) } }

    private fun TestScope.boot(rig: Rig): WalletFlow {
        rig.flow.onResume()
        advanceUntilIdle()
        return rig.flow
    }

    private fun WalletFlow.typeSetPin(digits: String) = digits.forEach(pin::digit)

    private fun WalletFlow.typePinTwice() {
        typeSetPin(PIN)
        typeSetPin(PIN)
    }

    private fun TestScope.finishWithPin(flow: WalletFlow) {
        flow.typePinTwice()
        flow.setup.finishSetup()
        advanceUntilIdle()
    }

    private fun Rig.hasWallet(vault: ByteArray) {
        store.meta = WalletMeta(false, false, MAIN_FINGERPRINT)
        store.vault = vault
    }

    private fun TestScope.pickWords(flow: WalletFlow, words: List<String> = Words12) = words.forEach {
        flow.setup.importPick(it)
        advanceUntilIdle()
    }

    private fun TestScope.answerAll(flow: WalletFlow, correct: Boolean) {
        flow.setup.quiz.forEachIndexed { i, item ->
            flow.setup.pick(item.index, if (correct || i > 0) item.options[0] else item.options[1])
            advanceUntilIdle()
        }
    }

    private fun TestScope.toVerify(flow: WalletFlow) {
        flow.setup.startCreate()
        advanceUntilIdle()
        assertEquals(Screen.Backup, flow.screen)
        flow.setup.startVerify()
        advanceUntilIdle()
        assertEquals(Screen.Verify, flow.screen)
    }

    private fun TestScope.passVerify(flow: WalletFlow) {
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
    }

    private fun TestScope.createWallet(flow: WalletFlow) {
        passVerify(flow)
        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
    }

    private fun TestScope.ready(vararg setup: (Rig) -> Unit): Rig {
        val rig = rig(*setup)
        createWallet(boot(rig))
        return rig
    }

    private fun TestScope.openUnlock(flow: WalletFlow) {
        flow.export.open()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
    }

    /** 等够延时再错一次. */
    private fun TestScope.failOnce(rig: Rig) {
        rig.clock.elapsed += pinDelayMs(rig.store.pin.failures)
        rig.flow.unlock.refreshStatus()
        submit(rig.flow, BAD_PIN)
    }

    // 启动和自检

    @Test
    fun firstRunGoesToWelcome() = runTest {
        val rig = rig()
        assertEquals(Screen.Booting, rig.flow.screen)
        assertEquals(Screen.Welcome, boot(rig).screen)
        assertEquals("全部通过", rig.flow.report?.summary)
    }

    @Test
    fun failedSelfCheckBlocksCreationAndRecovers() = runTest {
        val rig = rig({ it.check.facts = facts(wifiOn = true) })
        val flow = boot(rig)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.report!!.blocked)

        passSelfCheck(rig)
        assertEquals(Screen.Welcome, flow.screen)
    }

    @Test
    fun selfCheckRunsAgainBeforeCreating() = runTest {
        val rig = rig()
        val flow = boot(rig)
        rig.check.facts = facts(adbEnabled = true)
        flow.setup.startCreate()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertNull(rig.core.lastDraft)
    }

    @Test
    fun teeIsOnlyAWarning() = runTest {
        val rig = rig({ it.check.facts = facts(keystore = HardwareLevel.Tee, overlayApps = 2) })
        val flow = boot(rig)
        assertEquals(Screen.Welcome, flow.screen)
        assertEquals("2 项警告", flow.report?.summary)
    }

    @Test
    fun blockingFacts() = runTest {
        listOf(
                facts(keystore = HardwareLevel.None),
                facts(screenLockSet = false),
                facts(rooted = true),
                facts(bootloaderLocked = false),
                facts(thirdPartyAccessibility = 1),
            )
            .forEach { deviceFacts ->
                val rig = rig({ it.check.facts = deviceFacts })
                assertEquals(Screen.SelfCheckFailed, boot(rig).screen)
            }
    }

    @Test
    fun existingWalletGoesHome() = runTest {
        val rig =
            rig({
                it.hasWallet(ByteArray(8))
            })
        assertEquals(Screen.Home, boot(rig).screen)
    }

    @Test
    fun lostHardwareKeyNeedsRecoveryAndKeepsData() = runTest {
        val rig =
            rig({
                it.hasWallet(ByteArray(8))
                it.key.lost = true
            })
        assertEquals(Screen.NeedsRecovery, boot(rig).screen)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun recoveryImportReplacesLostWallet() = runTest {
        val rig =
            rig({
                it.hasWallet("old".encodeToByteArray())
                it.store.pin = PinRecord(3, 1, 0)
                it.key.lost = true
            })
        val flow = boot(rig)
        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.ImportOverwrite, flow.screen)

        // 返回不动原来的密文.
        flow.back()
        assertEquals(Screen.NeedsRecovery, flow.screen)
        assertContentEquals("old".encodeToByteArray(), rig.store.vault)

        flow.setup.startImport()
        advanceUntilIdle()
        flow.setup.confirmOverwrite()
        assertEquals(Screen.ImportWords, flow.screen)
        pickWords(flow)
        flow.setup.continueAfterImport()
        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(1, rig.key.deleted)
    }

    @Test
    fun recoveryImportContinuesAfterAnAcknowledgement() = runTest {
        val rig =
            rig({
                it.hasWallet("old".encodeToByteArray())
                it.key.lost = true
            })
        val flow = boot(rig)
        rig.check.facts = facts(wifiOn = true)
        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.ImportOverwrite, flow.screen)
        flow.setup.confirmOverwrite()
        pickWords(flow)
        flow.setup.continueAfterImport()
        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
        assertContentEquals(boundVault(PIN), rig.store.vault)
    }

    @Test
    fun aPhoneWithoutAWalletCanStillImportThroughAFailedCheck() = runTest {
        val rig = rig({ it.check.facts = facts(bootloaderLocked = false) })
        val flow = boot(rig)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.Welcome, flow.screen)

        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.ImportWords, flow.screen)
        pickWords(flow)
        flow.setup.continueAfterImport()
        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
    }

    @Test
    fun anAcknowledgementNeverGeneratesANewWallet() = runTest {
        val rig = rig({ it.check.facts = facts(wifiOn = true) })
        val flow = boot(rig)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        flow.setup.startCreate()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertNull(rig.core.lastDraft)
    }

    /** 自检一直不过时不能只剩"重新检测". */
    @Test
    fun generatingCanBeAbandonedWhenTheCheckKeepsFailing() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        val draft = rig.core.lastDraft
        rig.check.facts = facts(wifiOn = true)
        flow.setup.backToBackup()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertFalse(flow.canAcknowledge)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)

        flow.setup.abandonCreate()
        assertEquals(listOf(Screen.Welcome), flow.stack)
        assertTrue(draft!!.closed)
    }

    // 忘记 PIN

    private fun TestScope.finishRecovery(flow: WalletFlow) {
        flow.setup.confirmOverwrite()
        pickWords(flow)
        flow.setup.continueAfterImport()
        finishWithPin(flow)
    }

    @Test
    fun aForgottenPinRecoversWithoutThePinEvenDuringADelay() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(6) { failOnce(rig) }
        assertTrue(flow.unlock.waitMs > 0)
        val opens = rig.core.opens

        flow.forgotPin()
        advanceUntilIdle()
        assertEquals(listOf(Screen.Home, Screen.ImportOverwrite), flow.stack)

        // 返回不动原来的钱包.
        val vault = rig.store.vault
        flow.back()
        assertEquals(Screen.Home, flow.screen)
        assertContentEquals(vault, rig.store.vault)

        openUnlockAfterDelay(rig)
        flow.forgotPin()
        advanceUntilIdle()
        finishRecovery(flow)
        assertEquals(Screen.Home, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(0, flow.unlock.failures)
        assertEquals(opens + 1, rig.core.opens)
    }

    private fun TestScope.openUnlockAfterDelay(rig: Rig) {
        rig.clock.elapsed += pinDelayMs(rig.store.pin.failures)
        openUnlock(rig.flow)
    }

    @Test
    fun aForgottenPinDropsTheOperationInProgress() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.settings.startChangePin()
        advanceUntilIdle()
        flow.typeSetPin("246813")
        flow.typeSetPin("246813")
        flow.settings.continueChangePin()
        assertEquals(Screen.Unlock, flow.screen)

        flow.forgotPin()
        advanceUntilIdle()
        assertFalse(flow.settings.changingPin)
        finishRecovery(flow)
        assertEquals(Screen.Home, flow.screen)
        assertContentEquals(boundVault(PIN), rig.store.vault)
    }

    @Test
    fun aForgottenPinStillShowsTheSelfCheckWarning() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.check.facts = facts(wifiOn = true)
        flow.forgotPin()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.ImportOverwrite, flow.screen)
    }

    // 没有死路

    @Test
    fun aDamagedVaultGoesToRecovery() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.core.vaultDamaged = true
        submit(flow, PIN)
        assertEquals(Screen.NeedsRecovery, flow.screen)
        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.ImportOverwrite, flow.screen)
        assertNotNull(rig.store.vault)
    }

    /** 内容损坏和 PIN 错误分不开,一直报错的 Keystore 也重试不好,都要能走到导入. */
    @Test
    fun retriesThatCanNeverSucceedStillReachImport() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.bindFails = true
        repeat(3) { submit(flow, PIN) }
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(flow.unlock.error)

        flow.forgotPin()
        advanceUntilIdle()
        assertEquals(Screen.ImportOverwrite, flow.screen)
        rig.key.bindFails = false
        finishRecovery(flow)
        assertEquals(Screen.Home, flow.screen)
    }

    @Test
    fun aFreshInstallImportsWithoutTheOverwriteWarning() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.ImportWords, flow.screen)
    }

    @Test
    fun everyBip39LengthCanBeImported() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        flow.setup.setImportWordCount(18)
        pickWords(flow, List(18) { "word$it" })
        assertEquals(ChecksumState.Valid, flow.setup.checksum)
        flow.setup.continueAfterImport()
        assertEquals(Screen.SetPin, flow.screen)
    }

    // 验证

    @Test
    fun verifyFailureReturnsToBackupWithSameWords() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        val draft = rig.core.lastDraft
        answerAll(flow, correct = false)
        assertEquals(VerifyState.Failed, flow.setup.verifyState)

        flow.setup.backToBackup()
        advanceUntilIdle()
        assertEquals(Screen.Backup, flow.screen)
        assertEquals(Words12, flow.setup.backupWords)
        assertEquals(draft, rig.core.lastDraft)
        assertFalse(draft!!.closed)
    }

    @Test
    fun selfCheckStopsSealingTheSeed() = runTest {
        val rig = rig()
        val flow = boot(rig)
        passVerify(flow)
        rig.check.facts = facts(wifiOn = true)
        finishWithPin(flow)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertEquals(0, rig.key.binds)
        assertNull(rig.store.vault)
    }

    @Test
    fun selfCheckStopsShowingTheSeedAgain() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        rig.check.facts = facts(wifiOn = true)
        flow.setup.backToBackup()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.setup.backupWords.isEmpty())
    }

    @Test
    fun selfCheckStopsShowingANewSeed() = runTest {
        val rig = rig()
        val flow = boot(rig)
        flow.setup.startCreate()
        advanceUntilIdle()
        val old = rig.core.lastDraft!!
        rig.check.facts = facts(wifiOn = true)
        flow.setup.setBackupWordCount(24)
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertEquals(old, rig.core.lastDraft)
    }

    @Test
    fun failedVerifyCannotBeCorrectedInPlace() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        val first = flow.setup.quiz.first()
        answerAll(flow, correct = false)
        flow.setup.pick(first.index, first.options[0])
        advanceUntilIdle()
        assertEquals(VerifyState.Failed, flow.setup.verifyState)
        flow.setup.continueAfterVerify()
        assertEquals(Screen.Verify, flow.screen)
    }

    @Test
    fun cannotSkipVerification() = runTest {
        val rig = rig()
        val flow = boot(rig)
        flow.setup.startCreate()
        advanceUntilIdle()

        // 没答题就继续.
        flow.setup.startVerify()
        advanceUntilIdle()
        flow.setup.continueAfterVerify()
        assertEquals(Screen.Verify, flow.screen)

        // 绕过页面直接调用后面的步骤.
        flow.typePinTwice()
        assertTrue(flow.pin.done)
        flow.setup.finishSetup()
        advanceUntilIdle()
        flow.openHome()
        flow.openSettings()

        assertEquals(Screen.Verify, flow.screen)
        assertNull(rig.store.vault)
        assertNull(rig.store.meta)
    }

    @Test
    fun wrongAnswersCannotReachSetPin() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        answerAll(flow, correct = false)
        flow.setup.continueAfterVerify()
        finishWithPin(flow)
        assertNull(rig.store.vault)
    }

    @Test
    fun changingWordCountDiscardsVerification() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.back()
        advanceUntilIdle()
        assertEquals(Screen.Backup, flow.screen)
        val old = rig.core.lastDraft!!
        flow.setup.setBackupWordCount(24)
        advanceUntilIdle()
        assertTrue(old.closed)
        assertEquals(24, flow.setup.backupWords.size)
        flow.setup.finishSetup()
        advanceUntilIdle()
        assertNull(rig.store.vault)
    }

    @Test
    fun backupWordsLeaveStateDuringVerify() = runTest {
        val flow = boot(rig())
        toVerify(flow)
        assertTrue(flow.setup.backupWords.isEmpty())
    }

    // 建钱包

    @Test
    fun finishSetupStoresDoubleWrappedVault() = runTest {
        val rig = ready()
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(false, rig.store.meta?.imported)
        assertEquals("73C5DA0A", rig.flow.session.fingerprint)
        assertTrue(rig.core.lastDraft!!.closed)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun mismatchedPinStartsOver() = runTest {
        val flow = boot(rig())
        passVerify(flow)
        flow.typeSetPin(PIN)
        assertTrue(flow.pin.confirming)
        flow.typeSetPin(BAD_PIN)
        assertTrue(flow.pin.mismatch)
        assertFalse(flow.pin.confirming)
        assertEquals("", flow.pin.input)
    }

    @Test
    fun cancelledScreenLockKeepsDraft() = runTest {
        val rig = rig()
        val flow = boot(rig)
        passVerify(flow)
        flow.typePinTwice()
        rig.key.cancelAuth = true
        flow.setup.finishSetup()
        advanceUntilIdle()
        assertEquals(Screen.SetPin, flow.screen)
        assertNotNull(flow.pin.error)
        assertFalse(rig.core.lastDraft!!.closed)
        assertNull(rig.store.vault)
    }

    @Test
    fun importSkipsVerification() = runTest {
        val rig = rig()
        val flow = boot(rig)
        flow.setup.startImport()
        advanceUntilIdle()
        pickWords(flow)
        assertEquals(ChecksumState.Valid, flow.setup.checksum)
        flow.setup.continueAfterImport()
        assertEquals(Screen.SetPin, flow.screen)
        assertTrue(flow.setup.isImportFlow)
        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
        assertEquals(true, rig.store.meta?.imported)
    }

    @Test
    fun badChecksumBlocksImport() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        pickWords(flow, (Words12.dropLast(1) + "badword"))
        assertEquals(ChecksumState.Invalid, flow.setup.checksum)
        flow.setup.continueAfterImport()
        assertEquals(Screen.ImportWords, flow.screen)

        flow.setup.importDelete()
        advanceUntilIdle()
        assertEquals(ChecksumState.Incomplete, flow.setup.checksum)
    }

    @Test
    fun wrongWordCanBeFixedWithoutRetypingTheRest() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        pickWords(flow, (listOf("badword") + Words12.drop(1)))
        assertEquals(ChecksumState.Invalid, flow.setup.checksum)

        flow.setup.importEdit(0)
        assertEquals(0, flow.setup.importCurrent)
        assertEquals("badword", flow.setup.importBuffer)
        assertEquals(ChecksumState.Incomplete, flow.setup.checksum)
        flow.setup.continueAfterImport()
        assertEquals(Screen.ImportWords, flow.screen)

        flow.setup.importPick(Words12.first())
        advanceUntilIdle()
        assertEquals(Words12, flow.setup.importedWords)
        assertEquals(ChecksumState.Valid, flow.setup.checksum)
        assertNull(flow.setup.importEditing)
    }

    @Test
    fun deletingAnEditedWordEmptyCancelsTheEdit() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        pickWords(flow)
        flow.setup.importEdit(11)
        repeat(flow.setup.importBuffer.length) { flow.setup.importDelete() }
        assertEquals("", flow.setup.importBuffer)
        assertEquals(11, flow.setup.importEditing)

        flow.setup.importDelete()
        advanceUntilIdle()
        assertNull(flow.setup.importEditing)
        assertEquals(Words12, flow.setup.importedWords)
        assertEquals(ChecksumState.Valid, flow.setup.checksum)
    }

    @Test
    fun unknownWordIsFlagged() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        "zz".forEach(flow.setup::importLetter)
        assertTrue(flow.setup.bufferInvalid)
        flow.setup.importPick("notaword")
        assertTrue(flow.setup.importedWords.isEmpty())
    }

    @Test
    fun wordCountSwitchKeepsEnteredWords() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        pickWords(flow)
        flow.setup.setImportWordCount(24)
        advanceUntilIdle()
        assertEquals(12, flow.setup.importedWords.size)
        assertEquals(ChecksumState.Incomplete, flow.setup.checksum)
    }

    // PIN

    @Test
    fun delaySchedule() {
        assertEquals(listOf(0L, 0L, 0L, 0L, 0L), (0..4).map(::pinDelayMs))
        assertEquals(30_000L, pinDelayMs(5))
        assertEquals(300_000L, pinDelayMs(9))
        assertTrue((5..8).all { pinDelayMs(it) < pinDelayMs(it + 1) })
    }

    @Test
    fun wrongPinIsCountedAndDelayed() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(4) { submit(flow, BAD_PIN) }
        assertEquals(4, flow.unlock.failures)
        assertEquals(0, flow.unlock.waitMs)
        assertEquals(Screen.Unlock, flow.screen)

        submit(flow, BAD_PIN)
        assertEquals(5, flow.unlock.failures)
        assertEquals(30_000, flow.unlock.waitMs)
    }

    @Test
    fun waitingBlocksInputAndDoesNotCount() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(5) { submit(flow, BAD_PIN) }
        val opens = rig.core.opens
        submit(flow, BAD_PIN)
        assertEquals("", flow.unlock.pin)
        assertEquals(5, rig.store.pin.failures)
        assertEquals(opens, rig.core.opens)

        rig.clock.elapsed += 29_000
        flow.unlock.refreshStatus()
        assertEquals(1_000, flow.unlock.waitMs)
        rig.clock.elapsed += 1_000
        flow.unlock.refreshStatus()
        assertEquals(0, flow.unlock.waitMs)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
    }

    @Test
    fun rebootRestartsDelay() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(5) { submit(flow, BAD_PIN) }
        rig.clock.elapsed += 25_000
        flow.unlock.refreshStatus()
        assertEquals(5_000, flow.unlock.waitMs)

        // 重启后单调时钟归零,即使读数更大也按新开机算.
        rig.clock.boot = 2
        rig.clock.elapsed = 500_000
        flow.unlock.refreshStatus()
        assertEquals(30_000, flow.unlock.waitMs)
    }

    @Test
    fun counterIsWrittenBeforeKeyDerivation() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.core.killDuringOpen = true
        PIN.forEach(flow.unlock::digit)
        flow.unlock.submit()
        runCatching { advanceUntilIdle() }
        assertEquals(1, rig.store.pin.failures)
    }

    @Test
    fun correctPinClearsCounter() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(3) { submit(flow, BAD_PIN) }
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(0, flow.unlock.failures)
    }

    @Test
    fun wrongPinsOnlyEverDelay() = runTest {
        val rig = ready()
        val flow = rig.flow
        val deleted = rig.key.deleted
        openUnlock(flow)
        repeat(30) { failOnce(rig) }
        assertEquals(30, rig.store.pin.failures)
        assertEquals(300_000L, flow.unlock.waitMs)
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(deleted, rig.key.deleted)

        // 第 31 次输对照样能解开.
        rig.clock.elapsed += 300_000
        flow.unlock.refreshStatus()
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals(0, rig.store.pin.failures)
    }

    @Test
    fun aHighCounterAtLaunchKeepsTheWallet() = runTest {
        val rig =
            rig({
                it.hasWallet(boundVault(PIN))
                it.store.pin = PinRecord(99, 1, 0)
            })
        assertEquals(Screen.Home, boot(rig).screen)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun theCounterIsWrittenBeforeDecrypting() = runTest {
        val rig = ready()
        openUnlock(rig.flow)
        rig.core.onOpen = { assertEquals(1, rig.store.pin.failures) }
        submit(rig.flow, PIN)
        assertEquals(Screen.Export, rig.flow.screen)
    }

    /** 硬件结果按次认证,所以每猜一次 PIN 都要过一次系统屏幕锁. */
    @Test
    fun everyAttemptConfirmsTheDeviceLock() = runTest {
        val rig = ready()
        val flow = rig.flow
        val before = rig.key.binds
        openUnlock(flow)
        submit(flow, BAD_PIN)
        assertEquals(before + 1, rig.key.binds)
        submit(flow, BAD_PIN)
        assertEquals(before + 2, rig.key.binds)

        // 不过手机锁就不算一次尝试.
        rig.key.cancelAuth = true
        submit(flow, BAD_PIN)
        assertEquals(2, rig.store.pin.failures)
        assertNotNull(flow.unlock.error)
    }

    @Test
    fun legacyVaultMigratesOnTheRightPin() = runTest {
        val rig = ready()
        rig.store.vault = legacyVault(PIN)
        val flow = rig.flow
        openUnlock(flow)
        submit(flow, BAD_PIN)
        assertContentEquals(legacyVault(PIN), rig.store.vault)
        assertEquals(0, rig.key.legacyDropped)
        submit(flow, PIN)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(1, rig.key.legacyDropped)
    }

    /** 迁移没走完不影响这次使用,旧密文和旧密钥都在,下次解锁再换. */
    @Test
    fun failedMigrationKeepsTheLegacyVaultAndOpens() = runTest {
        listOf<(Rig) -> Unit>({ it.key.cancelFor = PIN }, { it.key.unavailable = true }).forEach { fail ->
            val rig = ready()
            rig.store.vault = legacyVault(PIN)
            val flow = rig.flow
            openUnlock(flow)
            fail(rig)
            submit(flow, PIN)
            assertEquals(Screen.Export, flow.screen)
            assertContentEquals(legacyVault(PIN), rig.store.vault)
            assertEquals(0, rig.key.legacyDropped)
            assertEquals(0, rig.store.pin.failures)

            rig.key.cancelFor = null
            rig.key.unavailable = false
            flow.export.finish()
            openUnlock(flow)
            submit(flow, PIN)
            assertContentEquals(boundVault(PIN), rig.store.vault)
            assertEquals(1, rig.key.legacyDropped)
        }
    }

    @Test
    fun legacyRetryConfirmsTheDeviceLockFirst() = runTest {
        val rig = ready()
        rig.store.vault = legacyVault(PIN)
        val flow = rig.flow
        openUnlock(flow)
        submit(flow, BAD_PIN)
        assertEquals(0, rig.key.confirmations)
        submit(flow, BAD_PIN)
        assertEquals(1, rig.key.confirmations)

        rig.key.cancelConfirm = true
        submit(flow, BAD_PIN)
        assertEquals(2, rig.store.pin.failures)
        assertNotNull(flow.unlock.error)
    }

    @Test
    fun changedLockDelaySchedule() {
        assertEquals(0L, changedLockDelayMs(5))
        assertEquals(listOf(60_000L, 300_000L, 900_000L), (6..8).map(::changedLockDelayMs))
        assertEquals(3_600_000L, changedLockDelayMs(9))
    }

    @Test
    fun changedFingerprintUsesTheLongerDelays() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.changed = true
        repeat(13) {
            rig.clock.elapsed += changedLockDelayMs(rig.store.pin.failures)
            flow.unlock.refreshStatus()
            submit(flow, BAD_PIN)
        }
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(13, rig.store.pin.failures)
        assertEquals(3_600_000, flow.unlock.waitMs)

        // 输对 PIN 后恢复.
        rig.clock.elapsed += 3_600_000
        flow.unlock.refreshStatus()
        submit(flow, PIN)
        assertEquals(false, rig.key.changed)
        assertEquals(0, rig.store.pin.failures)
    }

    @Test
    fun hardwareFailureIsNotAPinFailure() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.lost = true
        submit(flow, BAD_PIN)
        assertEquals(Screen.NeedsRecovery, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertNotNull(rig.store.vault)
    }

    /** Keystore 这次出错不是钱包丢了,不能把人推去导入. */
    @Test
    fun aKeystoreErrorStaysOnTheUnlockPageForARetry() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.bindFails = true
        submit(flow, PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(flow.unlock.error)
        assertEquals(0, rig.store.pin.failures)

        rig.key.bindFails = false
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
    }

    @Test
    fun cancelledScreenLockIsNotAPinFailure() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.cancelAuth = true
        submit(flow, BAD_PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertNotNull(flow.unlock.error)
    }

    @Test
    fun inputIsClearedAfterSubmitAndInBackground() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        PIN.forEach(flow.unlock::digit)
        flow.onBackground()
        assertEquals("", flow.unlock.pin)
    }

    // 导出

    @Test
    fun exportClosesWalletAndPassesFlags() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertTrue(rig.core.lastWallet!!.closed)
        assertEquals(false, rig.core.exportedImported)
        assertEquals(3, flow.player.total)
    }

    @Test
    fun importedFlagSurvivesInStore() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(imported = true, passphrase = false, fingerprint = "73C5DA0A")
                it.store.vault = boundVault(PIN)
            })
        val flow = boot(rig)
        openUnlock(flow)
        submit(flow, PIN)
        assertEquals(true, rig.core.exportedImported)
    }

    @Test
    fun framesCanBePausedAndChangeSpeed() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        submit(flow, PIN)
        val stream = rig.core.lastStream!!
        val player = launch { flow.player.play() }
        advanceTimeBy(600)

        flow.player.togglePaused()
        Snapshot.sendApplyNotifications()
        advanceTimeBy(100)
        val paused = stream.sent
        advanceTimeBy(5_000)
        assertEquals(paused, stream.sent)

        flow.player.togglePaused()
        Snapshot.sendApplyNotifications()
        repeat(2) { flow.player.nextSpeed() }
        assertEquals(Qr.intervals.last(), Qr.intervals[flow.player.speed])
        runCurrent()
        val resumed = stream.sent
        advanceTimeBy(1_000)
        assertTrue(stream.sent - resumed in 3..5, "最快的速度一秒发了 ${stream.sent - resumed} 帧")

        flow.export.finish()
        advanceTimeBy(1_000)
        player.join()
    }

    @Test
    fun framesLoopAtTheDefaultSpeedAndCloseOnFinish() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        submit(flow, PIN)
        val stream = rig.core.lastStream!!
        val player = launch { flow.player.play() }
        advanceTimeBy(1_000)
        assertTrue(stream.sent in 2..3, "一秒发了 ${stream.sent} 帧")
        assertEquals((stream.sent - 1) % 3 + 1, flow.player.index)
        assertFalse(stream.closed)

        flow.export.finish()
        assertEquals(Screen.Home, flow.screen)
        advanceTimeBy(1_000)
        player.join()
        assertTrue(stream.closed)
    }

    @Test
    fun selfCheckRunsAgainBeforeUnlock() = runTest {
        val rig = ready()
        val flow = rig.flow
        rig.check.facts = facts(wifiOn = true)
        flow.export.open()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)

        passSelfCheck(rig)
        assertEquals(Screen.Home, flow.screen)
    }

    // BIP39 密码

    private fun TestScope.toPassphrase(flow: WalletFlow) {
        flow.setup.startCreate()
        advanceUntilIdle()
        flow.setup.togglePassphrase()
        flow.setup.startVerify()
        advanceUntilIdle()
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
        assertEquals(Screen.Passphrase, flow.screen)
    }

    @Test
    fun thePassphraseIsEnteredTwiceAndSealedWithTheWords() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toPassphrase(flow)
        typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone)
        assertTrue(flow.passphrase.confirming)
        assertEquals("", flow.passphrase.input)
        assertEquals(Screen.Passphrase, flow.screen)
        typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone)
        assertEquals(Screen.SetPin, flow.screen)
        assertFalse(flow.passphrase.confirming)
        assertEquals(PASSPHRASE, rig.core.lastDraft?.given)

        finishWithPin(flow)
        assertEquals(Screen.Home, flow.screen)
        assertContentEquals(boundVault(PIN, PASSPHRASE), rig.store.vault)
        // 指纹和公钥和没有密码的钱包一样算出来写盘.
        assertEquals(PASSPHRASE_FINGERPRINT, rig.store.meta?.fingerprint)
        assertEquals(true, rig.store.meta?.passphrase)
        assertContentEquals(keysFor(PASSPHRASE_FINGERPRINT), rig.store.publicKeys)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.session.fingerprint)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun aPassphraseMismatchStartsOver() = runTest {
        val flow = boot(rig())
        toPassphrase(flow)
        typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone)
        typePassphrase(flow, "Pass 1?", flow.setup::passphraseDone)
        assertEquals(Screen.Passphrase, flow.screen)
        assertTrue(flow.passphrase.mismatch)
        assertFalse(flow.passphrase.confirming)

        flow.passphrase.type('x')
        assertFalse(flow.passphrase.mismatch)
        flow.passphrase.delete()
        repeat(2) { typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone) }
        assertEquals(Screen.SetPin, flow.screen)
    }

    @Test
    fun anEmptyPassphraseIsRefusedWhileTheSwitchIsOn() = runTest {
        val flow = boot(rig())
        toPassphrase(flow)
        flow.setup.passphraseDone()
        assertFalse(flow.passphrase.confirming)
        typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone)
        flow.setup.passphraseDone()
        assertTrue(flow.passphrase.confirming)
        assertEquals(Screen.Passphrase, flow.screen)
    }

    @Test
    fun turningTheSwitchOffAfterwardsSealsNoPassphrase() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toPassphrase(flow)
        repeat(2) { typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone) }
        assertEquals(Screen.SetPin, flow.screen)
        flow.back()
        flow.back()
        assertEquals(Screen.Verify, flow.screen)
        flow.back()
        advanceUntilIdle()
        assertEquals(Screen.Backup, flow.screen)

        flow.setup.togglePassphrase()
        flow.setup.startVerify()
        advanceUntilIdle()
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
        assertEquals(Screen.SetPin, flow.screen)
        finishWithPin(flow)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(false, rig.store.meta?.passphrase)
        assertEquals(MAIN_FINGERPRINT, rig.store.meta?.fingerprint)
    }

    @Test
    fun backgroundClearsThePassphraseBeingTyped() = runTest {
        val flow = boot(rig())
        toPassphrase(flow)
        typePassphrase(flow, PASSPHRASE, flow.setup::passphraseDone)
        "Pa".forEach(flow.passphrase::type)
        flow.onBackground()
        assertEquals("", flow.passphrase.input)
        assertFalse(flow.passphrase.confirming)
        assertEquals(Screen.Passphrase, flow.screen)
    }

    @Test
    fun aPassphraseWalletUnlocksWithThePinAlone() = runTest {
        val rig = readyRig(PIN, PASSPHRASE)
        val flow = rig.flow
        openUnlock(flow)
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals("current", rig.core.lastOpened)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.session.fingerprint)
        assertContentEquals(boundVault(PIN, PASSPHRASE), rig.store.vault)
        // 密码只在查看助记词时才经过 JVM.
        assertEquals(0, rig.core.passphraseRead)
    }

    // 旧格式的 BIP39 密码钱包

    @Test
    fun anOutdatedPassphraseWalletAsksThePassphraseOnce() = runTest {
        val rig = rig({ it.hasOutdatedPassphraseWallet(entropyOnlyVault(PIN)) })
        val flow = boot(rig)
        openUnlock(flow)
        assertTrue(flow.unlock.asksPassphrase)
        // 两遍一致之前 PIN 交不上去.
        PIN.forEach(flow.unlock::digit)
        assertFalse(flow.unlock.canSubmit)
        typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone)
        assertTrue(flow.unlock.asksPassphrase)
        typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone)
        assertFalse(flow.unlock.asksPassphrase)
        val binds = rig.key.binds
        flow.unlock.submit()
        advanceUntilIdle()
        assertEquals(Screen.Export, flow.screen)
        assertEquals("entropy:$PASSPHRASE", rig.core.lastOpened)
        // 换成当前格式,密码封进去.同一个 PIN 的硬件结果不变,只确认一次屏幕锁.
        assertContentEquals(boundVault(PIN, PASSPHRASE), rig.store.vault)
        assertEquals(binds + 1, rig.key.binds)
        assertEquals(PASSPHRASE_FINGERPRINT, rig.store.meta?.fingerprint)
        assertEquals(true, rig.store.meta?.passphrase)
        assertContentEquals(keysFor(PASSPHRASE_FINGERPRINT), rig.store.publicKeys)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.session.fingerprint)

        flow.export.finish()
        openUnlock(flow)
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals("current", rig.core.lastOpened)
    }

    @Test
    fun anOutdatedWalletWithoutAPassphraseMigratesSilently() = runTest {
        val rig = rig({ it.hasWallet(entropyOnlyVault(PIN)) })
        val flow = boot(rig)
        openUnlock(flow)
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals("entropy:", rig.core.lastOpened)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(MAIN_FINGERPRINT, rig.store.meta?.fingerprint)
    }

    @Test
    fun aLegacyPassphraseWalletMigratesWithThePassphrase() = runTest {
        val rig = rig({ it.hasOutdatedPassphraseWallet(legacyVault(PIN)) })
        val flow = boot(rig)
        openUnlock(flow)
        assertTrue(flow.unlock.asksPassphrase)
        repeat(2) { typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone) }
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals("legacy:$PASSPHRASE", rig.core.lastOpened)
        assertContentEquals(boundVault(PIN, PASSPHRASE), rig.store.vault)
        assertEquals(1, rig.key.legacyDropped)
        assertEquals(PASSPHRASE_FINGERPRINT, rig.store.meta?.fingerprint)
    }

    @Test
    fun aWrongPinKeepsTheConfirmedPassphraseAndTheOldVault() = runTest {
        val rig = rig({ it.hasOutdatedPassphraseWallet(entropyOnlyVault(PIN)) })
        val flow = boot(rig)
        openUnlock(flow)
        repeat(2) { typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone) }
        submit(flow, BAD_PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(1, rig.store.pin.failures)
        assertContentEquals(entropyOnlyVault(PIN), rig.store.vault)
        assertNull(rig.store.meta?.fingerprint)
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertContentEquals(boundVault(PIN, PASSPHRASE), rig.store.vault)
    }

    @Test
    fun backgroundForgetsTheConfirmedPassphrase() = runTest {
        val rig = rig({ it.hasOutdatedPassphraseWallet(entropyOnlyVault(PIN)) })
        val flow = boot(rig)
        openUnlock(flow)
        typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone)
        "Pa".forEach(flow.passphrase::type)
        flow.onBackground()
        assertEquals("", flow.passphrase.input)
        assertFalse(flow.passphrase.confirming)

        repeat(2) { typePassphrase(flow, PASSPHRASE, flow.unlock::passphraseDone) }
        assertFalse(flow.unlock.asksPassphrase)
        flow.onBackground()
        assertEquals(Screen.Unlock, flow.screen)
        assertTrue(flow.unlock.asksPassphrase)
    }

    /** 元数据丢了,密文是旧格式:不知道有没有设置过密码,可以留空. */
    @Test
    fun lostPreferencesWithAnOutdatedVaultMayLeaveThePassphraseBlank() = runTest {
        val rig =
            rig({
                it.store.vault = entropyOnlyVault(PIN)
                it.store.meta = null
            })
        val flow = boot(rig)
        assertEquals(Screen.Home, flow.screen)
        openUnlock(flow)
        assertTrue(flow.unlock.asksPassphrase)
        assertTrue(flow.unlock.passphraseOptional)
        repeat(2) { flow.unlock.passphraseDone() }
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals("entropy:", rig.core.lastOpened)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        // 解开之后写回真正的指纹,导出带上旧路径账户.
        assertEquals(MAIN_FINGERPRINT, rig.store.meta?.fingerprint)
        assertEquals(true, rig.store.meta?.imported)
        assertEquals(false, rig.store.meta?.passphrase)
        assertEquals(true, rig.core.exportedImported)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
    }

    // 设置

    @Test
    fun backClosesOpenLayersBeforeLeavingThePage() = runTest {
        val rig = rig()
        val flow = boot(rig)
        flow.setup.startImport()
        advanceUntilIdle()
        var closed = 0
        val sheet = { closed += 1 }
        val fullscreen = { closed += 10 }
        flow.openLayer(sheet)
        flow.openLayer(fullscreen)

        assertTrue(flow.back())
        assertEquals(10, closed)
        flow.closeLayer(fullscreen)
        assertTrue(flow.back())
        assertEquals(11, closed)
        flow.closeLayer(sheet)
        assertEquals(Screen.ImportWords, flow.screen)

        assertTrue(flow.back())
        assertEquals(Screen.Welcome, flow.screen)
    }

    @Test
    fun noticeClearsItself() = runTest {
        val rig = ready()
        rig.flow.showNotice("已保存")
        assertEquals("已保存", rig.flow.notice)
        advanceTimeBy(Motion.toast + 1L)
        assertNull(rig.flow.notice)
    }

    @Test
    fun backStack() = runTest {
        val rig = ready()
        val flow = rig.flow
        assertFalse(flow.back())
        flow.openSettings()
        assertTrue(flow.back())
        assertEquals(Screen.Home, flow.screen)
        openUnlock(flow)
        assertTrue(flow.back())
        assertEquals(Screen.Home, flow.screen)
        assertNotEquals(0, rig.check.runs)
    }
}

/** 冷钱包可能放很多年,这些情况都不能把持有人挡在资金外面. */
@OptIn(ExperimentalCoroutinesApi::class)
class LockoutTest {
    private fun TestScope.boot(setup: (Rig) -> Unit): Rig {
        val rig = newRig()
        setup(rig)
        rig.flow.onResume()
        advanceUntilIdle()
        return rig
    }

    private fun Rig.hasWallet(pin: String = PIN) {
        store.meta = WalletMeta(false, false, MAIN_FINGERPRINT)
        store.vault = boundVault(pin)
        store.publicKeys = keysFor(MAIN_FINGERPRINT)
    }

    // 自检

    @Test
    fun aFailedCheckAtLaunchIsAWarningWithAWayIn() = runTest {
        val rig = boot {
            it.hasWallet()
            it.check.facts = facts(bootloaderLocked = false)
        }
        val flow = rig.flow
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.canAcknowledge)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(listOf(Screen.Home), flow.stack)
    }

    @Test
    fun anAcknowledgementCoversOneOperation() = runTest {
        val rig = readyRig(PIN)
        val flow = rig.flow
        rig.check.facts = facts(wifiOn = true)
        flow.export.open()
        advanceUntilIdle()
        flow.acknowledgeCheck()
        advanceUntilIdle()
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)

        flow.export.finish()
        flow.export.open()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
    }

    @Test
    fun recheckIsStillThereAndClearsTheWarning() = runTest {
        val rig = readyRig(PIN)
        val flow = rig.flow
        rig.check.facts = facts(wifiOn = true)
        flow.scan.open()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.retryCheck()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        passSelfCheck(rig)
        assertEquals(Screen.Home, flow.screen)
    }

    @Test
    fun manualWipeStaysReachableAfterAnAcknowledgement() = runTest {
        val rig = readyRig(PIN)
        val flow = rig.flow
        flow.openSettings()
        rig.check.facts = facts(wifiOn = true)
        flow.settings.startWipe()
        advanceUntilIdle()
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        submit(flow, PIN)
        assertEquals(listOf(Screen.Welcome), flow.stack)
        assertNull(rig.store.vault)
    }

    @Test
    fun aMissingKeyCacheWarnsOverTheUnlockPage() = runTest {
        val rig = readyRig(PIN, cached = false)
        val flow = rig.flow
        flow.receive.open(flow.session.accounts.first())
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)

        rig.check.facts = facts(wifiOn = true)
        flow.back()
        advanceUntilIdle()
        flow.receive.open(flow.session.accounts.first())
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        flow.acknowledgeCheck()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        submit(flow, PIN)
        assertEquals(Screen.Receive, flow.screen)
        assertNotNull(flow.receive.address)
    }

    @Test
    fun anUnreadableProbeIsAWarningNotAFailure() {
        val cases =
            listOf(
                GoodFacts.copy(wifiOn = null),
                GoodFacts.copy(airplaneMode = null),
                GoodFacts.copy(rooted = null),
                GoodFacts.copy(bootloaderLocked = null),
                GoodFacts.copy(adbEnabled = null),
                GoodFacts.copy(usbDataConnected = null),
                GoodFacts.copy(keystore = null),
                GoodFacts.copy(screenLockSet = null),
                GoodFacts.copy(thirdPartyAccessibility = null),
                GoodFacts.copy(overlayApps = null),
            )
        cases.forEach { fact ->
            val report = evaluate(fact)
            assertFalse(report.blocked, fact.toString())
            assertEquals(1, report.warned, fact.toString())
        }
    }

    @Test
    fun aDefiniteProblemBeatsAnUnreadableOneInTheSameRow() {
        val report = evaluate(GoodFacts.copy(wifiOn = null, bluetoothOn = true))
        assertEquals(CheckState.Fail, report.items.first().state)
        assertEquals("蓝牙已开启", report.items.first().value)
    }

    @Test
    fun bootloaderAttestation() {
        fun row(locked: Boolean?) = evaluate(GoodFacts.copy(bootloaderLocked = locked)).items[1]
        assertEquals(CheckState.Pass, row(true).state)
        assertEquals(CheckState.Warn, row(null).state)
        assertEquals("无法确认 Bootloader", row(null).value)
        assertEquals(CheckState.Fail, row(false).state)
    }

    @Test
    fun onlyDefiniteResultsAreRemembered() {
        var runs = 0
        val answers = listOf(null, false, true)
        val result = DefiniteResult { answers[runs++] }
        assertNull(result.get())
        assertEquals(false, result.get())
        assertEquals(false, result.get())
        assertEquals(2, runs)
    }

    // PIN

    @Test
    fun delaysStayCappedAndNeverEnd() {
        assertEquals(300_000L, pinDelayMs(9))
        assertEquals(300_000L, pinDelayMs(10_000))
    }

    @Test
    fun aTruncatedPinRecordReadsAsClean() {
        val bytes =
            ByteArray(20).also {
                it[3] = 3
                it[11] = 7
                it[19] = 9
            }
        val record = PinRecord.decode(bytes)
        assertEquals(listOf(3L, 7L, 9L), listOf(record.failures.toLong(), record.bootId, record.elapsedMs))
        assertEquals(PinRecord.Clean, PinRecord.decode(bytes.copyOf(19)))
        assertEquals(PinRecord.Clean, PinRecord.decode(ByteArray(0)))
        assertEquals(PinRecord.Clean, PinRecord.decode(null))
    }

    // 钱包是否存在

    @Test
    fun lostPreferencesNeverHideARealVault() = runTest {
        val rig = boot {
            it.store.vault = boundVault(PIN, PASSPHRASE)
            it.store.meta = null
        }
        val flow = rig.flow
        assertEquals(Screen.Home, flow.screen)

        // 当前格式的密文里有 BIP39 密码,只要 PIN.解开之后写回指纹和公钥.
        flow.export.open()
        advanceUntilIdle()
        assertFalse(flow.unlock.asksPassphrase)
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals(PASSPHRASE_FINGERPRINT, rig.store.meta?.fingerprint)
        assertEquals(true, rig.store.meta?.imported)
        assertContentEquals(keysFor(PASSPHRASE_FINGERPRINT), rig.store.publicKeys)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.session.fingerprint)
    }

    @Test
    fun noVaultMeansNoWalletWhateverThePreferencesSay() = runTest {
        val rig = boot { it.store.meta = WalletMeta(false, false, MAIN_FINGERPRINT) }
        assertEquals(Screen.Welcome, rig.flow.screen)
    }

    // 遮挡和摄像头

    @Test
    fun anObscuredTouchSaysWhy() = runTest {
        val rig = readyRig(PIN)
        rig.flow.touchObscured()
        assertEquals(true, rig.flow.notice?.contains("盖住"))
        assertTrue(rig.flow.noticeFailed)
    }

    @Test
    fun theNextCameraIsTriedWhenBindingFails() {
        val tried = mutableListOf<String>()
        firstBound(listOf("back", "front")) {
            tried += it
            check(it == "front")
        }
        assertEquals(listOf("back", "front"), tried)

        firstBound(listOf("back", "front")) { tried += it }
        assertEquals("back", tried.last())
        assertEquals(3, tried.size)

        assertFailsWith<IllegalStateException> { firstBound(listOf("back", "front")) { error("没有摄像头") } }
    }
}

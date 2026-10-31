package com.ccobcode.bitone

import androidx.compose.runtime.snapshots.Snapshot
import com.ccobcode.bitone.flow.ChecksumState
import com.ccobcode.bitone.flow.MAX_PIN_FAILURES
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.VerifyState
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.changedLockDelayMs
import com.ccobcode.bitone.flow.pinDelayMs
import com.ccobcode.bitone.platform.HardwareLevel
import com.ccobcode.bitone.platform.PinRecord
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.ui.theme.Motion
import com.ccobcode.bitone.ui.theme.Qr
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

private const val PIN = "123456"
private const val BAD_PIN = "000000"

@OptIn(ExperimentalCoroutinesApi::class)
class WalletFlowTest {

    /** backgroundScope 的任务不算在 advanceUntilIdle 里,所以单独建作用域. */
    private fun TestScope.rig(vararg setup: (Rig) -> Unit): Rig {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val scope = CoroutineScope(dispatcher + SupervisorJob() + CoroutineExceptionHandler { _, _ -> })
        val rig = Rig(scope, dispatcher)
        setup.forEach { it(rig) }
        return rig
    }

    private fun TestScope.boot(rig: Rig): WalletFlow {
        rig.flow.onResume()
        advanceUntilIdle()
        return rig.flow
    }

    private fun WalletFlow.typeSetPin(digits: String) = digits.forEach(pin::digit)

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

    private fun TestScope.createWallet(flow: WalletFlow) {
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
        flow.setup.finishSetup()
        advanceUntilIdle()
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

    private fun TestScope.tryPin(flow: WalletFlow, pin: String) {
        pin.forEach(flow.unlock::digit)
        flow.unlock.submit()
        advanceUntilIdle()
    }

    /** 等够延时再错一次. */
    private fun TestScope.failOnce(rig: Rig) {
        rig.clock.elapsed += pinDelayMs(rig.store.pin.failures)
        rig.flow.unlock.refreshStatus()
        tryPin(rig.flow, BAD_PIN)
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

        rig.check.facts = GoodFacts
        flow.retryCheck()
        advanceUntilIdle()
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
                facts(bootloaderLocked = null),
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
                it.store.meta = WalletMeta(false, false, "73C5DA0A")
                it.store.vault = ByteArray(8)
            })
        assertEquals(Screen.Home, boot(rig).screen)
    }

    @Test
    fun lostHardwareKeyNeedsRecoveryAndKeepsData() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(false, false, "73C5DA0A")
                it.store.vault = ByteArray(8)
                it.key.lost = true
            })
        assertEquals(Screen.NeedsRecovery, boot(rig).screen)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun recoveryImportReplacesLostWallet() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(false, false, "73C5DA0A")
                it.store.vault = "old".encodeToByteArray()
                it.store.pin = PinRecord(3, 1, 0)
                it.key.lost = true
            })
        val flow = boot(rig)
        flow.setup.startImport()
        advanceUntilIdle()
        assertEquals(Screen.ImportWords, flow.screen)
        Words12.forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
        flow.setup.continueAfterImport()
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
        flow.setup.finishSetup()
        advanceUntilIdle()
        assertEquals(Screen.Home, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(1, rig.key.deleted)
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
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
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
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
        flow.setup.finishSetup()
        advanceUntilIdle()
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
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
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
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
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
        Words12.forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
        assertEquals(ChecksumState.Valid, flow.setup.checksum)
        flow.setup.continueAfterImport()
        assertEquals(Screen.SetPin, flow.screen)
        assertTrue(flow.setup.isImportFlow)
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
        flow.setup.finishSetup()
        advanceUntilIdle()
        assertEquals(Screen.Home, flow.screen)
        assertEquals(true, rig.store.meta?.imported)
    }

    @Test
    fun badChecksumBlocksImport() = runTest {
        val flow = boot(rig())
        flow.setup.startImport()
        advanceUntilIdle()
        (Words12.dropLast(1) + "badword").forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
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
        (listOf("badword") + Words12.drop(1)).forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
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
        Words12.forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
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
        Words12.forEach {
            flow.setup.importPick(it)
            advanceUntilIdle()
        }
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
        repeat(4) { tryPin(flow, BAD_PIN) }
        assertEquals(4, flow.unlock.failures)
        assertEquals(0, flow.unlock.waitMs)
        assertEquals(Screen.Unlock, flow.screen)

        tryPin(flow, BAD_PIN)
        assertEquals(5, flow.unlock.failures)
        assertEquals(30_000, flow.unlock.waitMs)
    }

    @Test
    fun waitingBlocksInputAndDoesNotCount() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(5) { tryPin(flow, BAD_PIN) }
        val opens = rig.core.opens
        tryPin(flow, BAD_PIN)
        assertEquals("", flow.unlock.pin)
        assertEquals(5, rig.store.pin.failures)
        assertEquals(opens, rig.core.opens)

        rig.clock.elapsed += 29_000
        flow.unlock.refreshStatus()
        assertEquals(1_000, flow.unlock.waitMs)
        rig.clock.elapsed += 1_000
        flow.unlock.refreshStatus()
        assertEquals(0, flow.unlock.waitMs)
        tryPin(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
    }

    @Test
    fun rebootRestartsDelay() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(5) { tryPin(flow, BAD_PIN) }
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
        repeat(3) { tryPin(flow, BAD_PIN) }
        tryPin(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(0, flow.unlock.failures)
    }

    @Test
    fun tenthFailureWipes() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        repeat(MAX_PIN_FAILURES - 1) { failOnce(rig) }
        assertEquals(9, rig.store.pin.failures)
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(0, rig.key.deleted - 1)

        failOnce(rig)
        assertEquals(Screen.Wiped, flow.screen)
        assertNull(rig.store.vault)
        assertNull(rig.store.meta)
        assertNull(flow.session.meta)
        assertEquals(0, rig.store.pin.failures)
        assertEquals(2, rig.key.deleted)

        flow.restartAfterWipe()
        advanceUntilIdle()
        assertEquals(Screen.Welcome, flow.screen)
    }

    @Test
    fun killedDuringTenthAttemptWipesOnNextStart() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(false, false, "73C5DA0A")
                it.store.vault = boundVault(PIN)
                it.store.pin = PinRecord(MAX_PIN_FAILURES, 1, 0)
            })
        assertEquals(Screen.Wiped, boot(rig).screen)
        assertNull(rig.store.vault)
    }

    /** 硬件结果按次认证,所以每猜一次 PIN 都要过一次系统屏幕锁. */
    @Test
    fun everyAttemptConfirmsTheDeviceLock() = runTest {
        val rig = ready()
        val flow = rig.flow
        val before = rig.key.binds
        openUnlock(flow)
        tryPin(flow, BAD_PIN)
        assertEquals(before + 1, rig.key.binds)
        tryPin(flow, BAD_PIN)
        assertEquals(before + 2, rig.key.binds)

        // 不过手机锁就不算一次尝试.
        rig.key.cancelAuth = true
        tryPin(flow, BAD_PIN)
        assertEquals(2, rig.store.pin.failures)
        assertNotNull(flow.unlock.error)
    }

    @Test
    fun legacyVaultMigratesOnTheRightPin() = runTest {
        val rig = ready()
        rig.store.vault = legacyVault(PIN)
        val flow = rig.flow
        openUnlock(flow)
        tryPin(flow, BAD_PIN)
        assertContentEquals(legacyVault(PIN), rig.store.vault)
        assertEquals(0, rig.key.legacyDropped)
        tryPin(flow, PIN)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(1, rig.key.legacyDropped)
    }

    /** 迁移没走完时旧密文和旧密钥都在,下次还能解开. */
    @Test
    fun cancelledMigrationKeepsTheLegacyVault() = runTest {
        val rig = ready()
        rig.store.vault = legacyVault(PIN)
        val flow = rig.flow
        openUnlock(flow)
        rig.key.cancelFor = PIN
        tryPin(flow, PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertContentEquals(legacyVault(PIN), rig.store.vault)
        assertEquals(0, rig.key.legacyDropped)
        assertEquals(0, rig.store.pin.failures)

        rig.key.cancelFor = null
        tryPin(flow, PIN)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(1, rig.key.legacyDropped)
    }

    @Test
    fun legacyRetryConfirmsTheDeviceLockFirst() = runTest {
        val rig = ready()
        rig.store.vault = legacyVault(PIN)
        val flow = rig.flow
        openUnlock(flow)
        tryPin(flow, BAD_PIN)
        assertEquals(0, rig.key.confirmations)
        tryPin(flow, BAD_PIN)
        assertEquals(1, rig.key.confirmations)

        rig.key.cancelConfirm = true
        tryPin(flow, BAD_PIN)
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
    fun changedFingerprintWaitsAndNeverWipes() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.changed = true
        repeat(MAX_PIN_FAILURES + 3) {
            rig.clock.elapsed += changedLockDelayMs(rig.store.pin.failures)
            flow.unlock.refreshStatus()
            tryPin(flow, BAD_PIN)
        }
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(MAX_PIN_FAILURES - 1, rig.store.pin.failures)
        assertEquals(3_600_000, flow.unlock.waitMs)
        assertEquals(false, flow.session.canWipe)

        // 输对 PIN 后恢复.
        rig.clock.elapsed += 3_600_000
        flow.unlock.refreshStatus()
        tryPin(flow, PIN)
        assertEquals(false, rig.key.changed)
        assertEquals(true, flow.session.canWipe)
        assertEquals(0, rig.store.pin.failures)
    }

    @Test
    fun changedFingerprintDoesNotFinishAnInterruptedWipe() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(false, false, "73C5DA0A")
                it.store.vault = boundVault(PIN)
                it.store.pin = PinRecord(MAX_PIN_FAILURES, 1, 0)
                it.key.changed = true
            })
        assertEquals(Screen.Home, boot(rig).screen)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun hardwareFailureIsNotAPinFailure() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.lost = true
        tryPin(flow, BAD_PIN)
        assertEquals(Screen.NeedsRecovery, flow.screen)
        assertEquals(0, rig.store.pin.failures)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun cancelledScreenLockIsNotAPinFailure() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        rig.key.cancelAuth = true
        tryPin(flow, BAD_PIN)
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
        tryPin(flow, PIN)
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
        tryPin(flow, PIN)
        assertEquals(true, rig.core.exportedImported)
    }

    @Test
    fun framesCanBePausedAndChangeSpeed() = runTest {
        val rig = ready()
        val flow = rig.flow
        openUnlock(flow)
        tryPin(flow, PIN)
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
        tryPin(flow, PIN)
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

        rig.check.facts = GoodFacts
        flow.retryCheck()
        advanceUntilIdle()
        assertEquals(Screen.Home, flow.screen)
    }

    // BIP39 密码

    @Test
    fun passphraseWalletHasNoStoredFingerprint() = runTest {
        val rig = rig()
        val flow = boot(rig)
        toVerify(flow)
        answerAll(flow, correct = true)
        flow.setup.continueAfterVerify()
        flow.typeSetPin(PIN)
        flow.typeSetPin(PIN)
        flow.setup.finishSetup()
        advanceUntilIdle()
        assertEquals("73C5DA0A", rig.store.meta?.fingerprint)

        val second = rig()
        val secondFlow = boot(second)
        secondFlow.setup.startCreate()
        advanceUntilIdle()
        secondFlow.setup.togglePassphrase()
        secondFlow.setup.startVerify()
        advanceUntilIdle()
        answerAll(secondFlow, correct = true)
        secondFlow.setup.continueAfterVerify()
        secondFlow.typeSetPin(PIN)
        secondFlow.typeSetPin(PIN)
        secondFlow.setup.finishSetup()
        advanceUntilIdle()
        assertNull(second.store.meta?.fingerprint)
        assertEquals(true, second.store.meta?.passphrase)
        assertNull(secondFlow.session.fingerprint)
    }

    @Test
    fun passphraseIsRequiredAtUnlockAndNotStored() = runTest {
        val rig =
            rig({
                it.store.meta = WalletMeta(imported = false, passphrase = true, fingerprint = null)
                it.store.vault = boundVault(PIN)
            })
        val flow = boot(rig)
        openUnlock(flow)
        assertTrue(flow.session.passphraseRequired)
        PIN.forEach(flow.unlock::digit)
        assertFalse(flow.unlock.canSubmit)

        "Pass 1!".forEach(flow.unlock::passphraseInput)
        assertTrue(flow.unlock.canSubmit)
        flow.unlock.submit()
        advanceUntilIdle()
        assertEquals(Screen.Export, flow.screen)
        assertEquals("9E21B4C7", flow.session.fingerprint)
        assertEquals("", flow.unlock.passphrase)
        assertNull(rig.store.meta?.fingerprint)
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

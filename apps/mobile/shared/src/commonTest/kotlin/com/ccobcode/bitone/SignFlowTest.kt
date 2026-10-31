package com.ccobcode.bitone

import com.ccobcode.bitone.flow.Accounts
import com.ccobcode.bitone.flow.ReviewStyle
import com.ccobcode.bitone.flow.ScanResult
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.UnlockPurpose
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.flow.addressChunks
import com.ccobcode.bitone.flow.pinDelayMs
import com.ccobcode.bitone.flow.reviewStyle
import com.ccobcode.bitone.flow.shortAddress
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.platform.WalletMeta
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
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
private const val NEW_PIN = "654321"
private const val REQUEST = "bitone-request"
private const val ADDRESS = "bitone-address"
private const val MANIFEST = "bitone-manifest"

@OptIn(ExperimentalCoroutinesApi::class)
class SignFlowTest {

    private fun TestScope.rig(setup: (Rig) -> Unit = {}): Rig {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val scope = CoroutineScope(dispatcher + SupervisorJob() + CoroutineExceptionHandler { _, _ -> })
        return Rig(scope, dispatcher).also(setup)
    }

    /** 已有钱包的状态，不走一遍创建流程。 */
    private fun TestScope.ready(passphrase: Boolean = false, cached: Boolean = true, setup: (Rig) -> Unit = {}): Rig {
        val rig = rig {
            it.store.meta = WalletMeta(false, passphrase, if (passphrase) null else MAIN_FINGERPRINT)
            it.store.vault = boundVault(PIN)
            if (!passphrase && cached) it.store.publicKeys = keysFor(MAIN_FINGERPRINT)
            setup(it)
        }
        rig.flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Home, rig.flow.screen)
        return rig
    }

    @Test
    fun aStrayCodeMidScanKeepsTheProgress() = runTest {
        val rig = ready()
        openScan(rig.flow)
        val parts = frames(REQUEST)
        feed(rig.flow, parts.take(1))
        val progress = rig.flow.scanProgress

        feed(rig.flow, listOf("https://example.com", "ur:bitone-pairing/abc"))
        assertEquals(Screen.Scan, rig.flow.screen)
        assertEquals(progress, rig.flow.scanProgress)

        feed(rig.flow, parts.drop(1))
        assertEquals(Screen.Review, rig.flow.screen)
    }

    @Test
    fun aStrayCodeBeforeAnyFrameIsReported() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, listOf("https://example.com"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
    }

    private fun TestScope.openScan(flow: WalletFlow) {
        flow.openScan()
        advanceUntilIdle()
    }

    private fun TestScope.feed(flow: WalletFlow, parts: List<String>) = parts.forEach {
        flow.onScanned(it)
        advanceUntilIdle()
    }

    private fun TestScope.scanRequest(rig: Rig) {
        openScan(rig.flow)
        assertEquals(Screen.Scan, rig.flow.screen)
        feed(rig.flow, frames(REQUEST))
    }

    /** 收齐后直接进 A6，中间没有摘要。 */
    private fun TestScope.toReview(rig: Rig) {
        scanRequest(rig)
        assertEquals(Screen.Review, rig.flow.screen)
        assertEquals(listOf(Screen.Home, Screen.Review), rig.flow.stack)
    }

    private fun TestScope.toPin(rig: Rig) {
        toReview(rig)
        rig.flow.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.Sign, rig.flow.unlockPurpose)
    }

    private fun TestScope.submit(flow: WalletFlow, pin: String, passphrase: String = "") {
        pin.forEach(flow::unlockDigit)
        passphrase.forEach(flow::passphraseInput)
        flow.submitUnlock()
        advanceUntilIdle()
    }

    // 页面分支

    @Test
    fun everyRiskPicksItsPage() {
        assertEquals(ReviewStyle.Plain, reviewStyle(reviewOf(Risk.Transfer)))
        assertEquals(ReviewStyle.Plain, reviewStyle(reviewOf(Risk.Recognized)))
        assertEquals(ReviewStyle.Unknown, reviewStyle(reviewOf(Risk.Unknown, ConfirmMode.Hold)))
        assertEquals(ReviewStyle.Approval, reviewStyle(reviewOf(Risk.Approval, ConfirmMode.Hold)))
        assertEquals(ReviewStyle.Permission, reviewStyle(reviewOf(Risk.PermissionChange, ConfirmMode.Hold)))
        assertEquals(ReviewStyle.HighRisk, reviewStyle(reviewOf(Risk.Transfer, ConfirmMode.Hold, listOf("手续费明显偏高"))))
        assertEquals(ReviewStyle.HighRisk, reviewStyle(reviewOf(Risk.Recognized, ConfirmMode.Hold, listOf("手续费明显偏高"))))
        // 类别高于告警，授权带告警仍然是 A6b。
        assertEquals(ReviewStyle.Approval, reviewStyle(reviewOf(Risk.Approval, ConfirmMode.Hold, listOf("参数异常"))))
    }

    @Test
    fun eachSignableRiskReachesTheReviewScreen() = runTest {
        listOf(
                reviewOf(Risk.Transfer),
                reviewOf(Risk.Recognized),
                reviewOf(Risk.Unknown, ConfirmMode.Hold),
                reviewOf(Risk.Approval, ConfirmMode.Hold),
                reviewOf(Risk.PermissionChange, ConfirmMode.Hold),
                reviewOf(Risk.Transfer, ConfirmMode.Hold, listOf("手续费明显偏高")),
            )
            .forEach { review ->
                val rig = ready { it.core.review = review }
                toReview(rig)
                assertEquals(review, rig.flow.review)
            }
    }

    // 确认方式

    @Test
    fun tapGoesToPinAndHoldDoesNothing() = runTest {
        val rig = ready()
        toReview(rig)
        rig.flow.holdStart()
        advanceTimeBy(3_100)
        runCurrent()
        assertEquals(Screen.Review, rig.flow.screen)

        rig.flow.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
    }

    @Test
    fun holdNeedsThreeFullSeconds() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Approval, ConfirmMode.Hold) }
        toReview(rig)
        val flow = rig.flow

        flow.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Review, flow.screen)

        flow.holdStart()
        advanceTimeBy(2_900)
        runCurrent()
        assertTrue(flow.holdProgress > 0.9f)
        flow.holdCancel()
        assertEquals(0f, flow.holdProgress)
        advanceTimeBy(5_000)
        runCurrent()
        assertEquals(Screen.Review, flow.screen)

        flow.holdStart()
        advanceTimeBy(3_000)
        runCurrent()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.Sign, flow.unlockPurpose)
    }

    @Test
    fun goingToBackgroundCancelsHold() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Unknown, ConfirmMode.Hold) }
        toReview(rig)
        rig.flow.holdStart()
        advanceTimeBy(1_000)
        rig.flow.onBackground()
        advanceTimeBy(5_000)
        runCurrent()
        assertEquals(Screen.Review, rig.flow.screen)
    }

    @Test
    fun refuseGoesToForbiddenAndCannotReachPin() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Forbidden, ConfirmMode.Refuse, listOf("没有 Chain ID")) }
        scanRequest(rig)
        val flow = rig.flow
        assertEquals(Screen.Forbidden, flow.screen)
        assertEquals("没有 Chain ID", flow.refusal)
        assertNull(flow.review)

        // 绕过页面直接调用后面的步骤。
        flow.confirmTap()
        flow.holdStart()
        advanceTimeBy(4_000)
        runCurrent()
        PIN.forEach(flow::unlockDigit)
        flow.submitUnlock()
        advanceUntilIdle()
        assertEquals(Screen.Forbidden, flow.screen)
        assertEquals(0, rig.core.opens)
        assertTrue(rig.core.signCalls.isEmpty())

        flow.exitRequest()
        assertEquals(Screen.Home, flow.screen)
    }

    @Test
    fun confirmRefuseWinsEvenWithALowRisk() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Transfer, ConfirmMode.Refuse) }
        scanRequest(rig)
        assertEquals(Screen.Forbidden, rig.flow.screen)
    }

    @Test
    fun rejectDropsTheRequest() = runTest {
        val rig = ready()
        toReview(rig)
        rig.flow.exitRequest()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.flow.review)
        assertEquals(0, rig.core.opens)
    }

    // 扫码

    @Test
    fun framesInAnyOrderShowProgress() = runTest {
        val rig = ready()
        val flow = rig.flow
        openScan(flow)
        val parts = frames(REQUEST, 4).reversed()
        feed(flow, parts.take(1))
        assertEquals(4, flow.scanFrames)
        assertEquals(0.25f, flow.scanProgress)
        assertNull(flow.scanResult)

        // 相机连续交来同一帧时不重复处理。
        feed(flow, parts.take(1) + parts.take(1))
        assertEquals(0.25f, flow.scanProgress)

        feed(flow, parts.drop(1))
        assertIs<ScanResult.Request>(flow.scanResult)
        assertTrue(rig.core.lastReceiver!!.closed)
    }

    @Test
    fun blockedSelfCheckCannotScan() = runTest {
        val rig = ready()
        rig.check.facts = facts(wifiOn = true)
        openScan(rig.flow)
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertNull(rig.core.lastReceiver)
    }

    @Test
    fun cameraDenied() = runTest {
        val rig = ready {
            it.scanner.granted = false
            it.scanner.grantOnRequest = false
        }
        val flow = rig.flow
        openScan(flow)
        assertEquals(Screen.CameraDenied, flow.screen)
        assertFalse(flow.cameraBroken)
        flow.openCameraSettings()
        assertEquals(1, rig.scanner.settingsOpened)

        // 从系统设置授权回来，直接进扫码。
        rig.scanner.granted = true
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Scan, flow.screen)
    }

    @Test
    fun cameraFailureIsNotAPermissionProblem() = runTest {
        val rig = ready()
        openScan(rig.flow)
        rig.flow.cameraFailed()
        assertEquals(Screen.CameraDenied, rig.flow.screen)
        assertTrue(rig.flow.cameraBroken)
    }

    @Test
    fun garbageFrameGoesToParseFailed() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, listOf("bc1qnotaframe"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("二维码帧无效", rig.flow.scanFailure)

        rig.flow.rescan()
        advanceUntilIdle()
        assertEquals(Screen.Scan, rig.flow.screen)
        assertEquals(listOf(Screen.Home, Screen.Scan), rig.flow.stack)
    }

    @Test
    fun framesOfOtherDirectionsAreRejected() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, frames("bitone-response", 1))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
    }

    // 指纹

    @Test
    fun fingerprintMismatchShowsItsOwnState() = runTest {
        val rig = ready { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        assertEquals(Screen.FingerprintMismatch, rig.flow.screen)
        assertFalse(rig.flow.passphraseRequired)
        assertEquals(PASSPHRASE_FINGERPRINT, rig.flow.mismatchFingerprint)
        assertEquals(0, rig.core.opens)

        rig.flow.exitRequest()
        assertNull(rig.flow.mismatchFingerprint)
    }

    @Test
    fun passphraseCanBeRetriedAndReviewAgain() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        // 内存里没有公钥，先输密码读公钥。
        scanRequest(rig)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlockPurpose)
        assertTrue(flow.unlockNeedsPassphrase)

        // 第一次输错，算出的指纹和请求对不上。
        rig.core.requestFingerprint = "00000000"
        submit(flow, PIN, "wrong")
        assertEquals(Screen.FingerprintMismatch, flow.screen)

        rig.core.requestFingerprint = PASSPHRASE_FINGERPRINT
        flow.retryPassphrase()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        submit(flow, PIN, "right")
        assertEquals(Screen.Review, flow.screen)
        assertIs<ScanResult.Request>(flow.scanResult)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.fingerprint)
    }

    @Test
    fun passphraseKeysStayInMemory() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        assertEquals(Screen.Review, rig.flow.screen)
        assertNull(rig.store.publicKeys)
        assertNull(rig.store.meta?.fingerprint)

        rig.flow.confirmTap()
        advanceUntilIdle()
        assertTrue(rig.flow.unlockNeedsPassphrase)
        submit(rig.flow, PIN, "pass")
        assertEquals(Screen.Signed, rig.flow.screen)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun wrongPassphraseAtSigningIsCaughtBeforeSigning() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        rig.flow.confirmTap()
        advanceUntilIdle()

        // 这次解开的钱包和核对时不是同一个。
        rig.core.requestFingerprint = "00000000"
        submit(rig.flow, PIN, "typo")
        assertEquals(Screen.FingerprintMismatch, rig.flow.screen)
        assertEquals("00000000", rig.flow.mismatchFingerprint)
        assertTrue(rig.core.signCalls.isEmpty())
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun cancelLoadingKeysAbandonsTheScan() = runTest {
        val rig = ready(passphrase = true)
        scanRequest(rig)
        assertEquals(Screen.Unlock, rig.flow.screen)
        rig.flow.back()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.flow.scanResult)
    }

    @Test
    fun walletWithoutCachedKeysFillsTheCacheOnce() = runTest {
        val rig = ready(cached = false)
        scanRequest(rig)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, rig.flow.unlockPurpose)
        submit(rig.flow, PIN)
        assertEquals(Screen.Review, rig.flow.screen)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    // 签名

    @Test
    fun signingProducesFramesAndClosesTheWallet() = runTest {
        val rig = ready()
        toPin(rig)
        val flow = rig.flow
        submit(flow, PIN)
        assertEquals(Screen.Signed, flow.screen)
        assertTrue(rig.core.lastWallet!!.closed)
        assertEquals("request", rig.core.signCalls.single().message)
        assertEquals(keysFor(MAIN_FINGERPRINT).decodeToString(), rig.core.signCalls.single().keys)
        assertEquals(2, flow.frameTotal)

        val stream = rig.core.lastStream!!
        val player = launch { flow.playFrames() }
        advanceTimeBy(300)
        assertTrue(stream.sent > 0)
        flow.finishSigned()
        advanceTimeBy(1_000)
        player.join()
        assertTrue(stream.closed)
        assertEquals(Screen.Home, flow.screen)
        assertNull(flow.review)
    }

    @Test
    fun selfCheckRunsAgainRightBeforeSigning() = runTest {
        val rig = ready()
        toPin(rig)
        val flow = rig.flow
        rig.check.facts = facts(adbEnabled = true)
        submit(flow, PIN)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertEquals(0, rig.core.opens)

        // 处理好之后回到 A7，请求还在。
        rig.check.facts = GoodFacts
        flow.retryCheck()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(flow.review)
        submit(flow, PIN)
        assertEquals(Screen.Signed, flow.screen)
    }

    @Test
    fun selfCheckBlocksConfirm() = runTest {
        val rig = ready()
        toReview(rig)
        rig.check.facts = facts(wifiOn = true)
        rig.flow.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
    }

    @Test
    fun wrongPinKeepsTheRequest() = runTest {
        val rig = ready()
        toPin(rig)
        val flow = rig.flow
        submit(flow, BAD_PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(1, flow.pinFailures)
        assertNotNull(flow.review)
        assertTrue(rig.core.signCalls.isEmpty())

        // 回 A6 再来也一样。
        flow.back()
        assertEquals(Screen.Review, flow.screen)
        flow.confirmTap()
        advanceUntilIdle()
        submit(flow, PIN)
        assertEquals(Screen.Signed, flow.screen)
        assertEquals("request", rig.core.signCalls.single().message)
    }

    @Test
    fun tenthWrongPinDropsTheRequest() = runTest {
        val rig = ready()
        toPin(rig)
        val flow = rig.flow
        repeat(10) {
            rig.clock.elapsed += pinDelayMs(rig.store.pin.failures)
            flow.refreshPinStatus()
            submit(flow, BAD_PIN)
        }
        assertEquals(Screen.Wiped, flow.screen)
        assertNull(flow.review)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun signingRefusalAfterPinClosesTheWallet() = runTest {
        val rig = ready()
        toPin(rig)
        rig.core.signFails = true
        submit(rig.flow, PIN)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertNotNull(rig.flow.unlockError)
        assertTrue(rig.core.lastWallet!!.closed)
        assertNotNull(rig.flow.review)
    }

    @Test
    fun coreRefusalAtSigningTimeGoesToForbidden() = runTest {
        val rig = ready()
        toPin(rig)
        rig.core.review = reviewOf(Risk.Forbidden, ConfirmMode.Refuse, listOf("地址与派生路径不符"))
        submit(rig.flow, PIN)
        assertEquals(Screen.Forbidden, rig.flow.screen)
        assertTrue(rig.core.signCalls.isEmpty())
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun hardwareLossDropsTheRequest() = runTest {
        val rig = ready()
        toPin(rig)
        rig.key.lost = true
        submit(rig.flow, PIN)
        assertEquals(Screen.NeedsRecovery, rig.flow.screen)
        assertNull(rig.flow.review)
    }

    // 最高找零索引

    @Test
    fun changeIndexIsWrittenAndOnlyGoesUp() = runTest {
        val rig = ready()
        val flow = rig.flow

        rig.core.nextChangeIndex = 7u
        toPin(rig)
        submit(flow, PIN)
        assertEquals(7u, rig.store.highestChange)
        flow.finishSigned()

        // 下一笔核对和签名都带上 7。
        rig.core.nextChangeIndex = 3u
        toPin(rig)
        assertEquals(7u, rig.core.reviewedHighestChange.last())
        submit(flow, PIN)
        assertEquals(7u, rig.core.signCalls.last().highestChange)
        assertEquals(7u, rig.store.highestChange)
        assertEquals(1, rig.store.highestChangeWrites)
        flow.finishSigned()

        rig.core.nextChangeIndex = 9u
        toPin(rig)
        submit(flow, PIN)
        assertEquals(9u, rig.store.highestChange)
    }

    @Test
    fun noChangeIndexWritesNothing() = runTest {
        val rig = ready()
        toPin(rig)
        submit(rig.flow, PIN)
        assertEquals(0, rig.store.highestChangeWrites)
    }

    @Test
    fun changeIndexIsNotWrittenBeforeSigning() = runTest {
        val rig = ready()
        toPin(rig)
        submit(rig.flow, BAD_PIN)
        assertEquals(0u, rig.store.highestChange)
    }

    @Test
    fun testnetIndexDoesNotLoosenMainnet() = runTest {
        val rig = ready {
            it.store.highestChangeTestnet = 50u
            it.store.highestChange = 3u
        }
        toPin(rig)
        assertEquals(3u, rig.core.reviewedHighestChange.last())
    }

    @Test
    fun newWalletResetsIndexAndKeys() = runTest {
        val rig = ready { it.store.highestChange = 40u }
        rig.store.saveWallet("v".encodeToByteArray(), WalletMeta(true, false, "11111111"), null)
        assertEquals(0u, rig.store.highestChange)
        assertNull(rig.store.publicKeys)
    }

    // A9

    @Test
    fun addressBelongsAndFinishes() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertIs<ScanResult.Address>(rig.flow.scanResult)
        assertEquals(Screen.CheckAddress, rig.flow.screen)
        assertEquals(true, rig.flow.addressReview?.belongs)
        assertEquals(0, rig.core.opens)

        rig.flow.exitRequest()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.flow.addressReview)
    }

    @Test
    fun addressThatIsNotMineStillShowsBoth() = runTest {
        val rig = ready { it.core.address = addressOf(belongs = false, address = "bc1qother") }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(false, rig.flow.addressReview?.belongs)
        assertEquals("bc1qmine", rig.flow.addressReview?.derived)
    }

    @Test
    fun addressOfAnotherWalletGoesToMismatch() = runTest {
        val rig = ready { it.core.address = addressOf(belongs = false, fingerprintMismatch = true) }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(Screen.FingerprintMismatch, rig.flow.screen)
        assertNull(rig.flow.mismatchFingerprint)
    }

    @Test
    fun badAddressRequestGoesToParseFailed() = runTest {
        val rig = ready { it.core.addressError = "路径不在白名单" }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("路径不在白名单", rig.flow.scanFailure)
    }

    // A3

    @Test
    fun homeShowsAddressesFromTheCache() = runTest {
        val rig = ready()
        rig.flow.loadAddresses()
        advanceUntilIdle()
        val row = Accounts.first()
        assertEquals("m/84'/0'/0':keys:$MAIN_FINGERPRINT", rig.flow.addresses[row.path])
        // 三条 EVM 账户的路径相同，地址也相同。
        assertTrue(Accounts.all { it.path in rig.flow.addresses })
        assertEquals(Accounts, rig.flow.accounts)
    }

    @Test
    fun walletWithoutKeysShowsNoAddresses() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.addresses.isEmpty())
    }

    @Test
    fun shortAndChunkedAddresses() {
        assertEquals("0x7099…79C8", shortAddress("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"))
        assertEquals("bc1qcr…6fyu", shortAddress("bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu"))
        assertEquals("TQ7n…P4hW", shortAddress("TQ7nE2hVwYcK9mRb4LfXp3ZsA6dGu1P4hW"))
        assertEquals(listOf("0x7099", "7970", "C5"), addressChunks("0x70997970C5"))
        assertEquals(listOf("bc1q", "cr8t", "e4"), addressChunks("bc1qcr8te4"))
    }

    // 清单

    @Test
    fun manifestUpdateIsVerifiedThenPassedToReview() = runTest {
        val rig = ready { it.core.manifestVersions["pack"] = 2u }
        val flow = rig.flow
        flow.openSettings()
        flow.openManifest()
        flow.loadManifest()
        advanceUntilIdle()
        assertEquals(1u, flow.manifestVersion)

        openScan(flow)
        feed(flow, frames(MANIFEST, 2, "pack"))
        assertEquals(listOf(Screen.Settings, Screen.Manifest), flow.stack)
        assertEquals(2u, assertIs<ScanResult.Manifest>(flow.scanResult).version)
        assertNull(rig.store.manifest)

        flow.applyManifest()
        advanceUntilIdle()
        assertEquals(UnlockPurpose.ApplyManifest, flow.unlockPurpose)
        submit(flow, PIN)
        assertEquals(Screen.Manifest, flow.screen)
        assertContentEquals("pack".encodeToByteArray(), rig.store.manifest)
        assertEquals(2u, flow.manifestVersion)
        assertNull(flow.scanResult)

        flow.openHome()
        toPin(rig)
        assertEquals("pack", rig.core.reviewedManifests.last())
        submit(flow, PIN)
        assertEquals("pack", rig.core.signCalls.single().manifest)
    }

    @Test
    fun staleManifestIsRejected() = runTest {
        val rig = ready {
            it.core.manifestVersions["old"] = 1u
            it.store.manifest = "newer".encodeToByteArray()
            it.core.manifestVersions["newer"] = 3u
        }
        openScan(rig.flow)
        feed(rig.flow, frames(MANIFEST, 1, "old"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("清单版本过旧", rig.flow.scanFailure)
        assertContentEquals("newer".encodeToByteArray(), rig.store.manifest)
    }

    @Test
    fun badManifestSignatureIsRejected() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, frames(MANIFEST, 1, "forged"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("签名无效", rig.flow.scanFailure)
        assertNull(rig.store.manifest)
    }

    @Test
    fun corruptStoredManifestFallsBackToBuiltin() = runTest {
        val rig = ready { it.store.manifest = "corrupt".encodeToByteArray() }
        toReview(rig)
        assertNull(rig.core.reviewedManifests.last())
    }

    // 修改 PIN

    private fun TestScope.enterNewPin(flow: WalletFlow) {
        flow.startChangePin()
        advanceUntilIdle()
        assertEquals(Screen.SetPin, flow.screen)
        assertTrue(flow.changingPin)
        NEW_PIN.forEach(flow::setPinDigit)
        NEW_PIN.forEach(flow::setPinDigit)
        assertTrue(flow.pinDone)
        flow.continueChangePin()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.ChangePin, flow.unlockPurpose)
    }

    @Test
    fun changePinReplacesTheVaultOnly() = runTest {
        val rig = ready { it.store.highestChange = 12u }
        val flow = rig.flow
        flow.openSettings()
        enterNewPin(flow)
        submit(flow, PIN)
        assertEquals(Screen.Settings, flow.screen)
        assertFalse(flow.changingPin)
        assertContentEquals(boundVault(NEW_PIN), rig.store.vault)
        assertTrue(rig.core.lastWallet!!.closed)
        // 助记词不经过 JVM。
        assertEquals(0, rig.core.wordsRead)
        assertNull(rig.core.lastDraft)
        // 钱包的其余状态都不动，硬件密钥也不能删。
        assertEquals(0, rig.key.deleted)
        assertEquals(12u, rig.store.highestChange)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
        assertEquals(false, rig.store.meta?.imported)
        assertEquals(MAIN_FINGERPRINT, rig.store.meta?.fingerprint)
    }

    @Test
    fun wrongCurrentPinChangesNothing() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        enterNewPin(flow)
        submit(flow, BAD_PIN)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(1, rig.store.pin.failures)
        assertContentEquals(boundVault(PIN), rig.store.vault)
    }

    @Test
    fun cancelledScreenLockKeepsOldVault() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        enterNewPin(flow)
        submitWithCancelledWrap(rig)
        assertContentEquals(boundVault(PIN), rig.store.vault)
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(flow.unlockError)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    private fun TestScope.submitWithCancelledWrap(rig: Rig) {
        rig.key.cancelFor = NEW_PIN
        submit(rig.flow, PIN)
    }

    @Test
    fun changingPinNeedsNoPassphrase() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.openSettings()
        enterNewPin(rig.flow)
        assertFalse(rig.flow.unlockNeedsPassphrase)
        submit(rig.flow, PIN)
        assertContentEquals(boundVault(NEW_PIN), rig.store.vault)
    }

    @Test
    fun backgroundAbandonsChangingPin() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        enterNewPin(flow)
        flow.onBackground()
        assertEquals(Screen.Settings, flow.screen)
        assertFalse(flow.changingPin)
        assertEquals("", flow.pinFirst)
    }

    @Test
    fun selfCheckBlocksChangingPin() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.check.facts = facts(thirdPartyAccessibility = 1)
        rig.flow.startChangePin()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
    }

    // 查看助记词

    @Test
    fun viewingWordsNeedsPinAndClosesTheWallet() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.startViewWords()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertTrue(flow.viewWords.isEmpty())

        submit(flow, BAD_PIN)
        assertTrue(flow.viewWords.isEmpty())

        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)
        assertEquals(Words12, flow.viewWords)
        assertTrue(rig.core.lastWallet!!.closed)

        flow.back()
        assertEquals(Screen.Settings, flow.screen)
        assertTrue(flow.viewWords.isEmpty())
    }

    @Test
    fun wordsLeaveMemoryInBackground() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.startViewWords()
        advanceUntilIdle()
        submit(flow, PIN)
        flow.onBackground()
        assertEquals(Screen.Settings, flow.screen)
        assertTrue(flow.viewWords.isEmpty())
    }

    @Test
    fun failedSelfCheckClosesTheWordsPage() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.startViewWords()
        advanceUntilIdle()
        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)

        // 没有经过后台，比如下拉通知栏时开了 Wi-Fi。
        rig.check.facts = facts(wifiOn = true)
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.viewWords.isEmpty())

        rig.check.facts = GoodFacts
        flow.retryCheck()
        advanceUntilIdle()
        assertEquals(Screen.Settings, flow.screen)
    }

    @Test
    fun selfCheckBlocksViewingWords() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.check.facts = facts(thirdPartyAccessibility = 1)
        rig.flow.startViewWords()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertEquals(0, rig.core.opens)
    }

    @Test
    fun viewingWordsNeedsNoPassphrase() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.openSettings()
        rig.flow.startViewWords()
        advanceUntilIdle()
        assertFalse(rig.flow.unlockNeedsPassphrase)
        submit(rig.flow, PIN)
        assertEquals(Screen.ViewWords, rig.flow.screen)
    }

    // 新钱包和抹除

    @Test
    fun setupCachesPublicKeysExceptForPassphrase() = runTest {
        val rig = rig()
        rig.flow.onResume()
        advanceUntilIdle()
        rig.flow.startImport()
        advanceUntilIdle()
        Words12.forEach {
            rig.flow.importPick(it)
            advanceUntilIdle()
        }
        rig.flow.continueAfterImport()
        PIN.forEach(rig.flow::setPinDigit)
        PIN.forEach(rig.flow::setPinDigit)
        rig.flow.finishSetup()
        advanceUntilIdle()
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun passphraseWalletNeverWritesKeysAtSetup() = runTest {
        val rig = rig()
        rig.flow.onResume()
        advanceUntilIdle()
        rig.flow.startImport()
        advanceUntilIdle()
        Words12.forEach {
            rig.flow.importPick(it)
            advanceUntilIdle()
        }
        rig.flow.togglePassphrase()
        rig.flow.continueAfterImport()
        PIN.forEach(rig.flow::setPinDigit)
        PIN.forEach(rig.flow::setPinDigit)
        rig.flow.finishSetup()
        advanceUntilIdle()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun wipeForgetsSessionKeys() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        rig.flow.exitRequest()
        rig.flow.openSettings()
        rig.flow.startWipe()
        advanceUntilIdle()
        submit(rig.flow, PIN)
        assertEquals(Screen.Welcome, rig.flow.screen)
        assertNull(rig.flow.fingerprint)
        rig.flow.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.addresses.isEmpty())
    }

    // 导出

    @Test
    fun exportFillsTheKeyCache() = runTest {
        val rig = ready(cached = false)
        rig.flow.openExport()
        advanceUntilIdle()
        submit(rig.flow, PIN)
        assertEquals(Screen.Export, rig.flow.screen)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
    }
}

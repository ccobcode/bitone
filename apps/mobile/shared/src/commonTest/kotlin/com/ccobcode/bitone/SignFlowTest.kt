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
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
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

    private fun TestScope.rig(setup: (Rig) -> Unit = {}) = newRig().also(setup)

    private fun TestScope.ready(passphrase: Boolean = false, cached: Boolean = true, setup: (Rig) -> Unit = {}) =
        readyRig(PIN, passphrase, cached, setup)

    private fun TestScope.openViewWords(flow: WalletFlow) {
        flow.openSettings()
        flow.settings.startViewWords()
        advanceUntilIdle()
    }

    @Test
    fun aStrayCodeMidScanKeepsTheProgress() = runTest {
        val rig = ready()
        openScan(rig.flow)
        val parts = frames(REQUEST)
        feed(rig.flow, parts.take(1))
        val progress = rig.flow.scan.progress

        feed(rig.flow, listOf("https://example.com", "ur:bitone-pairing/abc"))
        assertEquals(Screen.Scan, rig.flow.screen)
        assertEquals(progress, rig.flow.scan.progress)

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

    private fun TestScope.scanRequest(rig: Rig) {
        openScan(rig.flow)
        assertEquals(Screen.Scan, rig.flow.screen)
        feed(rig.flow, frames(REQUEST))
    }

    /** 收齐后直接进 A6,中间没有摘要. */
    private fun TestScope.toReview(rig: Rig) {
        scanRequest(rig)
        assertEquals(Screen.Review, rig.flow.screen)
        assertEquals(listOf(Screen.Home, Screen.Review), rig.flow.stack)
        readToTheEnd(rig)
    }

    private fun readToTheEnd(rig: Rig) = rig.flow.scan.reviewScrolled(canScrollForward = false)

    private fun TestScope.toPin(rig: Rig) {
        toReview(rig)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.Sign, rig.flow.unlock.purpose)
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
        // 类别高于告警,授权带告警仍然是 A6b.
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
                assertEquals(review, rig.flow.scan.review)
            }
    }

    // 确认方式

    @Test
    fun tapGoesToPinAndHoldDoesNothing() = runTest {
        val rig = ready()
        toReview(rig)
        rig.flow.scan.holdStart()
        advanceTimeBy(3_100)
        runCurrent()
        assertEquals(Screen.Review, rig.flow.screen)

        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
    }

    @Test
    fun holdNeedsThreeFullSeconds() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Approval, ConfirmMode.Hold) }
        toReview(rig)
        val flow = rig.flow

        flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Review, flow.screen)

        flow.scan.holdStart()
        advanceTimeBy(2_900)
        runCurrent()
        assertTrue(flow.scan.holdProgress > 0.9f)
        flow.scan.holdCancel()
        assertEquals(0f, flow.scan.holdProgress)
        advanceTimeBy(5_000)
        runCurrent()
        assertEquals(Screen.Review, flow.screen)

        flow.scan.holdStart()
        advanceTimeBy(3_000)
        runCurrent()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.Sign, flow.unlock.purpose)
    }

    @Test
    fun tapWaitsForTheEndOfTheReview() = runTest {
        val rig = ready()
        scanRequest(rig)
        rig.flow.scan.reviewScrolled(canScrollForward = true)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Review, rig.flow.screen)

        readToTheEnd(rig)
        // 展开详情后内容变长,看过的不收回.
        rig.flow.scan.reviewScrolled(canScrollForward = true)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
    }

    @Test
    fun holdWaitsForTheEndOfTheReview() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Approval, ConfirmMode.Hold) }
        scanRequest(rig)
        rig.flow.scan.holdStart()
        advanceTimeBy(3_100)
        runCurrent()
        assertEquals(0f, rig.flow.scan.holdProgress)
        assertEquals(Screen.Review, rig.flow.screen)

        readToTheEnd(rig)
        rig.flow.scan.holdStart()
        advanceTimeBy(3_100)
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
    }

    @Test
    fun aNewRequestHasToBeReadAgain() = runTest {
        val rig = ready()
        toReview(rig)
        rig.flow.scan.exitRequest()
        scanRequest(rig)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertEquals(Screen.Review, rig.flow.screen)
    }

    @Test
    fun goingToBackgroundCancelsHold() = runTest {
        val rig = ready { it.core.review = reviewOf(Risk.Unknown, ConfirmMode.Hold) }
        toReview(rig)
        rig.flow.scan.holdStart()
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
        assertEquals("没有 Chain ID", flow.scan.refusal)
        assertNull(flow.scan.review)

        // 绕过页面直接调用后面的步骤.
        flow.scan.confirmTap()
        flow.scan.holdStart()
        advanceTimeBy(4_000)
        runCurrent()
        PIN.forEach(flow.unlock::digit)
        flow.unlock.submit()
        advanceUntilIdle()
        assertEquals(Screen.Forbidden, flow.screen)
        assertEquals(0, rig.core.opens)
        assertTrue(rig.core.signCalls.isEmpty())

        flow.scan.exitRequest()
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
        rig.flow.scan.exitRequest()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.flow.scan.review)
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
        assertEquals(4, flow.scan.frames)
        assertEquals(0.25f, flow.scan.progress)
        assertNull(flow.scan.result)

        // 相机连续交来同一帧时不重复处理.
        feed(flow, parts.take(1) + parts.take(1))
        assertEquals(0.25f, flow.scan.progress)

        feed(flow, parts.drop(1))
        assertIs<ScanResult.Request>(flow.scan.result)
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
        assertFalse(flow.scan.cameraBroken)
        flow.scan.openCameraSettings()
        assertEquals(1, rig.scanner.settingsOpened)

        // 从系统设置授权回来,直接进扫码.
        rig.scanner.granted = true
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Scan, flow.screen)
    }

    @Test
    fun cameraFailureIsNotAPermissionProblem() = runTest {
        val rig = ready()
        openScan(rig.flow)
        rig.flow.scan.cameraFailed()
        assertEquals(Screen.CameraDenied, rig.flow.screen)
        assertTrue(rig.flow.scan.cameraBroken)
    }

    @Test
    fun garbageFrameGoesToParseFailed() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, listOf("bc1qnotaframe"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("二维码帧无效", rig.flow.scan.failure)

        rig.flow.scan.rescan()
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
        assertFalse(rig.flow.session.passphraseRequired)
        assertEquals(PASSPHRASE_FINGERPRINT, rig.flow.scan.mismatchFingerprint)
        assertEquals(0, rig.core.opens)

        rig.flow.scan.exitRequest()
        assertNull(rig.flow.scan.mismatchFingerprint)
    }

    @Test
    fun passphraseCanBeRetriedAndReviewAgain() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        // 内存里没有公钥,先输密码读公钥.
        scanRequest(rig)
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlock.purpose)
        assertTrue(flow.unlock.needsPassphrase)

        // 第一次输错,算出的指纹和请求对不上.
        rig.core.requestFingerprint = "00000000"
        submit(flow, PIN, "wrong")
        assertEquals(Screen.FingerprintMismatch, flow.screen)

        rig.core.requestFingerprint = PASSPHRASE_FINGERPRINT
        flow.scan.retryPassphrase()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        submit(flow, PIN, "right")
        assertEquals(Screen.Review, flow.screen)
        assertIs<ScanResult.Request>(flow.scan.result)
        assertEquals(PASSPHRASE_FINGERPRINT, flow.session.fingerprint)
    }

    @Test
    fun passphraseKeysStayInMemory() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        assertEquals(Screen.Review, rig.flow.screen)
        assertNull(rig.store.publicKeys)
        assertNull(rig.store.meta?.fingerprint)

        readToTheEnd(rig)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()
        assertTrue(rig.flow.unlock.needsPassphrase)
        submit(rig.flow, PIN, "pass")
        assertEquals(Screen.Signed, rig.flow.screen)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun wrongPassphraseAtSigningIsCaughtBeforeSigning() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        readToTheEnd(rig)
        rig.flow.scan.confirmTap()
        advanceUntilIdle()

        // 这次解开的钱包和核对时不是同一个.
        rig.core.requestFingerprint = "00000000"
        submit(rig.flow, PIN, "typo")
        assertEquals(Screen.FingerprintMismatch, rig.flow.screen)
        assertEquals("00000000", rig.flow.scan.mismatchFingerprint)
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
        assertNull(rig.flow.scan.result)
    }

    @Test
    fun walletWithoutCachedKeysFillsTheCacheOnce() = runTest {
        val rig = ready(cached = false)
        scanRequest(rig)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, rig.flow.unlock.purpose)
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
        assertEquals(2, flow.player.total)

        val stream = rig.core.lastStream!!
        val player = launch { flow.player.play() }
        advanceTimeBy(300)
        assertTrue(stream.sent > 0)
        flow.scan.finishSigned()
        advanceTimeBy(1_000)
        player.join()
        assertTrue(stream.closed)
        assertEquals(Screen.Home, flow.screen)
        assertNull(flow.scan.review)
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

        // 处理好之后回到 A7,请求还在.
        passSelfCheck(rig)
        assertEquals(Screen.Unlock, flow.screen)
        assertNotNull(flow.scan.review)
        submit(flow, PIN)
        assertEquals(Screen.Signed, flow.screen)
    }

    @Test
    fun aFailureDuringUnlockKeepsTheSignatureOffTheScreen() = runTest {
        val rig = ready()
        toPin(rig)
        val flow = rig.flow
        rig.core.onOpen = { rig.check.facts = facts(wifiOn = true) }
        submit(flow, PIN)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertEquals(1, rig.core.signCalls.size)
        assertTrue(rig.core.lastStream!!.closed)

        // 处理好之后回到 A7,可以重新签名.
        rig.core.onOpen = {}
        passSelfCheck(rig)
        assertEquals(Screen.Unlock, flow.screen)
        submit(flow, PIN)
        assertEquals(Screen.Signed, flow.screen)
    }

    @Test
    fun selfCheckBlocksConfirm() = runTest {
        val rig = ready()
        toReview(rig)
        rig.check.facts = facts(wifiOn = true)
        rig.flow.scan.confirmTap()
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
        assertEquals(1, flow.unlock.failures)
        assertNotNull(flow.scan.review)
        assertTrue(rig.core.signCalls.isEmpty())

        // 回 A6 再来也一样.
        flow.back()
        assertEquals(Screen.Review, flow.screen)
        flow.scan.confirmTap()
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
            flow.unlock.refreshStatus()
            submit(flow, BAD_PIN)
        }
        assertEquals(Screen.Wiped, flow.screen)
        assertNull(flow.scan.review)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun signingRefusalAfterPinClosesTheWallet() = runTest {
        val rig = ready()
        toPin(rig)
        rig.core.signFails = true
        submit(rig.flow, PIN)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertNotNull(rig.flow.unlock.error)
        assertTrue(rig.core.lastWallet!!.closed)
        assertNotNull(rig.flow.scan.review)
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
        assertNull(rig.flow.scan.review)
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
        flow.scan.finishSigned()

        // 下一笔核对和签名都带上 7.
        rig.core.nextChangeIndex = 3u
        toPin(rig)
        assertEquals(7u, rig.core.reviewedHighestChange.last())
        submit(flow, PIN)
        assertEquals(7u, rig.core.signCalls.last().highestChange)
        assertEquals(7u, rig.store.highestChange)
        assertEquals(1, rig.store.highestChangeWrites)
        flow.scan.finishSigned()

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
        assertIs<ScanResult.Address>(rig.flow.scan.result)
        assertEquals(Screen.CheckAddress, rig.flow.screen)
        assertEquals(true, rig.flow.scan.addressReview?.belongs)
        assertEquals(0, rig.core.opens)

        rig.flow.scan.exitRequest()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.flow.scan.addressReview)
    }

    @Test
    fun addressThatIsNotMineStillShowsBoth() = runTest {
        val rig = ready { it.core.address = addressOf(belongs = false, address = "bc1qother") }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(false, rig.flow.scan.addressReview?.belongs)
        assertEquals("bc1qmine", rig.flow.scan.addressReview?.derived)
    }

    @Test
    fun addressOfAnotherWalletGoesToMismatch() = runTest {
        val rig = ready { it.core.address = addressOf(belongs = false, fingerprintMismatch = true) }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(Screen.FingerprintMismatch, rig.flow.screen)
        assertNull(rig.flow.scan.mismatchFingerprint)
    }

    @Test
    fun badAddressRequestGoesToParseFailed() = runTest {
        val rig = ready { it.core.addressError = "路径不在白名单" }
        openScan(rig.flow)
        feed(rig.flow, frames(ADDRESS, 1))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("路径不在白名单", rig.flow.scan.failure)
    }

    // A3

    @Test
    fun homeShowsAddressesFromTheCache() = runTest {
        val rig = ready()
        rig.flow.session.loadAddresses()
        advanceUntilIdle()
        val row = Accounts.first()
        assertEquals("m/84'/0'/0':keys:$MAIN_FINGERPRINT", rig.flow.session.addresses[row.path])
        // 三条 EVM 账户的路径相同,地址也相同.
        assertTrue(Accounts.all { it.path in rig.flow.session.addresses })
        assertEquals(Accounts, rig.flow.session.accounts)
    }

    @Test
    fun walletWithoutKeysShowsNoAddresses() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.session.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.session.addresses.isEmpty())
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
        flow.manifest.open()
        flow.manifest.load()
        advanceUntilIdle()
        assertEquals(1u, flow.manifest.version)

        openScan(flow)
        feed(flow, frames(MANIFEST, 2, "pack"))
        assertEquals(listOf(Screen.Settings, Screen.Manifest), flow.stack)
        assertEquals(2u, assertIs<ScanResult.Manifest>(flow.scan.result).version)
        assertNull(rig.store.manifest)

        flow.scan.applyManifest()
        advanceUntilIdle()
        assertEquals(UnlockPurpose.ApplyManifest, flow.unlock.purpose)
        submit(flow, PIN)
        assertEquals(Screen.Manifest, flow.screen)
        assertContentEquals("pack".encodeToByteArray(), rig.store.manifest)
        assertEquals(2u, flow.manifest.version)
        assertNull(flow.scan.result)

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
        assertEquals("清单版本过旧", rig.flow.scan.failure)
        assertContentEquals("newer".encodeToByteArray(), rig.store.manifest)
    }

    @Test
    fun badManifestSignatureIsRejected() = runTest {
        val rig = ready()
        openScan(rig.flow)
        feed(rig.flow, frames(MANIFEST, 1, "forged"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("签名无效", rig.flow.scan.failure)
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
        flow.settings.startChangePin()
        advanceUntilIdle()
        assertEquals(Screen.SetPin, flow.screen)
        assertTrue(flow.settings.changingPin)
        NEW_PIN.forEach(flow.pin::digit)
        NEW_PIN.forEach(flow.pin::digit)
        assertTrue(flow.pin.done)
        flow.settings.continueChangePin()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.ChangePin, flow.unlock.purpose)
    }

    @Test
    fun changePinReplacesTheVaultOnly() = runTest {
        val rig = ready { it.store.highestChange = 12u }
        val flow = rig.flow
        flow.openSettings()
        enterNewPin(flow)
        submit(flow, PIN)
        assertEquals(Screen.Settings, flow.screen)
        assertFalse(flow.settings.changingPin)
        assertContentEquals(boundVault(NEW_PIN), rig.store.vault)
        assertTrue(rig.core.lastWallet!!.closed)
        // 助记词不经过 JVM.
        assertEquals(0, rig.core.wordsRead)
        assertNull(rig.core.lastDraft)
        // 钱包的其余状态都不动,硬件密钥也不能删.
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
        assertNotNull(flow.unlock.error)
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
        assertFalse(rig.flow.unlock.needsPassphrase)
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
        assertFalse(flow.settings.changingPin)
        assertEquals("", flow.pin.first)
    }

    @Test
    fun selfCheckBlocksChangingPin() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.check.facts = facts(thirdPartyAccessibility = 1)
        rig.flow.settings.startChangePin()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
    }

    // 查看助记词

    @Test
    fun viewingWordsNeedsPinAndClosesTheWallet() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        assertEquals(Screen.Unlock, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())

        submit(flow, BAD_PIN)
        assertTrue(flow.settings.viewWords.isEmpty())

        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)
        assertEquals(Words12, flow.settings.viewWords)
        assertTrue(rig.core.lastWallet!!.closed)

        flow.back()
        assertEquals(Screen.Settings, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())
    }

    @Test
    fun wordsLeaveMemoryInBackground() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        submit(flow, PIN)
        flow.onBackground()
        assertEquals(Screen.Settings, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())
    }

    @Test
    fun homeDuringUnlockLeavesNoWordsBehind() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        rig.core.onOpen = flow::onBackground
        submit(flow, PIN)
        assertEquals(Screen.Settings, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())
    }

    @Test
    fun screenLockPromptDuringUnlockStillShowsTheWords() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        rig.core.onOpen = {
            flow.onBackground()
            flow.onResume()
        }
        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)
        assertEquals(Words12, flow.settings.viewWords)
    }

    @Test
    fun aFailureDuringUnlockKeepsTheWordsOffTheScreen() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        rig.core.onOpen = { rig.check.facts = facts(wifiOn = true) }
        submit(flow, PIN)
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())
    }

    @Test
    fun failedSelfCheckClosesTheWordsPage() = runTest {
        val rig = ready()
        val flow = rig.flow
        openViewWords(flow)
        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)

        // 没有经过后台,比如下拉通知栏时开了 Wi-Fi.
        rig.check.facts = facts(wifiOn = true)
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, flow.screen)
        assertTrue(flow.settings.viewWords.isEmpty())

        passSelfCheck(rig)
        assertEquals(Screen.Settings, flow.screen)
    }

    @Test
    fun selfCheckBlocksViewingWords() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.check.facts = facts(thirdPartyAccessibility = 1)
        rig.flow.settings.startViewWords()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertEquals(0, rig.core.opens)
    }

    @Test
    fun viewingWordsNeedsNoPassphrase() = runTest {
        val rig = ready(passphrase = true)
        openViewWords(rig.flow)
        assertFalse(rig.flow.unlock.needsPassphrase)
        submit(rig.flow, PIN)
        assertEquals(Screen.ViewWords, rig.flow.screen)
    }

    // 新钱包和抹除

    @Test
    fun setupCachesPublicKeysExceptForPassphrase() = runTest {
        val rig = rig()
        rig.flow.onResume()
        advanceUntilIdle()
        rig.flow.setup.startImport()
        advanceUntilIdle()
        Words12.forEach {
            rig.flow.setup.importPick(it)
            advanceUntilIdle()
        }
        rig.flow.setup.continueAfterImport()
        PIN.forEach(rig.flow.pin::digit)
        PIN.forEach(rig.flow.pin::digit)
        rig.flow.setup.finishSetup()
        advanceUntilIdle()
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun passphraseWalletNeverWritesKeysAtSetup() = runTest {
        val rig = rig()
        rig.flow.onResume()
        advanceUntilIdle()
        rig.flow.setup.startImport()
        advanceUntilIdle()
        Words12.forEach {
            rig.flow.setup.importPick(it)
            advanceUntilIdle()
        }
        rig.flow.setup.togglePassphrase()
        rig.flow.setup.continueAfterImport()
        PIN.forEach(rig.flow.pin::digit)
        PIN.forEach(rig.flow.pin::digit)
        rig.flow.setup.finishSetup()
        advanceUntilIdle()
        assertEquals(Screen.Home, rig.flow.screen)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun wipeForgetsSessionKeys() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        scanRequest(rig)
        submit(rig.flow, PIN, "pass")
        rig.flow.scan.exitRequest()
        rig.flow.openSettings()
        rig.flow.settings.startWipe()
        advanceUntilIdle()
        submit(rig.flow, PIN)
        assertEquals(Screen.Welcome, rig.flow.screen)
        assertNull(rig.flow.session.fingerprint)
        rig.flow.session.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.session.addresses.isEmpty())
    }

    // 导出

    @Test
    fun exportFillsTheKeyCache() = runTest {
        val rig = ready(cached = false)
        rig.flow.export.open()
        advanceUntilIdle()
        submit(rig.flow, PIN)
        assertEquals(Screen.Export, rig.flow.screen)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
    }
}

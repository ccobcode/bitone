package com.bitone.wallet

import com.bitone.wallet.flow.AccountRow
import com.bitone.wallet.flow.Accounts
import com.bitone.wallet.flow.MAX_PIN_FAILURES
import com.bitone.wallet.flow.Network
import com.bitone.wallet.flow.ScanResult
import com.bitone.wallet.flow.Screen
import com.bitone.wallet.flow.UnlockPurpose
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.flow.accountsFor
import com.bitone.wallet.platform.AccountAddress
import com.bitone.wallet.platform.PinRecord
import com.bitone.wallet.platform.WalletMeta
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
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest

private const val PIN = "739264"
private const val BAD_PIN = "000000"
private const val MANIFEST = "bitone-manifest"
private const val REQUEST = "bitone-request"

/** 抹除、更新清单、弱 PIN、收款页和 BIP39 密码钱包的公钥。 */
@OptIn(ExperimentalCoroutinesApi::class)
class GuardedActionsTest {
    private fun TestScope.ready(passphrase: Boolean = false, setup: (Rig) -> Unit = {}): Rig {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val scope = CoroutineScope(dispatcher + SupervisorJob() + CoroutineExceptionHandler { _, _ -> })
        val rig = Rig(scope, dispatcher)
        rig.store.meta = WalletMeta(false, passphrase, if (passphrase) null else MAIN_FINGERPRINT)
        rig.store.vault = "hw:sealed:$PIN".encodeToByteArray()
        if (!passphrase) rig.store.publicKeys = keysFor(MAIN_FINGERPRINT)
        setup(rig)
        rig.flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Home, rig.flow.screen)
        return rig
    }

    private fun TestScope.submit(flow: WalletFlow, pin: String, passphrase: String = "") {
        pin.forEach(flow::unlockDigit)
        passphrase.forEach(flow::passphraseInput)
        flow.submitUnlock()
        advanceUntilIdle()
    }

    private fun TestScope.feed(flow: WalletFlow, parts: List<String>) = parts.forEach {
        flow.onScanned(it)
        advanceUntilIdle()
    }

    private fun TestScope.toWipePin(rig: Rig) {
        rig.flow.openSettings()
        rig.flow.startWipe()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.Wipe, rig.flow.unlockPurpose)
    }

    private fun TestScope.toManifestPin(rig: Rig) {
        scanManifest(rig)
        rig.flow.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.ApplyManifest, rig.flow.unlockPurpose)
    }

    private fun TestScope.scanManifest(rig: Rig) {
        rig.core.manifestVersions["pack"] = 2u
        rig.flow.openSettings()
        rig.flow.openManifest()
        rig.flow.openScan()
        advanceUntilIdle()
        feed(rig.flow, frames(MANIFEST, 2, "pack"))
        assertEquals(Screen.Manifest, rig.flow.screen)
    }

    // 抹除

    @Test
    fun wipeNeedsThePin() = runTest {
        val rig = ready()
        toWipePin(rig)
        assertNotNull(rig.store.vault)
        submit(rig.flow, PIN)
        assertEquals(Screen.Welcome, rig.flow.screen)
        assertNull(rig.store.vault)
        assertTrue(rig.key.deleted > 0)
        assertTrue(rig.core.lastWallet!!.closed)
    }

    @Test
    fun wrongPinDoesNotWipeAndCounts() = runTest {
        val rig = ready()
        toWipePin(rig)
        submit(rig.flow, BAD_PIN)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(0, rig.key.deleted)
        assertEquals(1, rig.store.pin.failures)
        assertEquals(1, rig.flow.pinFailures)
    }

    @Test
    fun cancelDoesNotWipe() = runTest {
        val rig = ready()
        toWipePin(rig)
        rig.flow.back()
        assertEquals(Screen.Settings, rig.flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(0, rig.key.deleted)
    }

    @Test
    fun blockedSelfCheckStopsWipe() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.check.facts = facts(wifiOn = true)
        rig.flow.startWipe()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertNotNull(rig.store.vault)
    }

    @Test
    fun selfCheckRunsAgainBeforeTheWalletOpens() = runTest {
        val rig = ready()
        toWipePin(rig)
        rig.check.facts = facts(wifiOn = true)
        submit(rig.flow, PIN)
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(0, rig.core.opens)
    }

    @Test
    fun lastWrongPinStillWipesThroughTheGuard() = runTest {
        val rig = ready {
            it.store.pin = PinRecord(MAX_PIN_FAILURES - 1, it.clock.boot, 0)
            it.clock.elapsed = 3_600_000
        }
        toWipePin(rig)
        submit(rig.flow, BAD_PIN)
        assertEquals(Screen.Wiped, rig.flow.screen)
        assertNull(rig.store.vault)
    }

    @Test
    fun passphraseWalletWipesWithPinAlone() = runTest {
        val rig = ready(passphrase = true)
        toWipePin(rig)
        assertFalse(rig.flow.unlockNeedsPassphrase)
        submit(rig.flow, PIN)
        assertEquals(Screen.Welcome, rig.flow.screen)
    }

    // 更新清单

    @Test
    fun manifestNeedsThePin() = runTest {
        val rig = ready()
        toManifestPin(rig)
        assertNull(rig.store.manifest)
        submit(rig.flow, PIN)
        assertEquals(listOf(Screen.Settings, Screen.Manifest), rig.flow.stack)
        assertContentEquals("pack".encodeToByteArray(), rig.store.manifest)
        assertEquals(2u, rig.flow.manifestVersion)
    }

    @Test
    fun wrongPinDoesNotWriteManifest() = runTest {
        val rig = ready()
        toManifestPin(rig)
        submit(rig.flow, BAD_PIN)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertNull(rig.store.manifest)
        assertEquals(1, rig.store.pin.failures)
    }

    @Test
    fun cancelKeepsThePendingManifestUnwritten() = runTest {
        val rig = ready()
        toManifestPin(rig)
        rig.flow.back()
        assertEquals(Screen.Manifest, rig.flow.screen)
        assertNull(rig.store.manifest)
        assertIs<ScanResult.Manifest>(rig.flow.scanResult)

        // 离开 A12 就放弃这个包，不留到下一次。
        rig.flow.back()
        assertEquals(Screen.Settings, rig.flow.screen)
        assertNull(rig.flow.scanResult)
        rig.flow.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.Settings, rig.flow.screen)
    }

    @Test
    fun blockedSelfCheckStopsManifest() = runTest {
        val rig = ready()
        scanManifest(rig)
        rig.check.facts = facts(wifiOn = true)
        rig.flow.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertNull(rig.store.manifest)
    }

    @Test
    fun passphraseWalletUpdatesManifestWithPinAlone() = runTest {
        val rig = ready(passphrase = true)
        toManifestPin(rig)
        assertFalse(rig.flow.unlockNeedsPassphrase)
        submit(rig.flow, PIN)
        assertContentEquals("pack".encodeToByteArray(), rig.store.manifest)
    }

    @Test
    fun manifestPageDoesNotOpenTheCamera() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.flow.openManifest()
        rig.flow.loadManifest()
        advanceUntilIdle()
        assertEquals(Screen.Manifest, rig.flow.screen)
        assertNull(rig.core.lastReceiver)
        assertEquals(1u, rig.flow.manifestVersion)
        assertEquals(listOf("ETH", "USDT"), rig.flow.manifestEntries.map { it.symbol })
    }

    @Test
    fun badPackageGoesToParseFailed() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.flow.openManifest()
        rig.flow.openScan()
        advanceUntilIdle()
        feed(rig.flow, frames(MANIFEST, 1, "forged"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("签名无效", rig.flow.scanFailure)
        assertNull(rig.store.manifest)
    }

    // 弱 PIN

    @Test
    fun weakPinIsRefusedOnTheFirstEntry() = runTest {
        val rig = ready()
        rig.core.weakPins = setOf("111111")
        val flow = rig.flow
        flow.openSettings()
        flow.startChangePin()
        advanceUntilIdle()
        "111111".forEach(flow::setPinDigit)
        assertTrue(flow.pinWeak)
        assertFalse(flow.pinConfirming)
        assertEquals("", flow.pinInput)

        flow.setPinDigit('7')
        assertFalse(flow.pinWeak)
        "39264".forEach(flow::setPinDigit)
        assertTrue(flow.pinConfirming)
        assertFalse(flow.pinWeak)
    }

    @Test
    fun secondEntryIsNotCheckedAgainstTheWeakList() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.startChangePin()
        advanceUntilIdle()
        PIN.forEach(flow::setPinDigit)
        rig.core.weakPins = setOf(PIN)
        PIN.forEach(flow::setPinDigit)
        assertTrue(flow.pinDone)
    }

    // BIP39 密码钱包的公钥

    @Test
    fun backgroundForgetsSessionKeys() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        flow.openScan()
        advanceUntilIdle()
        feed(flow, frames(REQUEST))
        submit(flow, PIN, "pass")
        assertEquals(Screen.Review, flow.screen)
        flow.exitRequest()

        flow.onBackground()
        flow.openScan()
        advanceUntilIdle()
        feed(flow, frames(REQUEST))
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlockPurpose)
    }

    @Test
    fun backgroundKeepsKeysWhileBusy() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        flow.openScan()
        advanceUntilIdle()
        feed(flow, frames(REQUEST))
        PIN.forEach(flow::unlockDigit)
        "pass".forEach(flow::passphraseInput)
        flow.submitUnlock()
        assertTrue(flow.busy)
        flow.onBackground()
        advanceUntilIdle()
        assertEquals(Screen.Review, flow.screen)
        flow.exitRequest()

        flow.openScan()
        advanceUntilIdle()
        feed(flow, frames(REQUEST))
        assertEquals(Screen.Review, flow.screen)
    }

    @Test
    fun cachedKeysSurviveBackground() = runTest {
        val rig = ready()
        rig.flow.onBackground()
        rig.flow.openScan()
        advanceUntilIdle()
        feed(rig.flow, frames(REQUEST))
        assertEquals(Screen.Review, rig.flow.screen)
    }

    @Test
    fun unreadableKeyCacheIsRegeneratedAfterUnlock() = runTest {
        val stale = "old-format".encodeToByteArray()
        val rig = ready {
            it.store.publicKeys = stale
            it.core.unreadableKeys = stale
        }
        rig.flow.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.addresses.isEmpty())
        rig.flow.openScan()
        advanceUntilIdle()
        feed(rig.flow, frames(REQUEST))
        assertEquals(UnlockPurpose.LoadKeys, rig.flow.unlockPurpose)
        submit(rig.flow, PIN)
        assertEquals(Screen.Review, rig.flow.screen)
        assertContentEquals(keysFor(MAIN_FINGERPRINT), rig.store.publicKeys)
    }

    // A11

    private val bitcoin = Accounts.first()
    private val ethereum = Accounts.first { it.network == Network.Ethereum }

    @Test
    fun receiveShowsTheAddressAsPlainQrText() = runTest {
        val rig = ready()
        rig.flow.openReceive(ethereum)
        advanceUntilIdle()
        assertEquals(listOf(Screen.Receive), rig.flow.stack)
        assertEquals(ethereum, rig.flow.receiveAccount)
        assertEquals("${ethereum.path}#0:keys:$MAIN_FINGERPRINT", rig.flow.receiveAddress)
        assertEquals(rig.flow.receiveAddress, rig.core.lastQrText)
        assertNotNull(rig.flow.receiveQr)
        assertEquals(0, rig.core.opens)
    }

    @Test
    fun navigationKeepsTheLastAccount() = runTest {
        val rig = ready()
        rig.flow.openReceive()
        advanceUntilIdle()
        assertEquals(rig.flow.accounts.first(), rig.flow.receiveAccount)
        rig.flow.selectReceive(ethereum)
        advanceUntilIdle()
        rig.flow.openHome()
        rig.flow.openReceive()
        advanceUntilIdle()
        assertEquals(ethereum, rig.flow.receiveAccount)
        assertTrue(rig.flow.back())
        assertEquals(Screen.Home, rig.flow.screen)
    }

    @Test
    fun nextAddressOnlyGoesUpAndIsPerAccount() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openReceive(bitcoin)
        advanceUntilIdle()
        assertTrue(flow.receiveHasNext)
        flow.nextReceiveAddress()
        advanceUntilIdle()
        flow.nextReceiveAddress()
        advanceUntilIdle()
        assertEquals(2u, rig.store.readReceiveIndex(bitcoin.path))
        assertEquals("${bitcoin.path}#2:keys:$MAIN_FINGERPRINT", flow.receiveAddress)

        val taproot = Accounts[1]
        flow.selectReceive(taproot)
        advanceUntilIdle()
        assertEquals("${taproot.path}#0:keys:$MAIN_FINGERPRINT", flow.receiveAddress)

        // 重新进来还是上次的序号，不会退回去。
        flow.openHome()
        flow.openReceive(bitcoin)
        advanceUntilIdle()
        assertEquals("${bitcoin.path}#2:keys:$MAIN_FINGERPRINT", flow.receiveAddress)
    }

    @Test
    fun oldAddressesStayListedAndViewingOneKeepsTheIndex() = runTest {
        val rig = ready()
        val flow = rig.flow
        fun address(index: Int) = "${bitcoin.path}#$index:keys:$MAIN_FINGERPRINT"
        flow.openReceive(bitcoin)
        advanceUntilIdle()
        flow.nextReceiveAddress()
        advanceUntilIdle()
        assertEquals(listOf(address(1), address(0)), flow.receiveHistory)

        flow.openReceiveAddresses()
        assertEquals(Screen.ReceiveAddresses, flow.screen)
        assertTrue(flow.back())
        assertEquals(Screen.Receive, flow.screen)

        flow.openReceiveAddresses()
        flow.showReceive(address(0))
        advanceUntilIdle()
        assertEquals(Screen.Receive, flow.screen)
        assertEquals(address(0), flow.receiveAddress)
        assertEquals(1u, rig.store.readReceiveIndex(bitcoin.path))

        // 看过旧地址后，使用新地址仍然是没发放过的那个。
        flow.nextReceiveAddress()
        advanceUntilIdle()
        assertEquals(address(2), flow.receiveAddress)
        assertEquals(listOf(address(2), address(1), address(0)), flow.receiveHistory)
    }

    @Test
    fun onlyBitcoinHasANextAddress() = runTest {
        val rig = ready()
        rig.flow.openReceive(ethereum)
        advanceUntilIdle()
        assertFalse(rig.flow.receiveHasNext)
        rig.flow.nextReceiveAddress()
        advanceUntilIdle()
        assertEquals(0u, rig.store.readReceiveIndex(ethereum.path))
    }

    @Test
    fun newWalletStartsReceivingFromTheFirstAddress() = runTest {
        val rig = ready { it.store.receiveIndexes[bitcoin.path] = 5u }
        rig.store.saveWallet("v".encodeToByteArray(), WalletMeta(false, false, MAIN_FINGERPRINT), null)
        assertEquals(0u, rig.store.readReceiveIndex(bitcoin.path))
    }

    @Test
    fun passphraseWalletLoadsKeysBeforeReceiving() = runTest {
        val rig = ready(passphrase = true)
        val flow = rig.flow
        flow.openReceive(ethereum)
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlockPurpose)
        assertTrue(flow.unlockNeedsPassphrase)
        submit(flow, PIN, "pass")
        assertEquals(Screen.Receive, flow.screen)
        assertEquals("${ethereum.path}#0:keys:$PASSPHRASE_FINGERPRINT", flow.receiveAddress)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun cancelLoadingKeysLeavesReceive() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.openReceive(ethereum)
        advanceUntilIdle()
        rig.flow.back()
        assertEquals(listOf(Screen.Home), rig.flow.stack)
    }

    @Test
    fun receiveAsksAgainAfterBackgroundForPassphraseWallets() = runTest {
        val rig = ready(passphrase = true)
        val flow = rig.flow
        flow.openReceive(ethereum)
        advanceUntilIdle()
        submit(flow, PIN, "pass")
        flow.onBackground()
        assertNull(flow.receiveAddress)
        assertNull(flow.receiveQr)
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlockPurpose)
    }

    // 账户列表

    @Test
    fun accountsComeFromTheCore() {
        val rows =
            listOf(
                AccountAddress("bitcoin", "Native SegWit", "m/84'/0'/0'", "a"),
                AccountAddress("evm", "单地址", "m/44'/60'/0'/0/0", "b"),
                AccountAddress("bitcoin", "Legacy", "m/44'/0'/0'", "c"),
            )
        assertEquals(
            listOf("Bitcoin SegWit", "Ethereum", "BNB Smart Chain", "Polygon", "Bitcoin Legacy"),
            accountsFor(rows).map(AccountRow::name),
        )
    }
}

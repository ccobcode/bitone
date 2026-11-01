package com.ccobcode.bitone

import com.ccobcode.bitone.flow.AccountRow
import com.ccobcode.bitone.flow.Accounts
import com.ccobcode.bitone.flow.Network
import com.ccobcode.bitone.flow.ScanResult
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.UnlockPurpose
import com.ccobcode.bitone.flow.accountsFor
import com.ccobcode.bitone.platform.AccountAddress
import com.ccobcode.bitone.platform.PinRecord
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
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest

private const val PIN = "739264"
private const val BAD_PIN = "000000"
private const val MANIFEST = "bitone-manifest"
private const val REQUEST = "bitone-request"

/** 抹除,更新清单,弱 PIN,收款页和 BIP39 密码钱包的公钥. */
@OptIn(ExperimentalCoroutinesApi::class)
class GuardedActionsTest {
    private fun TestScope.ready(passphrase: Boolean = false, setup: (Rig) -> Unit = {}) =
        readyRig(PIN, passphrase, setup = setup)

    private fun TestScope.toWipePin(rig: Rig) {
        rig.flow.openSettings()
        rig.flow.settings.startWipe()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.Wipe, rig.flow.unlock.purpose)
    }

    private fun TestScope.toManifestPin(rig: Rig) {
        scanManifest(rig)
        rig.flow.scan.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertEquals(UnlockPurpose.ApplyManifest, rig.flow.unlock.purpose)
    }

    private fun TestScope.scanManifest(rig: Rig) {
        rig.core.manifestVersions["pack"] = 2u
        rig.flow.openSettings()
        rig.flow.manifest.open()
        openScan(rig.flow)
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
        assertEquals(1, rig.flow.unlock.failures)
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
        rig.flow.settings.startWipe()
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
    fun wrongPinsOnTheWipePageNeverErase() = runTest {
        val rig = ready {
            it.store.pin = PinRecord(40, it.clock.boot, 0)
            it.clock.elapsed = 3_600_000
        }
        toWipePin(rig)
        submit(rig.flow, BAD_PIN)
        assertEquals(Screen.Unlock, rig.flow.screen)
        assertNotNull(rig.store.vault)
        assertEquals(41, rig.store.pin.failures)
    }

    @Test
    fun passphraseWalletWipesWithPinAlone() = runTest {
        val rig = ready(passphrase = true)
        toWipePin(rig)
        assertFalse(rig.flow.unlock.needsPassphrase)
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
        assertEquals(2u, rig.flow.manifest.version)
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
        assertIs<ScanResult.Manifest>(rig.flow.scan.result)

        // 离开 A12 就放弃这个包,不留到下一次.
        rig.flow.back()
        assertEquals(Screen.Settings, rig.flow.screen)
        assertNull(rig.flow.scan.result)
        rig.flow.scan.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.Settings, rig.flow.screen)
    }

    @Test
    fun blockedSelfCheckStopsManifest() = runTest {
        val rig = ready()
        scanManifest(rig)
        rig.check.facts = facts(wifiOn = true)
        rig.flow.scan.applyManifest()
        advanceUntilIdle()
        assertEquals(Screen.SelfCheckFailed, rig.flow.screen)
        assertNull(rig.store.manifest)
    }

    @Test
    fun passphraseWalletUpdatesManifestWithPinAlone() = runTest {
        val rig = ready(passphrase = true)
        toManifestPin(rig)
        assertFalse(rig.flow.unlock.needsPassphrase)
        submit(rig.flow, PIN)
        assertContentEquals("pack".encodeToByteArray(), rig.store.manifest)
    }

    @Test
    fun manifestPageDoesNotOpenTheCamera() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.flow.manifest.open()
        rig.flow.manifest.load()
        advanceUntilIdle()
        assertEquals(Screen.Manifest, rig.flow.screen)
        assertNull(rig.core.lastReceiver)
        assertEquals(1u, rig.flow.manifest.version)
        assertEquals(listOf("ETH", "USDT"), rig.flow.manifest.entries.map { it.symbol })
    }

    @Test
    fun badPackageGoesToParseFailed() = runTest {
        val rig = ready()
        rig.flow.openSettings()
        rig.flow.manifest.open()
        openScan(rig.flow)
        feed(rig.flow, frames(MANIFEST, 1, "forged"))
        assertEquals(Screen.ParseFailed, rig.flow.screen)
        assertEquals("签名无效", rig.flow.scan.failure)
        assertNull(rig.store.manifest)
    }

    // 弱 PIN

    @Test
    fun weakPinIsRefusedOnTheFirstEntry() = runTest {
        val rig = ready()
        rig.core.weakPins = setOf("111111")
        val flow = rig.flow
        flow.openSettings()
        flow.settings.startChangePin()
        advanceUntilIdle()
        "111111".forEach(flow.pin::digit)
        assertTrue(flow.pin.weak)
        assertFalse(flow.pin.confirming)
        assertEquals("", flow.pin.input)

        flow.pin.digit('7')
        assertFalse(flow.pin.weak)
        "39264".forEach(flow.pin::digit)
        assertTrue(flow.pin.confirming)
        assertFalse(flow.pin.weak)
    }

    @Test
    fun secondEntryIsNotCheckedAgainstTheWeakList() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.openSettings()
        flow.settings.startChangePin()
        advanceUntilIdle()
        PIN.forEach(flow.pin::digit)
        rig.core.weakPins = setOf(PIN)
        PIN.forEach(flow.pin::digit)
        assertTrue(flow.pin.done)
    }

    // BIP39 密码钱包的公钥

    @Test
    fun backgroundForgetsSessionKeys() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        openScan(flow)
        feed(flow, frames(REQUEST))
        submit(flow, PIN, "pass")
        assertEquals(Screen.Review, flow.screen)
        flow.scan.exitRequest()

        flow.onBackground()
        openScan(flow)
        feed(flow, frames(REQUEST))
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlock.purpose)
    }

    @Test
    fun backgroundKeepsKeysWhileBusy() = runTest {
        val rig = ready(passphrase = true) { it.core.requestFingerprint = PASSPHRASE_FINGERPRINT }
        val flow = rig.flow
        openScan(flow)
        feed(flow, frames(REQUEST))
        PIN.forEach(flow.unlock::digit)
        "pass".forEach(flow.unlock::passphraseInput)
        flow.unlock.submit()
        assertTrue(flow.busy)
        flow.onBackground()
        advanceUntilIdle()
        assertEquals(Screen.Review, flow.screen)
        flow.scan.exitRequest()

        openScan(flow)
        feed(flow, frames(REQUEST))
        assertEquals(Screen.Review, flow.screen)
    }

    @Test
    fun cachedKeysSurviveBackground() = runTest {
        val rig = ready()
        rig.flow.onBackground()
        openScan(rig.flow)
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
        rig.flow.session.loadAddresses()
        advanceUntilIdle()
        assertTrue(rig.flow.session.addresses.isEmpty())
        openScan(rig.flow)
        feed(rig.flow, frames(REQUEST))
        assertEquals(UnlockPurpose.LoadKeys, rig.flow.unlock.purpose)
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
        rig.flow.receive.open(ethereum)
        advanceUntilIdle()
        assertEquals(listOf(Screen.Receive), rig.flow.stack)
        assertEquals(ethereum, rig.flow.receive.account)
        assertEquals("${ethereum.path}#0:keys:$MAIN_FINGERPRINT", rig.flow.receive.address)
        assertEquals(rig.flow.receive.address, rig.core.lastQrText)
        assertNotNull(rig.flow.receive.qr)
        assertEquals(0, rig.core.opens)
    }

    @Test
    fun navigationKeepsTheLastAccount() = runTest {
        val rig = ready()
        rig.flow.receive.open()
        advanceUntilIdle()
        assertEquals(rig.flow.session.accounts.first(), rig.flow.receive.account)
        rig.flow.receive.select(ethereum)
        advanceUntilIdle()
        rig.flow.openHome()
        rig.flow.receive.open()
        advanceUntilIdle()
        assertEquals(ethereum, rig.flow.receive.account)
        assertTrue(rig.flow.back())
        assertEquals(Screen.Home, rig.flow.screen)
    }

    @Test
    fun nextAddressOnlyGoesUpAndIsPerAccount() = runTest {
        val rig = ready()
        val flow = rig.flow
        flow.receive.open(bitcoin)
        advanceUntilIdle()
        assertTrue(flow.receive.hasNext)
        flow.receive.nextAddress()
        advanceUntilIdle()
        flow.receive.nextAddress()
        advanceUntilIdle()
        assertEquals(2u, rig.store.readReceiveIndex(bitcoin.path))
        assertEquals("${bitcoin.path}#2:keys:$MAIN_FINGERPRINT", flow.receive.address)

        val taproot = Accounts[1]
        flow.receive.select(taproot)
        advanceUntilIdle()
        assertEquals("${taproot.path}#0:keys:$MAIN_FINGERPRINT", flow.receive.address)

        // 重新进来还是上次的序号,不会退回去.
        flow.openHome()
        flow.receive.open(bitcoin)
        advanceUntilIdle()
        assertEquals("${bitcoin.path}#2:keys:$MAIN_FINGERPRINT", flow.receive.address)
    }

    @Test
    fun oldAddressesStayListedAndViewingOneKeepsTheIndex() = runTest {
        val rig = ready()
        val flow = rig.flow
        fun address(index: Int) = "${bitcoin.path}#$index:keys:$MAIN_FINGERPRINT"
        flow.receive.open(bitcoin)
        advanceUntilIdle()
        flow.receive.nextAddress()
        advanceUntilIdle()
        assertEquals(listOf(address(1), address(0)), flow.receive.history)

        flow.receive.openAddresses()
        assertEquals(Screen.ReceiveAddresses, flow.screen)
        assertTrue(flow.back())
        assertEquals(Screen.Receive, flow.screen)

        flow.receive.openAddresses()
        flow.receive.show(address(0))
        advanceUntilIdle()
        assertEquals(Screen.Receive, flow.screen)
        assertEquals(address(0), flow.receive.address)
        assertEquals(1u, rig.store.readReceiveIndex(bitcoin.path))

        // 看过旧地址后,使用新地址仍然是没发放过的那个.
        flow.receive.nextAddress()
        advanceUntilIdle()
        assertEquals(address(2), flow.receive.address)
        assertEquals(listOf(address(2), address(1), address(0)), flow.receive.history)
    }

    @Test
    fun onlyBitcoinHasANextAddress() = runTest {
        val rig = ready()
        rig.flow.receive.open(ethereum)
        advanceUntilIdle()
        assertFalse(rig.flow.receive.hasNext)
        rig.flow.receive.nextAddress()
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
        flow.receive.open(ethereum)
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlock.purpose)
        assertTrue(flow.unlock.needsPassphrase)
        submit(flow, PIN, "pass")
        assertEquals(Screen.Receive, flow.screen)
        assertEquals("${ethereum.path}#0:keys:$PASSPHRASE_FINGERPRINT", flow.receive.address)
        assertNull(rig.store.publicKeys)
    }

    @Test
    fun cancelLoadingKeysLeavesReceive() = runTest {
        val rig = ready(passphrase = true)
        rig.flow.receive.open(ethereum)
        advanceUntilIdle()
        rig.flow.back()
        assertEquals(listOf(Screen.Home), rig.flow.stack)
    }

    @Test
    fun receiveAsksAgainAfterBackgroundForPassphraseWallets() = runTest {
        val rig = ready(passphrase = true)
        val flow = rig.flow
        flow.receive.open(ethereum)
        advanceUntilIdle()
        submit(flow, PIN, "pass")
        flow.onBackground()
        assertNull(flow.receive.address)
        assertNull(flow.receive.qr)
        flow.onResume()
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlock.purpose)
    }

    // 清空状态:每个子流程保存的内容都要清到,这是系统不变量,不是某一页的行为.

    @Test
    fun backgroundClearsSensitiveStateInEverySubflow() = runTest {
        val rig = ready(passphrase = true)
        val flow = rig.flow

        // 公钥和从它算出的地址在内存里.
        flow.receive.open(ethereum)
        advanceUntilIdle()
        submit(flow, PIN, "pass")
        flow.session.loadAddresses()
        advanceUntilIdle()
        assertNotNull(flow.receive.address)
        assertNotNull(flow.receive.qr)
        assertTrue(flow.session.addresses.isNotEmpty())

        // 新 PIN 已输完,当前 PIN 和 BIP39 密码输到一半.
        flow.openSettings()
        flow.settings.startChangePin()
        advanceUntilIdle()
        repeat(2) { "739265".forEach(flow.pin::digit) }
        assertTrue(flow.pin.done)
        flow.settings.continueChangePin()
        assertEquals(Screen.Unlock, flow.screen)
        "123".forEach(flow.unlock::digit)
        "pa".forEach(flow.unlock::passphraseInput)

        flow.onBackground()

        assertEquals("", flow.unlock.pin)
        assertEquals("", flow.unlock.passphrase)
        assertNull(flow.unlock.error)
        assertEquals("", flow.pin.first)
        assertEquals("", flow.pin.input)
        assertFalse(flow.pin.done)
        assertFalse(flow.settings.changingPin)
        assertEquals(listOf(Screen.Settings), flow.stack)
        assertNull(flow.receive.address)
        assertNull(flow.receive.qr)
        assertTrue(flow.session.addresses.isEmpty())

        // 公钥也不在了,再用时要重新解锁.
        flow.receive.open(ethereum)
        advanceUntilIdle()
        assertEquals(Screen.Unlock, flow.screen)
        assertEquals(UnlockPurpose.LoadKeys, flow.unlock.purpose)
        flow.back()

        // 助记词单独一页,离开前台同样不留.
        flow.settings.startViewWords()
        advanceUntilIdle()
        submit(flow, PIN)
        assertEquals(Screen.ViewWords, flow.screen)
        assertTrue(flow.settings.viewWords.isNotEmpty())

        flow.onBackground()

        assertTrue(flow.settings.viewWords.isEmpty())
        assertFalse(Screen.ViewWords in flow.stack)
    }

    @Test
    fun wipeClearsEverySubflow() = runTest {
        val rig = ready()
        val flow = rig.flow

        flow.session.loadAddresses()
        flow.receive.open(ethereum)
        advanceUntilIdle()
        scanManifest(rig)
        flow.manifest.load()
        advanceUntilIdle()
        assertNotNull(flow.scan.result)
        assertTrue(flow.manifest.entries.isNotEmpty())

        flow.export.open()
        advanceUntilIdle()
        submit(flow, PIN)
        assertEquals(Screen.Export, flow.screen)
        val stream = assertNotNull(rig.core.lastStream)

        flow.openSettings()
        flow.settings.startViewWords()
        advanceUntilIdle()
        submit(flow, PIN)
        flow.settings.startChangePin()
        advanceUntilIdle()
        assertTrue(flow.settings.viewWords.isNotEmpty())
        assertTrue(flow.settings.changingPin)
        assertNotNull(flow.session.fingerprint)

        flow.settings.startWipe()
        advanceUntilIdle()
        submit(flow, BAD_PIN)
        assertTrue(flow.unlock.failures > 0)
        rig.store.pin = PinRecord(5, rig.clock.boot, rig.clock.elapsed)
        flow.unlock.refreshStatus()
        assertTrue(flow.unlock.waitMs > 0)
        rig.clock.elapsed += 30_000
        flow.unlock.refreshStatus()
        submit(flow, PIN)

        assertEquals(listOf(Screen.Welcome), flow.stack)
        assertNull(rig.store.vault)
        assertTrue(stream.closed)
        assertNull(flow.player.frame)
        assertNull(flow.scan.result)
        assertNull(flow.session.meta)
        assertNull(flow.session.fingerprint)
        assertTrue(flow.session.addresses.isEmpty())
        assertEquals(Accounts, flow.session.accounts)
        assertNull(flow.receive.account)
        assertNull(flow.receive.address)
        assertNull(flow.receive.qr)
        assertTrue(flow.receive.history.isEmpty())
        assertTrue(flow.manifest.entries.isEmpty())
        assertTrue(flow.settings.viewWords.isEmpty())
        assertFalse(flow.settings.changingPin)
        assertEquals(0, flow.unlock.failures)
        assertEquals(0L, flow.unlock.waitMs)
    }

    // 账户列表

    @Test
    fun accountsComeFromTheCore() {
        val rows =
            listOf(
                AccountAddress("Native SegWit", "m/84'/0'/0'", "a"),
                AccountAddress("单地址", "m/44'/60'/0'/0/0", "b"),
                AccountAddress("Legacy", "m/44'/0'/0'", "c"),
            )
        assertEquals(
            listOf("Bitcoin SegWit", "Ethereum", "BNB Smart Chain", "Polygon", "Bitcoin Legacy"),
            accountsFor(rows).map(AccountRow::name),
        )
    }
}

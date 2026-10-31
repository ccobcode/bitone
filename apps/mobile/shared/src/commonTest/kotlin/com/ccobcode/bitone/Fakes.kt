package com.ccobcode.bitone

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import com.ccobcode.bitone.flow.Accounts
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.AccountAddress
import com.ccobcode.bitone.platform.AddressReview
import com.ccobcode.bitone.platform.BootClock
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.platform.ChangeIndexes
import com.ccobcode.bitone.platform.ClockReading
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.DeviceFacts
import com.ccobcode.bitone.platform.FrameReceiver
import com.ccobcode.bitone.platform.FrameStream
import com.ccobcode.bitone.platform.HardwareKey
import com.ccobcode.bitone.platform.HardwareKeyException
import com.ccobcode.bitone.platform.HardwareLevel
import com.ccobcode.bitone.platform.ManifestEntry
import com.ccobcode.bitone.platform.NATIVE_CONTRACT
import com.ccobcode.bitone.platform.PinRecord
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.QrMatrix
import com.ccobcode.bitone.platform.QrScanner
import com.ccobcode.bitone.platform.QuizItem
import com.ccobcode.bitone.platform.ReviewAmount
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.platform.ScreenGuard
import com.ccobcode.bitone.platform.SecureStore
import com.ccobcode.bitone.platform.SelfCheck
import com.ccobcode.bitone.platform.SignOutcome
import com.ccobcode.bitone.platform.UnlockedWallet
import com.ccobcode.bitone.platform.WalletCore
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletDraft
import com.ccobcode.bitone.platform.WalletMeta
import kotlin.test.assertEquals
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle

val GoodFacts =
    DeviceFacts(
        airplaneMode = true,
        wifiOn = false,
        bluetoothOn = false,
        nfcOn = false,
        rooted = false,
        bootloaderLocked = true,
        adbEnabled = false,
        usbDataConnected = false,
        keystore = HardwareLevel.StrongBox,
        screenLockSet = true,
        thirdPartyAccessibility = 0,
        overlayApps = 0,
    )

fun facts(
    wifiOn: Boolean = false,
    rooted: Boolean = false,
    bootloaderLocked: Boolean? = true,
    adbEnabled: Boolean = false,
    keystore: HardwareLevel = HardwareLevel.StrongBox,
    screenLockSet: Boolean = true,
    thirdPartyAccessibility: Int = 0,
    overlayApps: Int = 0,
) =
    DeviceFacts(
        airplaneMode = true,
        wifiOn = wifiOn,
        bluetoothOn = false,
        nfcOn = false,
        rooted = rooted,
        bootloaderLocked = bootloaderLocked,
        adbEnabled = adbEnabled,
        usbDataConnected = false,
        keystore = keystore,
        screenLockSet = screenLockSet,
        thirdPartyAccessibility = thirdPartyAccessibility,
        overlayApps = overlayApps,
    )

const val MAIN_FINGERPRINT = "73C5DA0A"
const val PASSPHRASE_FINGERPRINT = "9E21B4C7"

fun keysFor(fingerprint: String) = "keys:$fingerprint".encodeToByteArray()

fun reviewOf(
    risk: Risk = Risk.Transfer,
    confirm: ConfirmMode = ConfirmMode.Tap,
    alerts: List<String> = emptyList(),
    mismatch: Boolean = false,
    requestFingerprint: String? = null,
) =
    ReviewResult(
        risk = risk,
        confirm = confirm,
        network = "ethereum",
        title = "普通转账",
        amount = ReviewAmount("1", "ETH", "ethereum:native", "eth", false),
        counterpartyLabel = "收款地址",
        counterparty = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        alerts = alerts,
        actions = emptyList(),
        rows = emptyList(),
        raw = emptyList(),
        fingerprintMismatch = mismatch,
        requestFingerprint = requestFingerprint,
    )

fun tagFor(pin: String) = "tag:$pin".encodeToByteArray()

/** 当前格式的密文:解开要 PIN 和硬件对这个 PIN 的结果. */
fun boundVault(pin: String) = "sealed:$pin:tag:$pin".encodeToByteArray()

/** 旧格式的文件:外面包着硬件加密,里面只靠 PIN. */
fun legacyVault(pin: String) = "hw:sealed:$pin".encodeToByteArray()

val Words12 = List(12) { "word$it" }
val Words24 = List(24) { "word$it" }

class FakeDraft(private val list: List<String>, override val imported: Boolean) : WalletDraft {
    var closed = false

    override fun words() = list

    /** 第 1,5,9 题,正确答案放在第 0 项. */
    override fun quiz(rounds: Int) =
        (0 until rounds).map { i ->
            val index = i * 4
            QuizItem(index, listOf(list[index], "wrongA", "wrongB", "wrongC"))
        }

    override fun checkWord(index: Int, word: String) = list[index] == word

    override fun seal(pin: String, tag: ByteArray) = "sealed:$pin:${tag.decodeToString()}".encodeToByteArray()

    override fun close() {
        closed = true
    }
}

class FakeStream(override val fragmentCount: Int) : FrameStream {
    var closed = false
    var sent = 0

    override fun nextPart() = "part${sent++}"

    override fun close() {
        closed = true
    }
}

fun addressOf(
    belongs: Boolean = true,
    address: String = "bc1qmine",
    derived: String = "bc1qmine",
    fingerprintMismatch: Boolean = false,
) =
    AddressReview(
        belongs,
        "bitcoin",
        "m/84'/0'/0'/0/3",
        address,
        derived,
        "Native SegWit",
        fingerprintMismatch,
    )

class SignCall(
    val message: String,
    val keys: String,
    val highestChange: UInt,
    val manifest: String?,
)

class FakeWallet(private val passphrase: String, private val core: FakeCore) : UnlockedWallet {
    var closed = false

    override fun fingerprint() = if (passphrase.isEmpty()) MAIN_FINGERPRINT else PASSPHRASE_FINGERPRINT

    override fun exportAccounts(imported: Boolean): FrameStream {
        core.exportedImported = imported
        return FakeStream(3).also { core.lastStream = it }
    }

    override fun words(): List<String> {
        core.wordsRead++
        return core.walletWords
    }

    override fun seal(pin: String, tag: ByteArray) = "sealed:$pin:${tag.decodeToString()}".encodeToByteArray()

    override fun publicKeys(imported: Boolean) = keysFor(fingerprint())

    override fun signRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): SignOutcome {
        check(!closed) { "钱包已关闭" }
        if (core.signFails) throw WalletCoreException.Other("策略不允许")
        core.signCalls +=
            SignCall(
                message.decodeToString(),
                keys.decodeToString(),
                highestChange.mainnet,
                manifest?.decodeToString(),
            )
        return SignOutcome(FakeStream(2).also { core.lastStream = it }, core.nextChangeIndex, core.nextChangeIsTestnet)
    }

    override fun close() {
        closed = true
    }
}

/** 帧格式和真实的一样:`ur:<类型>/<序号>-<总数>/<数据>`. */
class FakeReceiver : FrameReceiver {
    private val seen = mutableSetOf<String>()
    private var total = 0
    private var payload = ""
    var closed = false
    override var urType: String? = null

    override fun receive(part: String): Float {
        check(!closed) { "接收器已关闭" }
        if (!part.startsWith("ur:")) throw WalletCoreException.Other("二维码帧无效")
        val fields = part.removePrefix("ur:").split('/')
        val type = fields[0]
        if (urType != null && urType != type) throw WalletCoreException.Other("二维码帧无效")
        urType = type
        val (index, count) = if (fields.size == 3) fields[1].split('-') else listOf("1", "1")
        seen += index
        total = count.toInt()
        payload = fields.last()
        return seen.size.toFloat() / total
    }

    override val complete: Boolean
        get() = total > 0 && seen.size == total

    override val fragmentCount: Int
        get() = total

    override fun message() = payload.encodeToByteArray()

    override fun close() {
        closed = true
    }
}

fun frames(type: String, count: Int = 2, payload: String = "request") =
    (1..count).map { "ur:$type/$it-$count/$payload" }

class FakeScanner : QrScanner {
    var granted = true
    var grantOnRequest = true
    var requests = 0
    var settingsOpened = 0

    override fun hasPermission() = granted

    override suspend fun requestPermission(): Boolean {
        requests++
        granted = grantOnRequest
        return granted
    }

    override fun openSettings() {
        settingsOpened++
    }

    @Composable override fun Viewfinder(modifier: Modifier, onCode: (String) -> Unit, onFailure: () -> Unit) = Unit
}

class FakeCore : WalletCore {
    var lastDraft: FakeDraft? = null
    var lastWallet: FakeWallet? = null
    var lastStream: FakeStream? = null
    var exportedImported: Boolean? = null
    var lastReceiver: FakeReceiver? = null
    var opens = 0
    var walletWords = Words12
    var wordsRead = 0

    /** 模拟 Argon2 途中被杀进程. */
    var killDuringOpen = false

    /** 下一次核对的结果. */
    var review = reviewOf()

    /** 请求里的指纹,用来模拟核心库的核对. */
    var requestFingerprint = MAIN_FINGERPRINT
    var reviewedHighestChange = mutableListOf<UInt>()
    var reviewedManifests = mutableListOf<String?>()
    val signCalls = mutableListOf<SignCall>()
    var signFails = false
    var nextChangeIndex: UInt? = null
    var nextChangeIsTestnet = false

    var address = addressOf()
    var addressError: String? = null

    /** 被核心库判定太简单的 PIN,默认没有. */
    var weakPins = emptySet<String>()

    /** 核心库解不开的公钥. */
    var unreadableKeys: ByteArray? = null
    var lastQrText: String? = null

    /** 内置清单的版本. */
    var builtinManifest = 1u
    val manifestVersions = mutableMapOf<String, UInt>()

    override fun generateDraft(wordCount: Int): WalletDraft =
        FakeDraft(if (wordCount == 24) Words24 else Words12, imported = false).also { lastDraft = it }

    override fun importDraft(words: List<String>): WalletDraft {
        if (words.any { it == "badword" }) throw WalletCoreException.InvalidMnemonic()
        return FakeDraft(words, imported = true).also { lastDraft = it }
    }

    override fun open(sealed: ByteArray, pin: String, tag: ByteArray, passphrase: String): UnlockedWallet {
        opens++
        if (killDuringOpen) error("进程被杀")
        if (sealed.decodeToString() != "sealed:$pin:${tag.decodeToString()}") throw WalletCoreException.WrongPin()
        return FakeWallet(passphrase, this).also { lastWallet = it }
    }

    override fun openLegacy(sealed: ByteArray, pin: String, passphrase: String): UnlockedWallet {
        opens++
        if (sealed.decodeToString() != "sealed:$pin") throw WalletCoreException.WrongPin()
        return FakeWallet(passphrase, this).also { lastWallet = it }
    }

    override fun isBoundVault(vault: ByteArray) = vault.decodeToString().startsWith("sealed:")

    override fun qrMatrix(part: String) = QrMatrix(2, BooleanArray(4))

    override fun qrText(text: String): QrMatrix {
        lastQrText = text
        return QrMatrix(2, BooleanArray(4))
    }

    override fun isWeakPin(pin: String) = pin in weakPins

    override fun isValidWord(word: String) = word != "notaword"

    override fun wordSuggestions(prefix: String, limit: Int) =
        if (prefix.startsWith("zz")) emptyList() else listOf("${prefix}a", "${prefix}b").take(limit)

    override fun newReceiver(): FrameReceiver = FakeReceiver().also { lastReceiver = it }

    override fun reviewRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): ReviewResult {
        reviewedHighestChange += highestChange.mainnet
        reviewedManifests += manifest?.decodeToString()
        if (!keys.contentEquals(keysFor(requestFingerprint))) {
            return reviewOf(
                Risk.Forbidden,
                ConfirmMode.Refuse,
                listOf("这个请求不属于本钱包"),
                mismatch = true,
                requestFingerprint = requestFingerprint,
            )
        }
        return review
    }

    override fun reviewAddress(message: ByteArray, keys: ByteArray): AddressReview {
        addressError?.let { throw WalletCoreException.Other(it) }
        return address
    }

    override fun accountRows(keys: ByteArray): List<AccountAddress> {
        if (unreadableKeys?.contentEquals(keys) == true) throw WalletCoreException.Other("公钥格式过旧")
        return Accounts.distinctBy { it.path }
            .map {
                AccountAddress("kind", it.path, "${it.path}:${keys.decodeToString()}")
            }
    }

    override fun receiveAddress(keys: ByteArray, accountPath: String, index: UInt) =
        "$accountPath#$index:${keys.decodeToString()}"

    override fun verifyManifest(bytes: ByteArray, currentVersion: UInt): UInt {
        val version = manifestVersions[bytes.decodeToString()] ?: throw WalletCoreException.Other("签名无效")
        if (version <= currentVersion) throw WalletCoreException.Other("清单版本过旧")
        return version
    }

    override fun manifestVersion(bytes: ByteArray?): UInt =
        if (bytes == null) builtinManifest else verifyManifest(bytes, 0u)

    override fun manifestEntries(bytes: ByteArray?) =
        listOf(
            ManifestEntry("ethereum", "Ethereum", "ETH", NATIVE_CONTRACT, "eth"),
            ManifestEntry(
                "ethereum",
                "Ethereum",
                "USDT",
                "0xdAC17F958D2ee523a2206206994597C13D831ec7",
                "usdt",
            ),
        ) + listOfNotNull(bytes?.let { ManifestEntry("tron", "TRON", it.decodeToString(), "T", "usdt") })
}

class FakeHardwareKey : HardwareKey {
    var lost = false
    var cancelAuth = false

    /** 只取消这个 PIN 的确认,比如修改 PIN 时的新 PIN. */
    var cancelFor: String? = null
    var unavailable = false
    var deleted = 0
    var legacyDropped = 0
    var binds = 0
    var confirmations = 0
    var cancelConfirm = false
    var changed = false

    override suspend fun bind(pin: String): ByteArray {
        if (lost) throw HardwareKeyException.Lost("失效")
        if (cancelAuth || cancelFor == pin) throw HardwareKeyException.AuthCancelled()
        binds++
        return tagFor(pin)
    }

    override suspend fun enroll(pin: String): ByteArray {
        if (unavailable) throw HardwareKeyException.Unavailable("无")
        if (cancelAuth || cancelFor == pin) throw HardwareKeyException.AuthCancelled()
        lost = false
        binds++
        return tagFor(pin)
    }

    override suspend fun unwrap(wrapped: ByteArray): ByteArray {
        if (lost) throw HardwareKeyException.Lost("失效")
        if (cancelAuth) throw HardwareKeyException.AuthCancelled()
        return wrapped.copyOfRange(3, wrapped.size)
    }

    override fun isLost() = lost

    override fun delete() {
        deleted++
    }

    override fun dropLegacy() {
        legacyDropped++
    }

    override suspend fun confirmDeviceLock() {
        if (cancelConfirm) throw HardwareKeyException.AuthCancelled()
        confirmations++
    }

    override fun lockChanged() = changed

    override fun acceptLock() {
        changed = false
    }
}

class FakeStore : SecureStore {
    var vault: ByteArray? = null
    var meta: WalletMeta? = null
    var pin = PinRecord.Clean
    var pinWrites = 0
    var publicKeys: ByteArray? = null
    var manifest: ByteArray? = null
    var highestChange = 0u
    var highestChangeTestnet = 0u
    var highestChangeWrites = 0
    val receiveIndexes = mutableMapOf<String, UInt>()

    override fun readVault() = vault

    override fun readMeta() = meta

    override fun saveWallet(vault: ByteArray, meta: WalletMeta, publicKeys: ByteArray?) {
        this.vault = vault
        this.meta = meta
        this.publicKeys = publicKeys
        pin = PinRecord.Clean
        highestChange = 0u
        highestChangeTestnet = 0u
        receiveIndexes.clear()
    }

    override fun replaceVault(vault: ByteArray) {
        this.vault = vault
    }

    override fun readPublicKeys() = publicKeys

    override fun writePublicKeys(keys: ByteArray) {
        publicKeys = keys
    }

    override fun readHighestChange() = ChangeIndexes(highestChange, highestChangeTestnet)

    override fun writeHighestChange(testnet: Boolean, index: UInt) {
        highestChangeWrites++
        if (testnet) highestChangeTestnet = index else highestChange = index
    }

    override fun readReceiveIndex(accountPath: String) = receiveIndexes[accountPath] ?: 0u

    override fun writeReceiveIndex(accountPath: String, index: UInt) {
        receiveIndexes[accountPath] = index
    }

    override fun readManifest() = manifest

    override fun writeManifest(bytes: ByteArray) {
        manifest = bytes
    }

    var language: String? = null
    var autoBrightness = true

    override fun readLanguage() = language

    override fun writeLanguage(code: String?) {
        language = code
    }

    override fun readAutoBrightness() = autoBrightness

    override fun writeAutoBrightness(on: Boolean) {
        autoBrightness = on
    }

    override fun readPinRecord() = pin

    override fun writePinRecord(record: PinRecord) {
        pin = record
        pinWrites++
    }

    override fun wipe() {
        vault = null
        meta = null
        pin = PinRecord.Clean
        publicKeys = null
        manifest = null
        highestChange = 0u
        highestChangeTestnet = 0u
        receiveIndexes.clear()
    }
}

class FakeClock : BootClock {
    var boot = 1L
    var elapsed = 1_000L

    override fun now() = ClockReading(boot, elapsed)
}

class FakeSelfCheck(var facts: DeviceFacts = GoodFacts) : SelfCheck {
    var runs = 0

    override suspend fun collect(): DeviceFacts {
        runs++
        return facts
    }
}

class FakeBrightness : Brightness {
    override fun hold(maximize: Boolean) {}

    override fun restore() {}
}

class Rig(
    scope: CoroutineScope,
    work: CoroutineDispatcher,
    val core: FakeCore = FakeCore(),
    val key: FakeHardwareKey = FakeHardwareKey(),
    val store: FakeStore = FakeStore(),
    val clock: FakeClock = FakeClock(),
    val check: FakeSelfCheck = FakeSelfCheck(),
    val scanner: FakeScanner = FakeScanner(),
    systemLanguage: String = "zh",
) {
    val flow =
        WalletFlow(
            Platform(
                core = core,
                hardwareKey = key,
                selfCheck = check,
                screenGuard =
                    object : ScreenGuard {
                        override fun protect() {}
                    },
                brightness = FakeBrightness(),
                store = store,
                clock = clock,
                scanner = scanner,
                systemLanguage = systemLanguage,
                appVersion = "1.0.0",
                certificate = "9F3A 61C2",
            ),
            scope,
            work,
        )
}

/** backgroundScope 的任务不算在 advanceUntilIdle 里,所以单独建作用域. */
@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.newRig(): Rig {
    val dispatcher = StandardTestDispatcher(testScheduler)
    val scope = CoroutineScope(dispatcher + SupervisorJob() + CoroutineExceptionHandler { _, _ -> })
    return Rig(scope, dispatcher)
}

/** 已有钱包的状态,不走一遍创建流程. */
@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.readyRig(
    pin: String,
    passphrase: Boolean = false,
    cached: Boolean = true,
    setup: (Rig) -> Unit = {},
): Rig {
    val rig = newRig()
    rig.store.meta = WalletMeta(false, passphrase, if (passphrase) null else MAIN_FINGERPRINT)
    rig.store.vault = boundVault(pin)
    if (!passphrase && cached) rig.store.publicKeys = keysFor(MAIN_FINGERPRINT)
    setup(rig)
    rig.flow.onResume()
    advanceUntilIdle()
    assertEquals(Screen.Home, rig.flow.screen)
    return rig
}

@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.openScan(flow: WalletFlow) {
    flow.scan.open()
    advanceUntilIdle()
}

@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.feed(flow: WalletFlow, parts: List<String>) = parts.forEach {
    flow.scan.onScanned(it)
    advanceUntilIdle()
}

@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.submit(flow: WalletFlow, pin: String, passphrase: String = "") {
    pin.forEach(flow.unlock::digit)
    passphrase.forEach(flow.unlock::passphraseInput)
    flow.unlock.submit()
    advanceUntilIdle()
}

@OptIn(ExperimentalCoroutinesApi::class)
fun TestScope.passSelfCheck(rig: Rig) {
    rig.check.facts = GoodFacts
    rig.flow.retryCheck()
    advanceUntilIdle()
}

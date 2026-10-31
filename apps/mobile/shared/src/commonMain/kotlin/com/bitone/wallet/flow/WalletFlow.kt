package com.bitone.wallet.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import com.bitone.wallet.i18n.I18n
import com.bitone.wallet.i18n.Lang
import com.bitone.wallet.platform.AddressReview
import com.bitone.wallet.platform.ConfirmMode
import com.bitone.wallet.platform.FrameReceiver
import com.bitone.wallet.platform.FrameStream
import com.bitone.wallet.platform.HardwareKeyException
import com.bitone.wallet.platform.ManifestEntry
import com.bitone.wallet.platform.Platform
import com.bitone.wallet.platform.QrMatrix
import com.bitone.wallet.platform.QrScanner
import com.bitone.wallet.platform.QuizItem
import com.bitone.wallet.platform.ReviewResult
import com.bitone.wallet.platform.Risk
import com.bitone.wallet.platform.UnlockedWallet
import com.bitone.wallet.platform.WalletCoreException
import com.bitone.wallet.platform.WalletDraft
import com.bitone.wallet.platform.WalletMeta
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Qr
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val QUIZ_ROUNDS = 3
private const val SUGGESTION_LIMIT = 2

/** 长按 3 秒，每 50 毫秒更新一次进度。 */
private const val HOLD_TICKS = 60
private const val HOLD_TICK_MS = 50L

private const val UR_REQUEST = "bitone-request"
private const val UR_ADDRESS = "bitone-address"
private const val UR_MANIFEST = "bitone-manifest"
private val SCANNABLE = setOf(UR_REQUEST, UR_ADDRESS, UR_MANIFEST)

enum class VerifyState {
    Pending,
    Passed,
    Failed,
}

enum class ChecksumState {
    Incomplete,
    Valid,
    Invalid,
}

/** A7 解开钱包之后做什么。钱包只活在这一次操作里，操作结束立刻关闭。 */
enum class UnlockPurpose {
    Export,
    Sign,
    LoadKeys,
    ViewWords,
    ChangePin,
    Wipe,
    ApplyManifest,
}

/** 助记词和 PIN 层都不依赖 BIP39 密码，这些用途只验证 PIN，不用再输密码。 */
private val PIN_ONLY =
    setOf(UnlockPurpose.ViewWords, UnlockPurpose.ChangePin, UnlockPurpose.Wipe, UnlockPurpose.ApplyManifest)

/** A6 到 A6e。只看核心库给的类别和告警，不做额外判断。 */
enum class ReviewStyle {
    Plain,
    Unknown,
    Approval,
    Permission,
    HighRisk,
}

fun reviewStyle(review: ReviewResult): ReviewStyle =
    when {
        review.risk == Risk.PermissionChange -> ReviewStyle.Permission
        review.risk == Risk.Approval -> ReviewStyle.Approval
        review.risk == Risk.Unknown -> ReviewStyle.Unknown
        review.alerts.isNotEmpty() -> ReviewStyle.HighRisk
        else -> ReviewStyle.Plain
    }

/** A5 收齐后的结果。A6、A9 和 A12 显示的都是这一份。 */
sealed interface ScanResult {
    class Request(val review: ReviewResult) : ScanResult

    class Address(val review: AddressReview) : ScanResult

    class Manifest(val version: UInt) : ScanResult
}

private class Scanned(val type: String, val message: ByteArray)

/**
 * 全部页面的流程状态，导航是一个状态栈。
 *
 * 助记词、PIN、BIP39 密码都只放在这里的内存状态里，不进 Bundle，所以 Activity 被回收时不会写到 saved instance state。输入在用完或离开页面时立刻清空。解开的钱包不进这里的任何字段。
 */
class WalletFlow(
    private val platform: Platform,
    private val scope: CoroutineScope,
    private val work: CoroutineDispatcher,
) {
    private val core = platform.core
    private val store = platform.store
    private val hardwareKey = platform.hardwareKey
    private val guard = PinGuard(store, platform.clock, hardwareKey::lockChanged)
    private val systemLanguage = platform.systemLanguage
    val scanner: QrScanner = platform.scanner

    // 导航

    var stack by mutableStateOf(listOf<Screen>(Screen.Booting))
        private set

    val screen: Screen
        get() = stack.last()

    var busy by mutableStateOf(false)
        private set

    var notice by mutableStateOf<String?>(null)
        private set

    /** 这条提示是失败，显示告警图标。 */
    var noticeFailed by mutableStateOf(false)
        private set

    private var noticeSerial = 0

    // 设置

    val appVersion: String
        get() = platform.appVersion

    val certificate: String
        get() = platform.certificate

    var manifestVersion by mutableStateOf<UInt?>(null)
        private set

    var manifestEntries by mutableStateOf(emptyList<ManifestEntry>())
        private set

    // 自检

    var report by mutableStateOf<SelfCheckReport?>(null)
        private set

    // 已保存的钱包

    var meta by mutableStateOf(store.readMeta())
        private set

    /** 本次解锁算出的指纹，只留在内存。启用 BIP39 密码时只有它是真指纹。 */
    var sessionFingerprint by mutableStateOf<String?>(null)
        private set

    val fingerprint: String?
        get() = sessionFingerprint ?: meta?.fingerprint

    /** BIP39 密码钱包的公钥只放这里，不落盘。 */
    private var sessionKeys: ByteArray? = null

    /** 派生路径到地址。公钥缓存里没有的账户不显示地址。 */
    var addresses by mutableStateOf(emptyMap<String, String>())
        private set

    /** A3 和 A11 列出的账户，公钥读到之后换成核心库给的。 */
    var accounts by mutableStateOf(Accounts)
        private set

    // A11

    var receiveAccount by mutableStateOf<AccountRow?>(null)
        private set

    var receiveAddress by mutableStateOf<String?>(null)
        private set

    var receiveQr by mutableStateOf<QrMatrix?>(null)
        private set

    /** 这个账户发放过的地址，最新的在前。使用新地址后旧地址仍然能收款。 */
    var receiveHistory by mutableStateOf<List<String>>(emptyList())
        private set

    /** Bitcoin 每次收款可以换一个地址，其他链只有一个。 */
    val receiveHasNext: Boolean
        get() = receiveAccount?.network == Network.Bitcoin

    // 创建流程

    private var draft: WalletDraft? = null
    private var verified = false

    var wordCount by mutableStateOf(12)
        private set

    var backupWords by mutableStateOf(emptyList<String>())
        private set

    var passphraseEnabled by mutableStateOf(false)
        private set

    var quiz by mutableStateOf(emptyList<QuizItem>())
        private set

    var picks by mutableStateOf(emptyMap<Int, String>())
        private set

    var verifyState by mutableStateOf(VerifyState.Pending)
        private set

    // A1b

    var importedWords by mutableStateOf(emptyList<String>())
        private set

    var importBuffer by mutableStateOf("")
        private set

    var suggestions by mutableStateOf(emptyList<String>())
        private set

    var checksum by mutableStateOf(ChecksumState.Incomplete)
        private set

    val bufferInvalid: Boolean
        get() = importBuffer.isNotEmpty() && suggestions.isEmpty()

    val importComplete: Boolean
        get() = importedWords.size >= wordCount

    /** 正在改的那个词。点已填的词回去改，不用删掉后面的词。 */
    var importEditing by mutableStateOf<Int?>(null)
        private set

    /** 正在输入的序号。 */
    val importCurrent: Int
        get() = importEditing ?: importedWords.size

    // A2c

    var pinFirst by mutableStateOf("")
        private set

    var pinInput by mutableStateOf("")
        private set

    var pinMismatch by mutableStateOf(false)
        private set

    /** 第一遍输完被核心库判定太简单。 */
    var pinWeak by mutableStateOf(false)
        private set

    var setupError by mutableStateOf<String?>(null)
        private set

    val pinConfirming: Boolean
        get() = pinFirst.length == PIN_LENGTH

    val pinDone: Boolean
        get() = pinConfirming && pinInput == pinFirst

    /** 导入的钱包走 A1b 到 A2c，步骤标记只用于新建。 */
    val isImportFlow: Boolean
        get() = draft?.imported == true

    /** A10 修改 PIN 复用 A2c。先输新 PIN，最后在 A7 输当前 PIN，钱包只在最后一步解开一次。 */
    var changingPin by mutableStateOf(false)
        private set

    // A7

    var unlockPin by mutableStateOf("")
        private set

    var unlockPassphrase by mutableStateOf("")
        private set

    var pinFailures by mutableStateOf(guard.failures)
        private set

    var waitMs by mutableStateOf(0L)
        private set

    /** 指纹变更后不提供抹除，输对 PIN 后恢复。 */
    var canWipe by mutableStateOf(true)
        private set

    // A10

    var autoBrightness by mutableStateOf(store.readAutoBrightness())
        private set

    fun toggleAutoBrightness() {
        autoBrightness = !autoBrightness
        store.writeAutoBrightness(autoBrightness)
    }

    fun toggleLanguage() {
        I18n.lang = if (I18n.lang == Lang.Zh) Lang.En else Lang.Zh
        store.writeLanguage(if (I18n.lang == Lang.Zh) "zh" else "en")
    }

    init {
        I18n.lang = Lang.of(store.readLanguage() ?: systemLanguage)
    }

    var unlockError by mutableStateOf<String?>(null)
        private set

    var unlockPurpose by mutableStateOf(UnlockPurpose.Export)
        private set

    val passphraseRequired: Boolean
        get() = meta?.passphrase == true

    val unlockNeedsPassphrase: Boolean
        get() = passphraseRequired && unlockPurpose !in PIN_ONLY

    val canSubmitUnlock: Boolean
        get() =
            !busy &&
                waitMs == 0L &&
                unlockPin.length == PIN_LENGTH &&
                (!unlockNeedsPassphrase || unlockPassphrase.isNotEmpty())

    // A4

    private var frames: FrameStream? = null
    private var playing = false
    var frame by mutableStateOf<QrMatrix?>(null)
        private set

    var frameIndex by mutableStateOf(1)
        private set

    var frameTotal by mutableStateOf(1)
        private set

    // A5

    private var receiver: FrameReceiver? = null
    private var lastPart: String? = null
    private var scanned: Scanned? = null
    var scanFrames by mutableStateOf(0)
        private set

    var scanProgress by mutableStateOf(0f)
        private set

    var scanResult by mutableStateOf<ScanResult?>(null)
        private set

    val review: ReviewResult?
        get() = (scanResult as? ScanResult.Request)?.review

    val addressReview: AddressReview?
        get() = (scanResult as? ScanResult.Address)?.review

    var holdProgress by mutableStateOf(0f)
        private set

    private var holdJob: Job? = null

    // AS

    var scanFailure by mutableStateOf("")
        private set

    /** 请求里声明的指纹。地址核对的结果里没有这一项。 */
    var mismatchFingerprint by mutableStateOf<String?>(null)
        private set

    var refusal by mutableStateOf("")
        private set

    var cameraBroken by mutableStateOf(false)
        private set

    // A10

    var viewWords by mutableStateOf(emptyList<String>())
        private set

    // 启动和自检

    /** 启动和每次回到前台都调用，见 docs/04 §2.2。 */
    fun onResume() {
        scope.launch {
            recheck()
            // 从系统设置授权回来，直接进扫码。
            if (screen == Screen.CameraDenied && !cameraBroken && scanner.hasPermission()) rescan()
            // BIP39 密码钱包的公钥在后台时清掉了，收款页要重新读。
            if (screen == Screen.Receive && receiveAddress == null) guarded { loadReceive() }
        }
    }

    /** 重新跑一次自检，并按结果决定去哪一页。返回 true 表示没有未通过项。 */
    suspend fun recheck(): Boolean {
        class Probe(
            val result: SelfCheckReport,
            val saved: WalletMeta?,
            val lost: Boolean,
            val mayWipe: Boolean,
            val exhausted: Boolean,
        ) {
            val wallet: Boolean
                get() = saved != null
        }
        val probe =
            withContext(work) {
                val saved = store.readMeta()?.takeIf { store.readVault() != null }
                val wallet = saved != null
                val mayWipe = guard.mayWipe()
                Probe(
                    result = evaluate(platform.selfCheck.collect()),
                    saved = saved,
                    lost = wallet && hardwareKey.isLost(),
                    mayWipe = mayWipe,
                    // 上次尝试时被杀进程，计数已经到顶，这里补上抹除。
                    exhausted = wallet && mayWipe && guard.failures >= MAX_PIN_FAILURES,
                )
            }
        report = probe.result
        meta = probe.saved
        canWipe = probe.mayWipe
        val root = if (probe.wallet) Screen.Home else Screen.Welcome
        when {
            probe.exhausted -> {
                withContext(work) { eraseData() }
                clearSession()
                stack = listOf(Screen.Wiped)
            }
            probe.lost && Screen.NeedsRecovery !in stack -> {
                dropRequest()
                stack = listOf(Screen.NeedsRecovery)
            }
            probe.result.blocked -> {
                // 助记词页也要锁定，内容不留到自检恢复之后。
                if (Screen.ViewWords in stack) leaveViewWords()
                if (screen != Screen.SelfCheckFailed) push(Screen.SelfCheckFailed)
            }
            screen == Screen.SelfCheckFailed -> {
                val below = stack.dropLast(1)
                stack = if (below.isEmpty() || below.last() == Screen.Booting) listOf(root) else below
            }
            screen == Screen.Booting -> stack = listOf(root)
            screen == Screen.NeedsRecovery && !probe.lost -> stack = listOf(root)
        }
        return !probe.result.blocked
    }

    /** AS 页的「重新检测」。 */
    fun retryCheck() = guarded { recheck() }

    private fun guarded(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                block()
            } finally {
                busy = false
            }
        }
    }

    private fun push(next: Screen) {
        stack = stack + next
    }

    private fun showOnly(next: Screen) {
        stack = stack.dropLast(1) + next
    }

    // 返回键

    /** 页面上打开的弹层和全屏二维码。返回键先关最后打开的那个，都关了才离开页面。 */
    private val layers = mutableListOf<() -> Unit>()

    fun openLayer(close: () -> Unit) {
        layers += close
    }

    fun closeLayer(close: () -> Unit) {
        layers -= close
    }

    /** 返回 false 表示已到栈底，由平台退出应用。 */
    fun back(): Boolean {
        layers.lastOrNull()?.let {
            it()
            return true
        }
        if (busy) return true
        when (screen) {
            Screen.Backup -> leaveCreate()
            Screen.Verify -> backToBackup()
            Screen.SetPin -> {
                resetPinEntry()
                changingPin = false
                stack = stack.dropLast(1)
            }
            Screen.ImportWords -> leaveImport()
            Screen.Unlock -> leaveUnlock()
            Screen.Export -> finishExport()
            Screen.Settings,
            Screen.Receive -> openHome()
            Screen.Manifest -> leaveManifest()
            Screen.ReceiveAddresses -> stack = stack.dropLast(1)
            Screen.Scan -> closeScan()
            Screen.Signed -> finishSigned()
            Screen.ViewWords -> leaveViewWords()
            Screen.Review,
            Screen.CheckAddress,
            Screen.FingerprintMismatch,
            Screen.ParseFailed,
            Screen.Forbidden,
            Screen.CameraDenied -> exitRequest()
            else -> return false
        }
        return true
    }

    // A1

    /** 新建钱包。自检不通过时停在 AS。 */
    fun startCreate() = guarded {
        if (!recheck()) return@guarded
        replaceDraft(withContext(work) { core.generateDraft(12) }, 12)
        passphraseEnabled = false
        loadBackupWords()
        push(Screen.Backup)
    }

    fun startImport() = guarded {
        if (!recheck()) return@guarded
        resetImport()
        passphraseEnabled = false
        push(Screen.ImportWords)
    }

    private fun replaceDraft(next: WalletDraft?, count: Int) {
        draft?.close()
        draft = next
        wordCount = count
        verified = false
    }

    // A2

    private suspend fun loadBackupWords() {
        val current = draft ?: return
        backupWords = withContext(work) { current.words() }
    }

    /** 换长度就是换一组新助记词，已抄写的作废。 */
    fun setBackupWordCount(count: Int) = guarded {
        if (count == wordCount) return@guarded
        replaceDraft(withContext(work) { core.generateDraft(count) }, count)
        loadBackupWords()
    }

    fun togglePassphrase() {
        passphraseEnabled = !passphraseEnabled
    }

    private fun leaveCreate() {
        backupWords = emptyList()
        replaceDraft(null, 12)
        stack = stack.dropLast(1)
    }

    // A2b

    fun startVerify() = guarded {
        val current = draft ?: return@guarded
        quiz = withContext(work) { current.quiz(QUIZ_ROUNDS) }
        picks = emptyMap()
        verifyState = VerifyState.Pending
        verified = false
        // 进入验证后助记词不再留在界面状态里。
        backupWords = emptyList()
        push(Screen.Verify)
    }

    /** 全部答完才判定。判错后选项锁定，只能回 A2，防止逐个试出答案。 */
    fun pick(index: Int, word: String) {
        if (verifyState == VerifyState.Failed || busy) return
        picks = picks + (index to word)
        if (picks.size < quiz.size) {
            verifyState = VerifyState.Pending
            return
        }
        val answers = picks.toMap()
        val current = draft ?: return
        guarded {
            val correct = withContext(work) { answers.all { (i, w) -> current.checkWord(i, w) } }
            verifyState = if (correct) VerifyState.Passed else VerifyState.Failed
        }
    }

    fun backToBackup() = guarded {
        quiz = emptyList()
        picks = emptyMap()
        verifyState = VerifyState.Pending
        verified = false
        loadBackupWords()
        stack = stack.dropLast(1)
    }

    fun continueAfterVerify() {
        if (verifyState != VerifyState.Passed) return
        verified = true
        resetPinEntry()
        push(Screen.SetPin)
    }

    // A1b

    fun setImportWordCount(count: Int) {
        if (busy || count == wordCount) return
        wordCount = count
        importedWords = importedWords.take(count)
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        refreshChecksum()
    }

    fun importLetter(letter: Char) {
        if (busy || (importComplete && importEditing == null)) return
        importBuffer += letter
        suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
    }

    fun importDelete() {
        if (busy) return
        when {
            importBuffer.isNotEmpty() -> {
                importBuffer = importBuffer.dropLast(1)
                suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
            }
            // 改词时删空了再按一次是放弃修改，原来的词还在。
            importEditing != null -> {
                importEditing = null
                refreshChecksum()
            }
            importedWords.isNotEmpty() -> {
                importedWords = importedWords.dropLast(1)
                refreshChecksum()
            }
        }
    }

    /** 回去改已经填好的词。这个词放回输入区，其余的词不动。再点一次放弃修改。 */
    fun importEdit(index: Int) {
        if (busy || index !in importedWords.indices) return
        importBuffer = ""
        suggestions = emptyList()
        if (importEditing == index) {
            importEditing = null
            refreshChecksum()
            return
        }
        importEditing = index
        importBuffer = importedWords[index]
        suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
        replaceDraft(null, wordCount)
        checksum = ChecksumState.Incomplete
    }

    fun importPick(word: String) {
        val editing = importEditing
        if (busy || (importComplete && editing == null) || !core.isValidWord(word)) return
        importedWords =
            if (editing != null) importedWords.toMutableList().also { it[editing] = word } else importedWords + word
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        refreshChecksum()
    }

    private fun refreshChecksum() {
        replaceDraft(null, wordCount)
        if (!importComplete) {
            checksum = ChecksumState.Incomplete
            return
        }
        val words = importedWords
        guarded {
            try {
                val next = withContext(work) { core.importDraft(words) }
                replaceDraft(next, wordCount)
                checksum = ChecksumState.Valid
            } catch (_: WalletCoreException) {
                checksum = ChecksumState.Invalid
            }
        }
    }

    /** 校验和错误时阻止，见 docs/02。 */
    fun continueAfterImport() {
        if (checksum != ChecksumState.Valid || draft?.imported != true) return
        resetPinEntry()
        push(Screen.SetPin)
    }

    private fun resetImport() {
        replaceDraft(null, 12)
        importedWords = emptyList()
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        checksum = ChecksumState.Incomplete
    }

    private fun leaveImport() {
        resetImport()
        stack = stack.dropLast(1)
    }

    // A2c

    private fun resetPinEntry() {
        pinFirst = ""
        pinInput = ""
        pinMismatch = false
        pinWeak = false
        setupError = null
    }

    /** 新建的钱包必须验证过才能设 PIN，导入的钱包跳过验证。 */
    private fun mayFinishSetup(): Boolean {
        val current = draft ?: return false
        return current.imported || verified
    }

    fun setPinDigit(digit: Char) {
        if (pinDone || pinInput.length >= PIN_LENGTH) return
        val next = pinInput + digit
        when {
            next.length < PIN_LENGTH -> {
                pinInput = next
                pinMismatch = false
                pinWeak = false
            }
            !pinConfirming && core.isWeakPin(next) -> {
                pinInput = ""
                pinMismatch = false
                pinWeak = true
            }
            !pinConfirming -> {
                pinFirst = next
                pinInput = ""
                pinMismatch = false
                pinWeak = false
            }
            next == pinFirst -> pinInput = next
            else -> {
                pinFirst = ""
                pinInput = ""
                pinMismatch = true
            }
        }
    }

    fun setPinDelete() {
        if (!pinDone) pinInput = pinInput.dropLast(1)
    }

    /** 建钱包的最后一步：硬件算出这个 PIN 的结果，Rust 用两者一起加密，落盘后才进首页。 */
    fun finishSetup() {
        if (!pinDone || !mayFinishSetup()) return
        val current = draft ?: return
        val pin = pinInput
        val passphrase = passphraseEnabled
        guarded {
            setupError = null
            try {
                val saved =
                    withContext(work) {
                        // 恢复时旧密钥可能已失效，新钱包用新密钥。
                        hardwareKey.delete()
                        val tag = hardwareKey.enroll(pin)
                        val vault = current.seal(pin, tag)
                        // 启用 BIP39 密码时不算指纹和公钥：这里算出的不是用户真正的钱包，也不把密码派生的结果写盘。
                        var fingerprint: String? = null
                        var keys: ByteArray? = null
                        if (!passphrase) {
                            val wallet = core.open(vault, pin, tag, "")
                            try {
                                fingerprint = wallet.fingerprint()
                                keys = wallet.publicKeys(current.imported)
                            } finally {
                                wallet.close()
                            }
                        }
                        val next = WalletMeta(current.imported, passphrase, fingerprint)
                        store.saveWallet(vault, next, keys)
                        next
                    }
                meta = saved
                sessionFingerprint = null
                sessionKeys = null
                pinFailures = 0
                resetPinEntry()
                clearReceive()
                backupWords = emptyList()
                importedWords = emptyList()
                quiz = emptyList()
                picks = emptyMap()
                replaceDraft(null, 12)
                stack = listOf(Screen.Home)
            } catch (_: HardwareKeyException.AuthCancelled) {
                setupError = "需要确认系统屏幕锁"
            } catch (_: HardwareKeyException) {
                recheck()
                setupError = "硬件密钥不可用"
            } catch (_: WalletCoreException) {
                setupError = "保存失败，请重试"
            }
        }
    }

    // A3

    /** 没有落盘的钱包就没有首页，验证没通过时走不到这里。 */
    fun openHome() {
        if (meta != null) stack = listOf(Screen.Home)
    }

    fun openSettings() {
        if (meta != null) stack = listOf(Screen.Settings)
    }

    fun showNotice(text: String, failed: Boolean = false) {
        notice = text
        noticeFailed = failed
        val serial = ++noticeSerial
        scope.launch {
            delay(Motion.toast.toLong())
            if (serial == noticeSerial) notice = null
        }
    }

    /** 解锁相当于签名前的一步，先重新自检。 */
    fun openExport() = guarded {
        if (!recheck()) return@guarded
        unlockFor(UnlockPurpose.Export)
    }

    /** 由 A3 驱动，解锁回来时会重算。 */
    suspend fun loadAddresses() {
        val rows =
            withContext(work) {
                try {
                    reviewKeys()?.let(core::accountRows).orEmpty()
                } catch (_: WalletCoreException) {
                    emptyList()
                }
            }
        addresses = rows.associate { it.path to it.address }
        if (rows.isNotEmpty()) accounts = accountsFor(rows)
    }

    private fun reviewKeys(): ByteArray? = if (meta?.passphrase == true) sessionKeys else cachedKeys()

    /** 旧版本写下的公钥解不开，当作没有缓存。下次解锁时重新生成，不让它一路报错。 */
    private fun cachedKeys(): ByteArray? =
        store.readPublicKeys()?.takeIf { keys ->
            try {
                core.accountRows(keys)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    /** 本机保存的清单包。验不过就当没有，退回内置清单，不把损坏的文件交给核对。 */
    private fun manifestPackage(): ByteArray? =
        store.readManifest()?.takeIf { bytes ->
            try {
                core.manifestVersion(bytes)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    // A11

    /** 底部导航进来时保持上次选的账户，A3 点账户进来时选中那个账户。 */
    fun openReceive(account: AccountRow? = null) {
        if (meta == null) return
        guarded {
            receiveAccount = account ?: receiveAccount ?: accounts.first()
            stack = listOf(Screen.Receive)
            loadReceive()
        }
    }

    fun selectReceive(account: AccountRow) {
        if (account == receiveAccount) return
        guarded {
            receiveAccount = account
            loadReceive()
        }
    }

    /** 序号只增不减，每次都是没有发放过的地址。从地址列表发起，发完回到收款页显示新地址。 */
    fun nextReceiveAddress() {
        val account = receiveAccount?.takeIf { receiveHasNext } ?: return
        guarded {
            withContext(work) { store.writeReceiveIndex(account.path, store.readReceiveIndex(account.path) + 1u) }
            loadReceive()
            if (screen == Screen.ReceiveAddresses) stack = stack.dropLast(1)
        }
    }

    fun openReceiveAddresses() {
        if (screen == Screen.Receive) push(Screen.ReceiveAddresses)
    }

    /** 回到收款页显示发放过的地址。序号不变，下一次「使用新地址」仍然是新的。 */
    fun showReceive(address: String) = guarded {
        try {
            receiveQr = withContext(work) { core.qrText(address) }
            receiveAddress = address
        } catch (_: WalletCoreException) {
            showNotice("地址生成失败", failed = true)
        }
        if (screen == Screen.ReceiveAddresses) stack = stack.dropLast(1)
    }

    /** 只用公钥，不需要 PIN。BIP39 密码钱包的公钥不在内存时先读公钥，回来再走这里。 */
    private suspend fun loadReceive() {
        val account = receiveAccount ?: return
        receiveAddress = null
        receiveQr = null
        receiveHistory = emptyList()
        val keys = withContext(work) { reviewKeys() }
        if (keys == null) {
            unlockFor(UnlockPurpose.LoadKeys)
            return
        }
        try {
            val (history, qr) =
                withContext(work) {
                    val last = if (account.network == Network.Bitcoin) store.readReceiveIndex(account.path) else 0u
                    val history = (last downTo 0u).map { core.receiveAddress(keys, account.path, it) }
                    history to core.qrText(history.first())
                }
            receiveHistory = history
            receiveAddress = history.first()
            receiveQr = qr
        } catch (_: WalletCoreException) {
            showNotice("地址生成失败", failed = true)
        }
    }

    private fun clearReceive() {
        receiveAccount = null
        receiveAddress = null
        receiveQr = null
        receiveHistory = emptyList()
    }

    // A5

    /** 自检不通过时停在 AS。 */
    fun openScan() = guarded {
        if (!recheck()) return@guarded
        if (!scanner.hasPermission() && !scanner.requestPermission()) {
            cameraBroken = false
            push(Screen.CameraDenied)
            return@guarded
        }
        beginScan()
        push(Screen.Scan)
    }

    private fun beginScan() {
        dropRequest()
        receiver = core.newReceiver()
    }

    /** AS 页的「重新扫描」。 */
    fun rescan() {
        if (busy) return
        dropRequest()
        stack = listOf(Screen.Home)
        openScan()
    }

    private fun closeScan() {
        dropRequest()
        stack = stack.dropLast(1)
    }

    /** 请求用完或离开时清掉，不留到下一次。 */
    private fun dropRequest() {
        receiver?.close()
        receiver = null
        lastPart = null
        scanned = null
        scanResult = null
        scanFrames = 0
        scanProgress = 0f
        mismatchFingerprint = null
        holdCancel()
    }

    /** 相机会连续交来同一帧，只处理变化的帧。处理不过来的帧丢掉，发送端是循环的。 */
    fun onScanned(part: String) {
        val current = receiver ?: return
        if (busy || screen != Screen.Scan || part == lastPart) return
        lastPart = part
        // 已经开始收帧后，镜头里混进别的码不算失败，继续等下一帧，收到的进度不作废。
        val type = current.urType
        if (type != null && !part.lowercase().startsWith("ur:$type/")) return
        guarded {
            try {
                val progress = withContext(work) { current.receive(part) }
                val received = current.urType
                if (received !in SCANNABLE) {
                    failScan("这个二维码不是给冷钱包扫的")
                    return@guarded
                }
                scanFrames = current.fragmentCount.coerceAtLeast(1)
                scanProgress = progress
                if (current.complete) {
                    scanProgress = 1f
                    val message = withContext(work) { current.message() }
                    receiver = null
                    current.close()
                    scanned = Scanned(received.orEmpty(), message)
                    evaluateScanned()
                }
            } catch (e: WalletCoreException) {
                failScan(e.message.orEmpty())
            }
        }
    }

    private fun failScan(reason: String) {
        dropRequest()
        scanFailure = reason
        showOnly(Screen.ParseFailed)
    }

    private fun refuse(reason: String) {
        dropRequest()
        refusal = reason
        showOnly(Screen.Forbidden)
    }

    /** 按二维码类型分流，收齐后直接进 A6、A9 或 A12。公钥不在手边时先解锁读公钥，回来再走这里。 */
    private suspend fun evaluateScanned() {
        val current = scanned ?: return
        try {
            if (current.type == UR_MANIFEST) {
                val version =
                    withContext(work) {
                        core.verifyManifest(current.message, core.manifestVersion(manifestPackage()))
                    }
                scanResult = ScanResult.Manifest(version)
                stack = listOf(Screen.Settings, Screen.Manifest)
                return
            }
            val keys = withContext(work) { reviewKeys() }
            when {
                keys == null -> unlockFor(UnlockPurpose.LoadKeys)
                current.type == UR_REQUEST -> evaluateRequest(current.message, keys)
                else -> evaluateAddress(current.message, keys)
            }
        } catch (e: WalletCoreException) {
            failScan(e.message.orEmpty())
        }
    }

    private suspend fun evaluateRequest(message: ByteArray, keys: ByteArray) {
        val result =
            withContext(work) { core.reviewRequest(message, keys, store.readHighestChange(), manifestPackage()) }
        when {
            result.fingerprintMismatch -> {
                mismatchFingerprint = result.requestFingerprint
                showOnly(Screen.FingerprintMismatch)
            }
            result.refused -> refuse(result.alerts.firstOrNull() ?: "这笔交易包含禁止的动作")
            else -> {
                scanResult = ScanResult.Request(result)
                stack = listOf(Screen.Home, Screen.Review)
            }
        }
    }

    private suspend fun evaluateAddress(message: ByteArray, keys: ByteArray) {
        val result = withContext(work) { core.reviewAddress(message, keys) }
        if (result.fingerprintMismatch) {
            mismatchFingerprint = result.requestFingerprint
            showOnly(Screen.FingerprintMismatch)
        } else {
            scanResult = ScanResult.Address(result)
            stack = listOf(Screen.Home, Screen.CheckAddress)
        }
    }

    /** 相机打不开，和没有权限不同，不引导去系统设置。 */
    fun cameraFailed() {
        if (screen != Screen.Scan || busy) return
        dropRequest()
        cameraBroken = true
        showOnly(Screen.CameraDenied)
    }

    fun openCameraSettings() = scanner.openSettings()

    /** 重新输入 BIP39 密码后重新核对。 */
    fun retryPassphrase() = guarded {
        if (!recheck()) return@guarded
        unlockFor(UnlockPurpose.LoadKeys)
    }

    /** 拒绝和 AS 的「退出」。 */
    fun exitRequest() {
        if (busy) return
        dropRequest()
        stack = listOf(Screen.Home)
    }

    // A6

    fun confirmTap() {
        if (review?.confirm == ConfirmMode.Tap && screen == Screen.Review) proceedToPin()
    }

    /** 按下时开始计时，满 3 秒才继续，中途松手作废。Tap 和 Refuse 都走不到这里。 */
    fun holdStart() {
        if (review?.confirm != ConfirmMode.Hold || screen != Screen.Review || busy || holdJob != null) return
        holdJob = scope.launch {
            repeat(HOLD_TICKS) { tick ->
                delay(HOLD_TICK_MS)
                holdProgress = (tick + 1f) / HOLD_TICKS
            }
            holdJob = null
            holdProgress = 0f
            proceedToPin()
        }
    }

    fun holdCancel() {
        holdJob?.cancel()
        holdJob = null
        holdProgress = 0f
    }

    private fun proceedToPin() = guarded {
        if (!recheck()) return@guarded
        unlockFor(UnlockPurpose.Sign)
    }

    // A12

    /** A10 只进这一页，「更新清单」才打开相机。 */
    fun openManifest() {
        if (screen == Screen.Settings) push(Screen.Manifest)
    }

    suspend fun loadManifestVersion() {
        manifestVersion = withContext(work) { core.manifestVersion(manifestPackage()) }
    }

    /** 版本号和代币列表都来自同一份清单包。 */
    suspend fun loadManifest() {
        val (version, entries) =
            withContext(work) {
                val bytes = manifestPackage()
                core.manifestVersion(bytes) to core.manifestEntries(bytes)
            }
        manifestVersion = version
        manifestEntries = entries
    }

    private fun leaveManifest() {
        dropRequest()
        stack = stack.dropLast(1)
    }

    /** 扫到的包已经验过签，这里要自检加 PIN，通过后才写入。 */
    fun applyManifest() {
        if (scanResult !is ScanResult.Manifest) return
        guarded {
            if (!recheck()) return@guarded
            unlockFor(UnlockPurpose.ApplyManifest)
        }
    }

    // A7

    private fun unlockFor(purpose: UnlockPurpose) {
        unlockPurpose = purpose
        resetUnlockEntry()
        refreshPinStatus()
        push(Screen.Unlock)
    }

    private fun resetUnlockEntry() {
        unlockPin = ""
        unlockPassphrase = ""
        unlockError = null
    }

    /** 倒计时读单调时钟，不靠界面计数。 */
    fun refreshPinStatus() {
        pinFailures = guard.failures
        waitMs = guard.remainingMs()
    }

    fun unlockDigit(digit: Char) {
        if (busy || waitMs > 0 || unlockPin.length >= PIN_LENGTH) return
        unlockPin += digit
        unlockError = null
    }

    fun unlockDelete() {
        if (!busy) unlockPin = unlockPin.dropLast(1)
    }

    fun passphraseInput(char: Char) {
        if (!busy) unlockPassphrase += char
    }

    fun passphraseDelete() {
        if (!busy) unlockPassphrase = unlockPassphrase.dropLast(1)
    }

    private fun leaveUnlock() {
        resetUnlockEntry()
        stack = stack.dropLast(1)
        if (unlockPurpose != UnlockPurpose.LoadKeys) return
        // 没有公钥就核对不了，取消等于放弃这次扫码。没有公钥也算不出收款地址。
        if (screen == Screen.Scan) closeScan()
        if (screen == Screen.Receive) openHome()
    }

    fun submitUnlock() {
        if (!canSubmitUnlock) return
        val pin = unlockPin
        val passphrase = if (unlockNeedsPassphrase) unlockPassphrase else ""
        val purpose = unlockPurpose
        val newPin = pinFirst
        val message = scanned?.message
        guarded {
            // 输入框不等结果，先清空。
            resetUnlockEntry()
            // 解开钱包之前再自检一次，这是签名前的最后一步。
            if (!recheck() || screen != Screen.Unlock) return@guarded
            val outcome =
                withContext(work) {
                    attemptUnlock(pin, passphrase, purpose) { wallet, saved ->
                        useWallet(purpose, wallet, saved, newPin, message)
                    }
                }
            when (outcome) {
                is Attempt.Opened -> {
                    pinFailures = 0
                    waitMs = 0
                    canWipe = true
                    applyUnlocked(outcome.result)
                }
                Attempt.Wrong -> refreshPinStatus()
                Attempt.Wait -> refreshPinStatus()
                Attempt.Cancelled -> unlockError = "需要确认系统屏幕锁"
                is Attempt.Failed -> {
                    refreshPinStatus()
                    unlockError = outcome.reason
                }
                Attempt.Lost -> {
                    dropRequest()
                    stack = listOf(Screen.NeedsRecovery)
                }
                Attempt.Wiped -> {
                    clearSession()
                    stack = listOf(Screen.Wiped)
                }
            }
        }
    }

    private sealed interface Attempt {
        class Opened(val result: Unlocked) : Attempt

        data object Wrong : Attempt

        data object Wait : Attempt

        data object Cancelled : Attempt

        class Failed(val reason: String) : Attempt

        data object Lost : Attempt

        data object Wiped : Attempt
    }

    /** 解开钱包做完一件事之后带回来的结果，里面没有钱包对象。 */
    private sealed interface Unlocked {
        class Exported(val stream: FrameStream, val fingerprint: String, val keys: ByteArray) : Unlocked

        class KeysLoaded(val keys: ByteArray, val fingerprint: String) : Unlocked

        class Signed(val stream: FrameStream, val fingerprint: String, val keys: ByteArray) : Unlocked

        class Mismatch(val requestFingerprint: String?) : Unlocked

        class Refused(val reason: String) : Unlocked

        class Words(val list: List<String>) : Unlocked

        data object PinChanged : Unlocked

        data object EraseConfirmed : Unlocked

        data object ManifestConfirmed : Unlocked
    }

    private suspend fun applyUnlocked(result: Unlocked) {
        when (result) {
            is Unlocked.Exported -> {
                rememberKeys(result.keys, result.fingerprint)
                showFrames(result.stream)
                showOnly(Screen.Export)
            }
            is Unlocked.KeysLoaded -> {
                rememberKeys(result.keys, result.fingerprint)
                stack = stack.dropLast(1)
                if (screen == Screen.Receive) {
                    loadAddresses()
                    loadReceive()
                } else {
                    evaluateScanned()
                }
            }
            is Unlocked.Signed -> {
                rememberKeys(result.keys, result.fingerprint)
                showFrames(result.stream)
                scanned = null
                stack = listOf(Screen.Home, Screen.Signed)
            }
            is Unlocked.Mismatch -> {
                mismatchFingerprint = result.requestFingerprint
                stack = listOf(Screen.Home, Screen.FingerprintMismatch)
            }
            is Unlocked.Refused -> {
                stack = listOf(Screen.Home, Screen.Review)
                refuse(result.reason)
            }
            is Unlocked.Words -> {
                viewWords = result.list
                showOnly(Screen.ViewWords)
            }
            Unlocked.PinChanged -> {
                changingPin = false
                resetPinEntry()
                stack = listOf(Screen.Settings)
                showNotice("PIN 已修改")
            }
            Unlocked.EraseConfirmed -> {
                withContext(work) { eraseData() }
                clearSession()
                stack = listOf(Screen.Welcome)
            }
            Unlocked.ManifestConfirmed -> {
                val current = scanned ?: return
                withContext(work) { store.writeManifest(current.message) }
                dropRequest()
                stack = listOf(Screen.Settings, Screen.Manifest)
                loadManifest()
                showNotice("清单已更新")
            }
        }
    }

    private fun rememberKeys(keys: ByteArray, fingerprint: String) {
        sessionFingerprint = fingerprint
        if (meta?.passphrase == true) sessionKeys = keys
    }

    /** 在后台线程运行。钱包由 [attemptUnlock] 关闭，这里不要留下引用。 */
    private suspend fun useWallet(
        purpose: UnlockPurpose,
        wallet: UnlockedWallet,
        saved: WalletMeta,
        newPin: String,
        message: ByteArray?,
    ): Unlocked =
        when (purpose) {
            UnlockPurpose.Export ->
                Unlocked.Exported(wallet.exportAccounts(saved.imported), wallet.fingerprint(), keysOf(wallet, saved))
            UnlockPurpose.LoadKeys -> Unlocked.KeysLoaded(keysOf(wallet, saved), wallet.fingerprint())
            UnlockPurpose.Sign -> sign(wallet, saved, requireNotNull(message))
            UnlockPurpose.ViewWords -> Unlocked.Words(wallet.words())
            UnlockPurpose.ChangePin -> {
                changePin(wallet, newPin)
                Unlocked.PinChanged
            }
            UnlockPurpose.Wipe -> Unlocked.EraseConfirmed
            UnlockPurpose.ApplyManifest -> Unlocked.ManifestConfirmed
        }

    /** 公钥缓存缺失时顺手补上。BIP39 密码钱包的公钥不落盘，由 [rememberKeys] 放进内存。 */
    private fun keysOf(wallet: UnlockedWallet, saved: WalletMeta): ByteArray {
        val keys = wallet.publicKeys(saved.imported)
        if (!saved.passphrase && cachedKeys() == null) store.writePublicKeys(keys)
        return keys
    }

    /** 用刚解开的钱包算出的公钥重新核对，不用缓存：BIP39 密码这次可能输错，核对页用的公钥不一定对应这个钱包。 找零索引在返回之前落盘，屏幕上出现签名时本机已经记下。 */
    private fun sign(wallet: UnlockedWallet, saved: WalletMeta, message: ByteArray): Unlocked {
        val keys = keysOf(wallet, saved)
        val highest = store.readHighestChange()
        val manifest = manifestPackage()
        val check = core.reviewRequest(message, keys, highest, manifest)
        if (check.fingerprintMismatch) return Unlocked.Mismatch(check.requestFingerprint)
        if (check.refused) return Unlocked.Refused(check.alerts.firstOrNull() ?: "这笔交易包含禁止的动作")
        val outcome = wallet.signRequest(message, keys, highest, manifest)
        outcome.changeIndex?.let { index ->
            val slot = if (outcome.changeIsTestnet) highest.testnet else highest.mainnet
            if (index > slot) store.writeHighestChange(outcome.changeIsTestnet, index)
        }
        return Unlocked.Signed(outcome.frames, wallet.fingerprint(), keys)
    }

    /** 不删硬件密钥：新密文落盘之前，旧密文必须一直能解开。 */
    private suspend fun changePin(wallet: UnlockedWallet, newPin: String) {
        store.replaceVault(wallet.seal(newPin, hardwareKey.bind(newPin)))
    }

    /** 旧格式在输对 PIN 时换成当前格式。新密文落盘后才删旧密钥，中途断电时旧密文还能解开。 */
    private suspend fun migrate(wallet: UnlockedWallet, pin: String) {
        store.replaceVault(wallet.seal(pin, hardwareKey.enroll(pin)))
        hardwareKey.dropLegacy()
    }

    private suspend fun attemptUnlock(
        pin: String,
        passphrase: String,
        purpose: UnlockPurpose,
        use: suspend (UnlockedWallet, WalletMeta) -> Unlocked,
    ): Attempt {
        if (guard.remainingMs() > 0) return Attempt.Wait
        val vault = store.readVault() ?: return Attempt.Lost
        val current = store.readMeta() ?: return Attempt.Lost
        // 硬件层失败不是 PIN 错误，不计数，也绝不能因此抹除。
        val (sealed, tag) =
            try {
                if (core.isBoundVault(vault)) {
                    // 每猜一次 PIN 都要过一次系统屏幕锁，硬件算出结果才能试。
                    vault to hardwareKey.bind(pin)
                } else {
                    // 旧格式只在输错过时才确认手机屏幕锁。杀进程重来也算，计数已经落盘。
                    if (guard.failures > 0) hardwareKey.confirmDeviceLock()
                    hardwareKey.unwrap(vault) to null
                }
            } catch (_: HardwareKeyException.AuthCancelled) {
                return Attempt.Cancelled
            } catch (_: HardwareKeyException) {
                return Attempt.Lost
            }
        val failures = guard.beginAttempt()
        val wallet: UnlockedWallet =
            try {
                if (tag != null) {
                    core.open(sealed, pin, tag, passphrase)
                } else {
                    core.openLegacy(sealed, pin, passphrase)
                }
            } catch (_: WalletCoreException.WrongPin) {
                if (failures < MAX_PIN_FAILURES) return Attempt.Wrong
                eraseData()
                return Attempt.Wiped
            } catch (_: WalletCoreException) {
                return Attempt.Lost
            }
        try {
            guard.clear()
            if (hardwareKey.lockChanged()) hardwareKey.acceptLock()
            if (tag == null) migrate(wallet, pin)
            // 结果里只有公钥、帧和助记词文字，这一步之后种子不留在内存。
            return Attempt.Opened(use(wallet, current))
        } catch (_: HardwareKeyException.AuthCancelled) {
            return Attempt.Cancelled
        } catch (_: HardwareKeyException) {
            return Attempt.Failed("硬件密钥不可用")
        } catch (_: WalletCoreException) {
            return Attempt.Failed(if (purpose == UnlockPurpose.Sign) "签名失败，请重试" else "操作失败，请重试")
        } finally {
            wallet.close()
        }
    }

    // A4

    /** 轮播暂停。喷泉码后面的帧也能用，恢复后接着发新帧。 */
    var framesPaused by mutableStateOf(false)
        private set

    /** [Qr.intervals] 的下标，0 是默认速度。 */
    var frameSpeed by mutableStateOf(0)
        private set

    fun togglePaused() {
        framesPaused = !framesPaused
    }

    fun nextSpeed() {
        frameSpeed = (frameSpeed + 1) % Qr.intervals.size
    }

    /** 循环取帧，间隔见 [Qr.intervals]。由 A4 的协程驱动，离开页面时取消。 取帧在后台线程，所以结束时由这里关闭，不能在主线程直接关掉正在用的对象。 */
    suspend fun playFrames() {
        val stream = frames ?: return
        val total = frameTotal
        var sent = 0
        playing = true
        try {
            while (frames === stream) {
                val next =
                    try {
                        withContext(work) { core.qrMatrix(stream.nextPart()) }
                    } catch (_: WalletCoreException) {
                        showNotice("二维码生成失败", failed = true)
                        break
                    }
                frame = next
                frameIndex = sent % total + 1
                sent++
                delay(Qr.intervals[frameSpeed].toLong())
                snapshotFlow { framesPaused }.first { !it }
            }
        } finally {
            playing = false
            if (frames !== stream) stream.close()
        }
    }

    private fun showFrames(stream: FrameStream) {
        disposeFrames()
        frames = stream
        frame = null
        frameIndex = 1
        frameTotal = stream.fragmentCount.coerceAtLeast(1)
        framesPaused = false
        frameSpeed = 0
    }

    private fun disposeFrames() {
        val stream = frames
        frames = null
        frame = null
        if (!playing) stream?.close()
    }

    fun finishExport() {
        disposeFrames()
        stack = stack.dropLast(1)
    }

    // A8

    fun finishSigned() {
        disposeFrames()
        dropRequest()
        stack = listOf(Screen.Home)
    }

    // A10

    fun startViewWords() = guarded {
        if (!recheck()) return@guarded
        unlockFor(UnlockPurpose.ViewWords)
    }

    fun leaveViewWords() {
        viewWords = emptyList()
        stack = stack.filterNot { it == Screen.ViewWords }
    }

    fun startChangePin() = guarded {
        if (!recheck()) return@guarded
        resetPinEntry()
        changingPin = true
        push(Screen.SetPin)
    }

    fun continueChangePin() {
        if (changingPin && pinDone) unlockFor(UnlockPurpose.ChangePin)
    }

    private fun leaveChangePin() {
        changingPin = false
        stack = stack.filterNot { it == Screen.SetPin || it == Screen.Unlock }
    }

    /** 设置页的二次确认之后才调用。自检通过、PIN 正确才抹除，错误计入 [PinGuard]。 */
    fun startWipe() = guarded {
        if (!recheck() || !canWipe) return@guarded
        unlockFor(UnlockPurpose.Wipe)
    }

    /** 先删硬件密钥，这一步之后密文就解不开了。中途断电时下次启动进入「需恢复」，不会留下可用的钱包。 */
    private fun eraseData() {
        hardwareKey.delete()
        store.wipe()
    }

    private fun clearSession() {
        disposeFrames()
        dropRequest()
        meta = null
        sessionFingerprint = null
        sessionKeys = null
        addresses = emptyMap()
        accounts = Accounts
        clearReceive()
        manifestEntries = emptyList()
        viewWords = emptyList()
        changingPin = false
        pinFailures = 0
        waitMs = 0
    }

    /** 离开前台时清掉正在输入的 PIN 和 BIP39 密码。 */
    fun onBackground() {
        resetUnlockEntry()
        pinFirst = ""
        pinInput = ""
        pinMismatch = false
        pinWeak = false
        holdCancel()
        // 系统屏幕锁确认也会走到这里，操作进行中不动页面。
        if (busy) return
        if (changingPin) leaveChangePin()
        if (Screen.ViewWords in stack) leaveViewWords()
        if (passphraseRequired) forgetSessionKeys()
    }

    /** BIP39 密码钱包的公钥只在内存里，离开前台就清掉，从它们算出的地址也不留。再用到时走 LoadKeys。 */
    private fun forgetSessionKeys() {
        sessionKeys = null
        addresses = emptyMap()
        receiveAddress = null
        receiveQr = null
    }

    /** 错误 10 次之后的「重新开始」。 */
    fun restartAfterWipe() {
        stack = listOf(Screen.Booting)
        onResume()
    }
}

package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.AddressReview
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.FrameReceiver
import com.ccobcode.bitone.platform.FrameStream
import com.ccobcode.bitone.platform.QrScanner
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.platform.UnlockedWallet
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletMeta
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** 长按 3 秒,每 50 毫秒更新一次进度. */
private const val HOLD_TICKS = 60
private const val HOLD_TICK_MS = 50L

private const val UR_REQUEST = "bitone-request"
private const val UR_ADDRESS = "bitone-address"
private const val UR_MANIFEST = "bitone-manifest"
private val SCANNABLE = setOf(UR_REQUEST, UR_ADDRESS, UR_MANIFEST)

/** A6 到 A6e.只看核心库给的类别和告警,不做额外判断. */
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

/** A5 收齐后的结果.A6,A9 和 A12 显示的都是这一份. */
sealed interface ScanResult {
    class Request(val review: ReviewResult) : ScanResult

    class Address(val review: AddressReview) : ScanResult

    class Manifest(val version: UInt) : ScanResult
}

private class Scanned(val type: String, val message: ByteArray)

/** 签名这一步带回来的结果,里面没有钱包对象. */
private sealed interface SignOutcome {
    class Signed(val stream: FrameStream) : SignOutcome

    class Mismatch(val requestFingerprint: String?) : SignOutcome

    class Refused(val reason: String) : SignOutcome
}

/** A5 扫码,按二维码类型进 A6 核对交易,A9 核对地址或 A12 更新清单. */
class ScanFlow internal constructor(private val host: FlowHost, private val player: FramePlayer) {
    private val core = host.platform.core
    private val store = host.platform.store
    private val session = host.session
    private val work = host.work
    val scanner: QrScanner = host.platform.scanner

    // A5

    private var receiver: FrameReceiver? = null
    private var lastPart: String? = null
    private var scanned: Scanned? = null

    var frames by mutableStateOf(0)
        private set

    var progress by mutableStateOf(0f)
        private set

    var result by mutableStateOf<ScanResult?>(null)
        private set

    val review: ReviewResult?
        get() = (result as? ScanResult.Request)?.review

    val addressReview: AddressReview?
        get() = (result as? ScanResult.Address)?.review

    var holdProgress by mutableStateOf(0f)
        private set

    private var holdJob: Job? = null

    /** 长按满了,等用户再确认一次.只有 [ConfirmMode.HoldConfirm] 才走到这里. */
    var awaitingSecondConfirm by mutableStateOf(false)
        private set

    private var confirmedTwice = false

    /** A6 的内容看到底才能确认.一屏放得下时一进页面就是 true,看过之后不再收回. */
    var reviewRead by mutableStateOf(false)
        private set

    fun reviewScrolled(canScrollForward: Boolean) {
        if (!canScrollForward) reviewRead = true
    }

    /** 通用列表里真正经过画行代码的行.签名时把行数传回核心库,和节点数不同就拒绝,所以列表漏画一行就签不了. */
    private val shownRows = mutableSetOf<UInt>()

    /** 画出一行时调用.同一行画多次只算一次. */
    fun rowShown(id: UInt) {
        shownRows += id
    }

    // AS

    var failure by mutableStateOf("")
        private set

    /** 请求里声明的指纹.地址核对的结果里没有这一项. */
    var mismatchFingerprint by mutableStateOf<String?>(null)
        private set

    var refusal by mutableStateOf("")
        private set

    var cameraBroken by mutableStateOf(false)
        private set

    // A5

    /** 自检不通过时停在 AS,确认后接着打开. */
    fun open() = host.operation {
        if (!scanner.hasPermission() && !scanner.requestPermission()) {
            cameraBroken = false
            host.push(Screen.CameraDenied)
            return@operation
        }
        beginScan()
        host.push(Screen.Scan)
    }

    private fun beginScan() {
        dropRequest()
        receiver = core.newReceiver()
    }

    /** AS 页的"重新扫描". */
    fun rescan() {
        if (host.busy) return
        dropRequest()
        host.stack = listOf(Screen.Home)
        open()
    }

    internal fun close() {
        dropRequest()
        host.pop()
    }

    /** 请求用完或离开时清掉,不留到下一次. */
    internal fun dropRequest() {
        receiver?.close()
        receiver = null
        lastPart = null
        scanned = null
        result = null
        frames = 0
        progress = 0f
        mismatchFingerprint = null
        shownRows.clear()
        awaitingSecondConfirm = false
        confirmedTwice = false
        holdCancel()
    }

    /** 相机会连续交来同一帧,只处理变化的帧.处理不过来的帧丢掉,发送端是循环的. */
    fun onScanned(part: String) {
        val current = receiver ?: return
        if (host.busy || host.screen != Screen.Scan || part == lastPart) return
        lastPart = part
        // 已经开始收帧后,镜头里混进别的码不算失败,继续等下一帧,收到的进度不作废.
        val type = current.urType
        if (type != null && !part.lowercase().startsWith("ur:$type/")) return
        host.guarded {
            try {
                val advanced = withContext(work) { current.receive(part) }
                val received = current.urType
                if (received !in SCANNABLE) {
                    failScan("这个二维码不是给冷钱包扫的")
                    return@guarded
                }
                frames = current.fragmentCount.coerceAtLeast(1)
                progress = advanced
                if (current.complete) {
                    progress = 1f
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
        failure = reason
        host.showOnly(Screen.ParseFailed)
    }

    private fun refuse(reason: String) {
        dropRequest()
        refusal = reason
        host.showOnly(Screen.Forbidden)
    }

    /** 按二维码类型分流,收齐后直接进 A6,A9 或 A12.公钥不在手边时先解锁读公钥,回来再走这里. */
    internal suspend fun evaluateScanned() {
        val current = scanned ?: return
        try {
            if (current.type == UR_MANIFEST) {
                val version =
                    withContext(work) {
                        core.verifyManifest(current.message, core.manifestVersion(session.manifestPackage()))
                    }
                result = ScanResult.Manifest(version)
                host.stack = listOf(Screen.Settings, Screen.Manifest)
                return
            }
            val keys = withContext(work) { session.reviewKeys() }
            when {
                keys == null -> host.loadKeys()
                current.type == UR_REQUEST -> evaluateRequest(current.message, keys)
                else -> evaluateAddress(current.message, keys)
            }
        } catch (e: WalletCoreException) {
            failScan(e.message.orEmpty())
        }
    }

    private suspend fun evaluateRequest(message: ByteArray, keys: ByteArray) {
        val reviewed =
            withContext(work) {
                core.reviewRequest(message, keys, store.readHighestChange(), session.manifestPackage())
            }
        when {
            reviewed.fingerprintMismatch -> {
                mismatchFingerprint = reviewed.requestFingerprint
                host.showOnly(Screen.FingerprintMismatch)
            }
            reviewed.refused -> refuse(reviewed.alerts.firstOrNull() ?: "这笔交易包含禁止的动作")
            else -> {
                reviewRead = false
                shownRows.clear()
                awaitingSecondConfirm = false
                confirmedTwice = false
                result = ScanResult.Request(reviewed)
                host.stack = listOf(Screen.Home, Screen.Review)
            }
        }
    }

    private suspend fun evaluateAddress(message: ByteArray, keys: ByteArray) {
        val reviewed = withContext(work) { core.reviewAddress(message, keys) }
        if (reviewed.fingerprintMismatch) {
            mismatchFingerprint = reviewed.requestFingerprint
            host.showOnly(Screen.FingerprintMismatch)
        } else {
            result = ScanResult.Address(reviewed)
            host.stack = listOf(Screen.Home, Screen.CheckAddress)
        }
    }

    /** 相机打不开,和没有权限不同,不引导去系统设置. */
    fun cameraFailed() {
        if (host.screen != Screen.Scan || host.busy) return
        dropRequest()
        cameraBroken = true
        host.showOnly(Screen.CameraDenied)
    }

    fun openCameraSettings() = scanner.openSettings()

    /** 拒绝和 AS 的"退出". */
    fun exitRequest() {
        if (host.busy) return
        dropRequest()
        host.stack = listOf(Screen.Home)
    }

    // A6

    fun confirmTap() {
        if (reviewRead && review?.confirm == ConfirmMode.Tap && host.screen == Screen.Review) proceedToPin()
    }

    /** 按下时开始计时,满 3 秒才继续,中途松手作废.Tap 和 Refuse 都走不到这里. */
    fun holdStart() {
        val confirm = review?.confirm
        if (confirm != ConfirmMode.Hold && confirm != ConfirmMode.HoldConfirm) return
        if (host.screen != Screen.Review || host.busy || !reviewRead || holdJob != null) return
        confirmedTwice = false
        holdJob =
            host.scope.launch {
                repeat(HOLD_TICKS) { tick ->
                    delay(HOLD_TICK_MS)
                    holdProgress = (tick + 1f) / HOLD_TICKS
                }
                holdJob = null
                holdProgress = 0f
                if (confirm == ConfirmMode.HoldConfirm) awaitingSecondConfirm = true else proceedToPin()
            }
    }

    /** 二次确认.只有长按满了才会出现这一步. */
    fun confirmSecond() {
        if (!awaitingSecondConfirm || review?.confirm != ConfirmMode.HoldConfirm) return
        if (host.screen != Screen.Review || host.busy) return
        awaitingSecondConfirm = false
        confirmedTwice = true
        proceedToPin()
    }

    /** 不继续,回到长按. */
    fun cancelSecondConfirm() {
        awaitingSecondConfirm = false
    }

    fun holdCancel() {
        holdJob?.cancel()
        holdJob = null
        holdProgress = 0f
    }

    private fun proceedToPin() = host.operation { host.unlock(UnlockRequest(UnlockPurpose.Sign) { signTask() }) }

    private fun signTask(): UnlockTask<SignOutcome> {
        val message = scanned?.message
        val shown = review
        val rendered = shownRows.size.toUInt()
        val twice = confirmedTwice
        return UnlockTask(
            use = { wallet, saved ->
                sign(wallet, saved, requireNotNull(message), requireNotNull(shown), rendered, twice)
            },
            finish = { outcome -> applySigned(outcome) },
        )
    }

    /** 用刚解开的钱包算出的公钥重新核对,不用缓存:核对页用的是本机文件里的公钥,不一定对应解开的这个钱包.找零索引在返回之前落盘,屏幕上出现签名时本机已经记下. */
    private fun sign(
        wallet: UnlockedWallet,
        saved: WalletMeta,
        message: ByteArray,
        shown: ReviewResult,
        rendered: UInt,
        twice: Boolean,
    ): SignOutcome {
        val keys = session.keysOf(wallet, saved)
        val highest = store.readHighestChange()
        val manifest = session.manifestPackage()
        val check = core.reviewRequest(message, keys, highest, manifest)
        if (check.fingerprintMismatch) return SignOutcome.Mismatch(check.requestFingerprint)
        if (check.refused) return SignOutcome.Refused(check.alerts.firstOrNull() ?: "这笔交易包含禁止的动作")
        val outcome = wallet.signRequest(message, keys, highest, manifest, shown.reviewDigest, rendered, twice)
        outcome.changeIndex?.let { index ->
            val slot = if (outcome.changeIsTestnet) highest.testnet else highest.mainnet
            if (index > slot) store.writeHighestChange(outcome.changeIsTestnet, index)
        }
        return SignOutcome.Signed(outcome.frames)
    }

    private suspend fun applySigned(outcome: SignOutcome) {
        when (outcome) {
            is SignOutcome.Signed -> {
                // 解开钱包要几秒,这期间新出现的失败要先确认.签名丢掉,确认后回到 A7 重来.
                if (!host.recheck() || host.screen != Screen.Unlock) {
                    outcome.stream.close()
                    return
                }
                player.show(outcome.stream)
                scanned = null
                host.stack = listOf(Screen.Home, Screen.Signed)
            }
            is SignOutcome.Mismatch -> {
                mismatchFingerprint = outcome.requestFingerprint
                host.stack = listOf(Screen.Home, Screen.FingerprintMismatch)
            }
            is SignOutcome.Refused -> {
                host.stack = listOf(Screen.Home, Screen.Review)
                refuse(outcome.reason)
            }
        }
    }

    // A8

    fun finishSigned() {
        player.dispose()
        dropRequest()
        host.stack = listOf(Screen.Home)
    }

    // A12

    internal fun leaveManifest() {
        dropRequest()
        host.pop()
    }

    /** 扫到的包已经验过签,这里要自检加 PIN,通过后才写入. */
    fun applyManifest() {
        if (result !is ScanResult.Manifest) return
        host.operation {
            host.unlock(
                UnlockRequest(UnlockPurpose.ApplyManifest) {
                    UnlockTask(use = { _, _ -> }, finish = { writeManifest() })
                }
            )
        }
    }

    private suspend fun writeManifest() {
        val current = scanned ?: return
        withContext(work) { store.writeManifest(current.message) }
        dropRequest()
        host.stack = listOf(Screen.Settings, Screen.Manifest)
        host.manifestChanged()
        host.showNotice("清单已更新")
    }
}

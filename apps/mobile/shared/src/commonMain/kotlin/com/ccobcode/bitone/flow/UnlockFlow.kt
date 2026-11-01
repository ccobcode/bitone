package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.HardwareKeyException
import com.ccobcode.bitone.platform.UnlockedWallet
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.platform.walletMeta
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.withContext

/** A7 解开钱包之后做什么.钱包只活在这一次操作里,操作结束立刻关闭. */
enum class UnlockPurpose {
    Export,
    Sign,
    LoadKeys,
    ViewWords,
    ChangePin,
    Wipe,
    ApplyManifest,
}

/** 助记词和 PIN 层都不依赖 BIP39 密码,这些用途只验证 PIN,不用再输密码. */
private val PIN_ONLY =
    setOf(UnlockPurpose.ViewWords, UnlockPurpose.ChangePin, UnlockPurpose.Wipe, UnlockPurpose.ApplyManifest)

/**
 * 解开钱包要做的一件事.[use] 在后台线程运行,钱包由 [UnlockFlow] 关闭,这里不要留下引用.
 *
 * 结果里只有公钥,帧和助记词文字,没有钱包对象.[finish] 回到主线程,只拿得到这个结果.
 */
internal class UnlockTask<T>(
    val use: suspend (UnlockedWallet, WalletMeta) -> T,
    val finish: suspend (T) -> Unit,
)

/** 发起方交给 A7 的请求.[task] 在提交时才调用,要用的输入在这一刻取走.[cancelled] 在离开 A7 之后调用. */
internal class UnlockRequest(
    val purpose: UnlockPurpose,
    val cancelled: () -> Unit = {},
    val task: () -> UnlockTask<*>,
)

private sealed interface Attempt<out T> {
    class Opened<T>(val result: T) : Attempt<T>

    data object Wrong : Attempt<Nothing>

    data object Wait : Attempt<Nothing>

    data object Cancelled : Attempt<Nothing>

    class Failed(val reason: String) : Attempt<Nothing>

    data object Lost : Attempt<Nothing>
}

/** A7 输 PIN,解开钱包,做完发起方交来的事.这里不知道发起方是谁. */
class UnlockFlow internal constructor(private val host: FlowHost, private val guard: PinGuard) {
    private val core = host.platform.core
    private val store = host.platform.store
    private val hardwareKey = host.platform.hardwareKey
    private val session = host.session

    private var request: UnlockRequest? = null

    var pin by mutableStateOf("")
        private set

    var passphrase by mutableStateOf("")
        private set

    var failures by mutableStateOf(guard.failures)
        private set

    var waitMs by mutableStateOf(0L)
        private set

    var error by mutableStateOf<String?>(null)
        private set

    var purpose by mutableStateOf(UnlockPurpose.Export)
        private set

    val needsPassphrase: Boolean
        get() = session.passphraseRequired && purpose !in PIN_ONLY

    val canSubmit: Boolean
        get() =
            !host.busy &&
                waitMs == 0L &&
                pin.length == PIN_LENGTH &&
                (!needsPassphrase || passphrase.isNotEmpty() || session.passphraseOptional)

    internal fun open(next: UnlockRequest) {
        request = next
        purpose = next.purpose
        resetEntry()
        refreshStatus()
        host.push(Screen.Unlock)
    }

    internal fun resetEntry() {
        pin = ""
        passphrase = ""
        error = null
    }

    /** 倒计时读单调时钟,不靠界面计数. */
    fun refreshStatus() {
        failures = guard.failures
        waitMs = guard.remainingMs()
    }

    fun digit(digit: Char) {
        if (host.busy || waitMs > 0 || pin.length >= PIN_LENGTH) return
        pin += digit
        error = null
    }

    fun delete() {
        if (!host.busy) pin = pin.dropLast(1)
    }

    fun passphraseInput(char: Char) {
        if (!host.busy) passphrase += char
    }

    fun passphraseDelete() {
        if (!host.busy) passphrase = passphrase.dropLast(1)
    }

    internal fun leave() {
        resetEntry()
        host.pop()
        request?.cancelled?.invoke()
    }

    fun submit() {
        if (!canSubmit) return
        val enteredPin = pin
        val enteredPassphrase = if (needsPassphrase) passphrase else ""
        val current = purpose
        val task = request?.task?.invoke()
        host.guarded {
            // 输入框不等结果,先清空.
            resetEntry()
            // 解开钱包之前再自检一次,这是签名前的最后一步.停在 AS 时 PIN 已经清掉,确认后回到这页重新输入.
            if (!host.recheck() || host.screen != Screen.Unlock || task == null) return@guarded
            perform(task, enteredPin, enteredPassphrase, current)
        }
    }

    private suspend fun <T> perform(
        task: UnlockTask<T>,
        enteredPin: String,
        enteredPassphrase: String,
        current: UnlockPurpose,
    ) {
        val outcome = withContext(host.work) { attempt(enteredPin, enteredPassphrase, current, task.use) }
        when (outcome) {
            is Attempt.Opened -> {
                failures = 0
                waitMs = 0
                task.finish(outcome.result)
            }
            Attempt.Wrong -> refreshStatus()
            Attempt.Wait -> refreshStatus()
            Attempt.Cancelled -> error = "需要确认系统屏幕锁"
            is Attempt.Failed -> {
                refreshStatus()
                error = outcome.reason
            }
            Attempt.Lost -> host.walletLost()
        }
    }

    /** 旧格式在输对 PIN 时换成当前格式.新密文落盘后才删旧密钥,中途断电时旧密文还能解开.失败时留着旧密文,下次解锁再换,不影响这次使用. */
    private suspend fun migrate(wallet: UnlockedWallet, enteredPin: String) {
        try {
            store.replaceVault(wallet.seal(enteredPin, hardwareKey.enroll(enteredPin)))
            hardwareKey.dropLegacy()
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {}
    }

    private suspend fun <T> attempt(
        enteredPin: String,
        enteredPassphrase: String,
        current: UnlockPurpose,
        use: suspend (UnlockedWallet, WalletMeta) -> T,
    ): Attempt<T> {
        if (guard.remainingMs() > 0) return Attempt.Wait
        val vault = store.readVault() ?: return Attempt.Lost
        val saved = store.walletMeta() ?: return Attempt.Lost
        // 硬件层失败不是 PIN 错误,不计数.只有密钥确实不在了才算丢失,其他错误可以重试.
        val (sealed, tag) =
            try {
                if (core.isBoundVault(vault)) {
                    // 每猜一次 PIN 都要过一次系统屏幕锁,硬件算出结果才能试.
                    vault to hardwareKey.bind(enteredPin)
                } else {
                    // 旧格式只在输错过时才确认手机屏幕锁.杀进程重来也算,计数已经落盘.
                    if (guard.failures > 0) hardwareKey.confirmDeviceLock()
                    hardwareKey.unwrap(vault) to null
                }
            } catch (_: HardwareKeyException.AuthCancelled) {
                return Attempt.Cancelled
            } catch (_: HardwareKeyException.Lost) {
                return Attempt.Lost
            } catch (_: HardwareKeyException) {
                return Attempt.Failed("硬件密钥暂时不可用,请重试")
            }
        guard.beginAttempt()
        val wallet: UnlockedWallet =
            try {
                if (tag != null) {
                    core.open(sealed, enteredPin, tag, enteredPassphrase)
                } else {
                    core.openLegacy(sealed, enteredPin, enteredPassphrase)
                }
            } catch (_: WalletCoreException.WrongPin) {
                return Attempt.Wrong
            } catch (_: WalletCoreException) {
                return Attempt.Lost
            }
        try {
            guard.clear()
            if (hardwareKey.lockChanged()) hardwareKey.acceptLock()
            if (tag == null) migrate(wallet, enteredPin)
            // 结果里只有公钥,帧和助记词文字,这一步之后种子不留在内存.
            return Attempt.Opened(use(wallet, saved))
        } catch (_: HardwareKeyException.AuthCancelled) {
            return Attempt.Cancelled
        } catch (_: HardwareKeyException) {
            return Attempt.Failed("硬件密钥不可用")
        } catch (_: WalletCoreException) {
            return Attempt.Failed(if (current == UnlockPurpose.Sign) "签名失败,请重试" else "操作失败,请重试")
        } finally {
            wallet.close()
        }
    }

    /** 新钱包落盘后计数归零. */
    internal fun clearFailures() {
        failures = 0
    }

    internal fun clear() {
        failures = 0
        waitMs = 0
    }
}

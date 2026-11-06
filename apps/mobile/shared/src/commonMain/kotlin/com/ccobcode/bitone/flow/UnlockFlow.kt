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

/** 只核对 PIN,用不到解开的钱包.旧格式的 BIP39 密码钱包这时不问密码,也不迁移,留到下一次要用钱包的解锁. */
private val PIN_ONLY = setOf(UnlockPurpose.Wipe, UnlockPurpose.ApplyManifest)

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
    /** [meta] 和 [outdated] 是解开之后本机保存的状态. */
    class Opened<T>(val result: T, val meta: WalletMeta, val outdated: Boolean) : Attempt<T>

    data object Wrong : Attempt<Nothing>

    data object Wait : Attempt<Nothing>

    data object Cancelled : Attempt<Nothing>

    class Failed(val reason: String) : Attempt<Nothing>

    data object Lost : Attempt<Nothing>

    /** 旧格式要先输 BIP39 密码.PIN 和硬件都还没用,不计数. */
    data object NeedsPassphrase : Attempt<Nothing>
}

private class Settled(val meta: WalletMeta, val outdated: Boolean)

/** A7 输 PIN,解开钱包,做完发起方交来的事.这里不知道发起方是谁.BIP39 密码在密文里,只有旧格式迁移时问一次. */
class UnlockFlow
internal constructor(
    private val host: FlowHost,
    private val guard: PinGuard,
    private val passphrase: PassphraseEntry,
) {
    private val core = host.platform.core
    private val store = host.platform.store
    private val hardwareKey = host.platform.hardwareKey
    private val session = host.session

    private var request: UnlockRequest? = null

    var pin by mutableStateOf("")
        private set

    var failures by mutableStateOf(guard.failures)
        private set

    var waitMs by mutableStateOf(0L)
        private set

    var error by mutableStateOf<String?>(null)
        private set

    var purpose by mutableStateOf(UnlockPurpose.Export)
        private set

    /** 旧格式的 BIP39 密码钱包第一次解锁:密文里没有密码,先输两遍,和 A2d 一样. */
    var upgrading by mutableStateOf(false)
        private set

    /** 两遍一致的密码,输 PIN 时用.PIN 输错时留着,解锁成功,离开 A7 或退到后台时清掉. */
    private var confirmed by mutableStateOf<String?>(null)

    /** 还在输密码这一步. */
    val asksPassphrase: Boolean
        get() = upgrading && confirmed == null

    /** 元数据丢了,不知道有没有设置过密码,可以留空. */
    val passphraseOptional: Boolean
        get() = session.meta?.passphraseOptional == true

    val canSubmit: Boolean
        get() = !host.busy && waitMs == 0L && pin.length == PIN_LENGTH && !asksPassphrase

    internal fun open(next: UnlockRequest) {
        request = next
        purpose = next.purpose
        resetEntry()
        forgetPassphrase()
        upgrading = next.purpose !in PIN_ONLY && session.upgradeNeedsPassphrase
        refreshStatus()
        host.push(Screen.Unlock)
    }

    internal fun resetEntry() {
        pin = ""
        error = null
    }

    internal fun forgetPassphrase() {
        passphrase.reset()
        confirmed = null
    }

    /** 密码这一步的"完成".两遍一致之后才出现 PIN 键盘. */
    fun passphraseDone() {
        if (host.busy || !asksPassphrase) return
        confirmed = passphrase.next(allowEmpty = passphraseOptional) ?: return
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

    internal fun leave() {
        resetEntry()
        forgetPassphrase()
        upgrading = false
        host.pop()
        request?.cancelled?.invoke()
    }

    fun submit() {
        if (!canSubmit) return
        val enteredPin = pin
        val enteredPassphrase = confirmed
        val current = purpose
        val task = request?.task?.invoke()
        host.guarded {
            // 输入框不等结果,先清空.两遍一致的密码留着,PIN 输错时不用再输.
            resetEntry()
            // 解开钱包之前再自检一次,这是签名前的最后一步.停在 AS 时 PIN 已经清掉,确认后回到这页重新输入.
            if (!host.recheck() || host.screen != Screen.Unlock || task == null) return@guarded
            perform(task, enteredPin, enteredPassphrase, current)
        }
    }

    private suspend fun <T> perform(
        task: UnlockTask<T>,
        enteredPin: String,
        enteredPassphrase: String?,
        current: UnlockPurpose,
    ) {
        val outcome = withContext(host.work) { attempt(enteredPin, enteredPassphrase, current, task.use) }
        when (outcome) {
            is Attempt.Opened -> {
                failures = 0
                waitMs = 0
                session.meta = outcome.meta
                session.outdated = outcome.outdated
                forgetPassphrase()
                upgrading = false
                task.finish(outcome.result)
            }
            Attempt.NeedsPassphrase -> {
                forgetPassphrase()
                upgrading = true
            }
            Attempt.Wrong -> refreshStatus()
            Attempt.Wait -> refreshStatus()
            Attempt.Cancelled -> error = "需要确认系统屏幕锁"
            is Attempt.Failed -> {
                refreshStatus()
                error = outcome.reason
            }
            Attempt.Lost -> {
                forgetPassphrase()
                host.walletLost()
            }
        }
    }

    /**
     * 解开的是真正的钱包时,本机保存的东西都换成它的.旧格式换成当前格式,BIP39 密码这时封进去.迁移失败时留着旧密文,下次解锁再换,不影响这次使用.
     *
     * 指纹不对时写回这个钱包的指纹和公钥,旧格式的 BIP39 密码钱包和元数据丢失的钱包靠这一步补上.公钥先写,元数据对上了公钥就一定写好了.写不进去也不影响这次使用,下次解锁再写.
     */
    private suspend fun settle(
        wallet: UnlockedWallet,
        saved: WalletMeta,
        outdated: Boolean,
        enteredPin: String,
        tag: ByteArray?,
    ): Settled {
        val stillOutdated = outdated && !migrate(wallet, enteredPin, tag)
        val fingerprint = wallet.fingerprint()
        if (saved.fingerprint == fingerprint) return Settled(saved, stillOutdated)
        val next = WalletMeta(saved.imported, wallet.hasPassphrase(), fingerprint)
        return try {
            store.writePublicKeys(wallet.publicKeys(next.imported))
            store.writeMeta(next)
            Settled(next, stillOutdated)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            Settled(saved, stillOutdated)
        }
    }

    /** 新密文落盘后才删旧密钥,中途断电时旧密文还能解开.只存熵的格式已经绑定硬件,同一个 PIN 的结果不变,不用再确认一次屏幕锁. */
    private suspend fun migrate(wallet: UnlockedWallet, enteredPin: String, tag: ByteArray?): Boolean =
        try {
            store.replaceVault(wallet.seal(enteredPin, tag ?: hardwareKey.enroll(enteredPin)))
            if (tag == null) hardwareKey.dropLegacy()
            true
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            false
        }

    private suspend fun <T> attempt(
        enteredPin: String,
        enteredPassphrase: String?,
        current: UnlockPurpose,
        use: suspend (UnlockedWallet, WalletMeta) -> T,
    ): Attempt<T> {
        if (guard.remainingMs() > 0) return Attempt.Wait
        val vault = store.readVault() ?: return Attempt.Lost
        val saved = store.walletMeta() ?: return Attempt.Lost
        val outdated = !core.isCurrentVault(vault)
        // 旧格式的密文里只有熵.启用了 BIP39 密码的用这次输两遍的密码,没有启用的用空密码.
        val passphrase = if (outdated && saved.passphrase) enteredPassphrase else ""
        // 只核对 PIN 的用途不问密码.这时解开的不是真正的钱包,不迁移也不写元数据.
        if (passphrase == null && current !in PIN_ONLY) return Attempt.NeedsPassphrase
        // 硬件层失败不是 PIN 错误,不计数.只有密钥确实不在了才算丢失,其他错误可以重试.
        val (sealed, tag) =
            try {
                if (core.isBoundVault(vault)) {
                    // 每猜一次 PIN 都要过一次系统屏幕锁,硬件算出结果才能试.
                    vault to hardwareKey.bind(enteredPin)
                } else {
                    // 最早的格式只在输错过时才确认手机屏幕锁.杀进程重来也算,计数已经落盘.
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
                when {
                    tag == null -> core.openLegacy(sealed, enteredPin, passphrase.orEmpty())
                    outdated -> core.openEntropyOnly(sealed, enteredPin, tag, passphrase.orEmpty())
                    else -> core.open(sealed, enteredPin, tag)
                }
            } catch (_: WalletCoreException.WrongPin) {
                return Attempt.Wrong
            } catch (_: WalletCoreException) {
                return Attempt.Lost
            }
        try {
            guard.clear()
            if (hardwareKey.lockChanged()) hardwareKey.acceptLock()
            val settled =
                if (passphrase == null) {
                    Settled(saved, outdated)
                } else {
                    settle(wallet, saved, outdated, enteredPin, tag)
                }
            // 结果里只有公钥,帧和助记词文字,这一步之后种子不留在内存.
            return Attempt.Opened(use(wallet, settled.meta), settled.meta, settled.outdated)
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
        forgetPassphrase()
        upgrading = false
    }
}

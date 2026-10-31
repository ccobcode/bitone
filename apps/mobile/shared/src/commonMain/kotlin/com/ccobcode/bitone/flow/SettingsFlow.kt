package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.i18n.I18n
import com.ccobcode.bitone.i18n.Lang
import kotlinx.coroutines.withContext

/** A10 的偏好,以及从这里发起的查看助记词,修改 PIN 和抹除. */
class SettingsFlow internal constructor(private val host: FlowHost, private val pin: PinEntry) {
    private val platform = host.platform
    private val store = platform.store
    private val hardwareKey = platform.hardwareKey

    val appVersion: String
        get() = platform.appVersion

    val certificate: String
        get() = platform.certificate

    var autoBrightness by mutableStateOf(store.readAutoBrightness())
        private set

    fun toggleAutoBrightness() {
        autoBrightness = !autoBrightness
        store.writeAutoBrightness(autoBrightness)
    }

    /** null 是自动,跟随系统. */
    var languageChoice by mutableStateOf(store.readLanguage()?.let(Lang::of))
        private set

    fun chooseLanguage(choice: Lang?) {
        languageChoice = choice
        store.writeLanguage(choice?.code)
        applyLanguage()
    }

    private fun applyLanguage() {
        I18n.lang = languageChoice ?: Lang.of(platform.systemLanguage)
    }

    init {
        applyLanguage()
    }

    var viewWords by mutableStateOf(emptyList<String>())
        private set

    fun startViewWords() = host.guarded {
        if (!host.recheck()) return@guarded
        host.unlock(
            UnlockRequest(UnlockPurpose.ViewWords) {
                UnlockTask(
                    use = { wallet, _ -> wallet.words() },
                    finish = { words ->
                        viewWords = words
                        host.showOnly(Screen.ViewWords)
                    },
                )
            }
        )
    }

    fun leaveViewWords() {
        viewWords = emptyList()
        host.stack = host.stack.filterNot { it == Screen.ViewWords }
    }

    /** A10 修改 PIN 复用 A2c.先输新 PIN,最后在 A7 输当前 PIN,钱包只在最后一步解开一次. */
    var changingPin by mutableStateOf(false)
        private set

    fun startChangePin() = host.guarded {
        if (!host.recheck()) return@guarded
        pin.reset()
        changingPin = true
        host.push(Screen.SetPin)
    }

    fun continueChangePin() {
        if (changingPin && pin.done) host.unlock(UnlockRequest(UnlockPurpose.ChangePin) { changePinTask() })
    }

    private fun changePinTask(): UnlockTask<Unit> {
        val newPin = pin.first
        return UnlockTask(
            // 不删硬件密钥:新密文落盘之前,旧密文必须一直能解开.
            use = { wallet, _ -> store.replaceVault(wallet.seal(newPin, hardwareKey.bind(newPin))) },
            finish = {
                changingPin = false
                pin.reset()
                host.stack = listOf(Screen.Settings)
                host.showNotice("PIN 已修改")
            },
        )
    }

    /** A2c 的返回键,建钱包时也走这里. */
    internal fun cancelChangePin() {
        changingPin = false
    }

    internal fun leaveChangePin() {
        changingPin = false
        host.stack = host.stack.filterNot { it == Screen.SetPin || it == Screen.Unlock }
    }

    /** 设置页的二次确认之后才调用.自检通过,PIN 正确才抹除,错误计入 [PinGuard]. */
    fun startWipe() = host.guarded {
        if (!host.recheck() || !host.session.canWipe) return@guarded
        host.unlock(
            UnlockRequest(UnlockPurpose.Wipe) {
                UnlockTask(
                    use = { _, _ -> },
                    finish = {
                        withContext(host.work) { host.eraseData() }
                        host.walletErased()
                        host.stack = listOf(Screen.Welcome)
                    },
                )
            }
        )
    }

    internal fun clear() {
        viewWords = emptyList()
        changingPin = false
    }
}

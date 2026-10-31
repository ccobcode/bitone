package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.ui.theme.Motion
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private class LoadedKeys(val keys: ByteArray, val fingerprint: String)

/**
 * 根协调者,管导航栈,busy,提示,生命周期,自检和会话.具体流程在各个子流程里,页面直接用子流程.
 *
 * 助记词,PIN,BIP39 密码都只放在子流程的内存状态里,不进 Bundle,所以 Activity 被回收时不会写到 saved instance state.输入在用完或离开页面时立刻清空.解开的钱包不进任何字段.
 */
class WalletFlow(
    private val platform: Platform,
    private val scope: CoroutineScope,
    private val work: CoroutineDispatcher,
) {
    private val store = platform.store
    private val hardwareKey = platform.hardwareKey
    private val guard = PinGuard(store, platform.clock, hardwareKey::lockChanged)

    var stack by mutableStateOf(listOf<Screen>(Screen.Booting))
        private set

    val screen: Screen
        get() = stack.last()

    var busy by mutableStateOf(false)
        private set

    var notice by mutableStateOf<String?>(null)
        private set

    /** 这条提示是失败,显示告警图标. */
    var noticeFailed by mutableStateOf(false)
        private set

    private var noticeSerial = 0

    var report by mutableStateOf<SelfCheckReport?>(null)
        private set

    val session = Session(platform, work)
    val pin = PinEntry(platform.core)

    private val host: FlowHost = Host()
    val player = FramePlayer(host)
    val setup = SetupFlow(host, pin)
    val unlock = UnlockFlow(host, guard)
    val scan = ScanFlow(host, player)
    val receive = ReceiveFlow(host)
    val export = ExportFlow(host, player)
    val manifest = ManifestFlow(host)
    val settings = SettingsFlow(host, pin)

    private inner class Host : FlowHost {
        override val platform = this@WalletFlow.platform
        override val scope = this@WalletFlow.scope
        override val work = this@WalletFlow.work

        override val session
            get() = this@WalletFlow.session

        override var stack: List<Screen>
            get() = this@WalletFlow.stack
            set(value) {
                this@WalletFlow.stack = value
            }

        override val screen
            get() = this@WalletFlow.screen

        override val busy
            get() = this@WalletFlow.busy

        override fun push(next: Screen) = this@WalletFlow.push(next)

        override fun showOnly(next: Screen) {
            stack = stack.dropLast(1) + next
        }

        override fun pop() {
            stack = stack.dropLast(1)
        }

        override fun guarded(block: suspend () -> Unit) = this@WalletFlow.guarded(block)

        override fun showNotice(text: String, failed: Boolean) = this@WalletFlow.showNotice(text, failed)

        override suspend fun recheck() = this@WalletFlow.recheck()

        override fun unlock(request: UnlockRequest) = this@WalletFlow.unlock.open(request)

        override fun loadKeys() = this@WalletFlow.unlock.open(keysRequest)

        override fun walletSaved(saved: WalletMeta) {
            session.saved(saved)
            this@WalletFlow.unlock.clearFailures()
            receive.clear()
        }

        override fun walletLost() = this@WalletFlow.walletLost()

        override fun eraseData() = this@WalletFlow.eraseData()

        override fun walletErased() = clearSession()

        override suspend fun manifestChanged() = manifest.load()
    }

    /** 收款和扫码都可能缺公钥,取消和读到之后按当时的页面分别处理. */
    private val keysRequest =
        UnlockRequest(
            purpose = UnlockPurpose.LoadKeys,
            // 没有公钥就核对不了,取消等于放弃这次扫码.没有公钥也算不出收款地址.
            cancelled = {
                if (screen == Screen.Scan) scan.close()
                if (screen == Screen.Receive) openHome()
            },
        ) {
            UnlockTask(
                use = { wallet, saved -> LoadedKeys(session.keysOf(wallet, saved), wallet.fingerprint()) },
                finish = { loaded ->
                    session.remember(loaded.keys, loaded.fingerprint)
                    stack = stack.dropLast(1)
                    if (screen == Screen.Receive) {
                        session.loadAddresses()
                        receive.load()
                    } else {
                        scan.evaluateScanned()
                    }
                },
            )
        }

    /** 启动和每次回到前台都调用,见 docs/04 §2.2. */
    fun onResume() {
        scope.launch {
            recheck()
            // 从系统设置授权回来,直接进扫码.
            if (screen == Screen.CameraDenied && !scan.cameraBroken && scan.scanner.hasPermission()) scan.rescan()
            // BIP39 密码钱包的公钥在后台时清掉了,收款页要重新读.
            if (screen == Screen.Receive && receive.address == null) guarded { receive.load() }
        }
    }

    /** 重新跑一次自检,并按结果决定去哪一页.返回 true 表示没有未通过项. */
    private suspend fun recheck(): Boolean {
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
                    // 上次尝试时被杀进程,计数已经到顶,这里补上抹除.
                    exhausted = wallet && mayWipe && guard.failures >= MAX_PIN_FAILURES,
                )
            }
        report = probe.result
        session.meta = probe.saved
        session.canWipe = probe.mayWipe
        val root = if (probe.wallet) Screen.Home else Screen.Welcome
        when {
            probe.exhausted -> {
                withContext(work) { eraseData() }
                clearSession()
                stack = listOf(Screen.Wiped)
            }
            probe.lost && Screen.NeedsRecovery !in stack -> walletLost()
            probe.result.blocked -> {
                // 助记词页也要锁定,内容不留到自检恢复之后.
                if (Screen.ViewWords in stack) settings.leaveViewWords()
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

    /** AS 页的"重新检测". */
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

    /** 页面上打开的弹层和全屏二维码.返回键先关最后打开的那个,都关了才离开页面. */
    private val layers = mutableListOf<() -> Unit>()

    fun openLayer(close: () -> Unit) {
        layers += close
    }

    fun closeLayer(close: () -> Unit) {
        layers -= close
    }

    /** 返回 false 表示已到栈底,由平台退出应用. */
    fun back(): Boolean {
        layers.lastOrNull()?.let {
            it()
            return true
        }
        if (busy) return true
        when (screen) {
            Screen.Backup -> setup.leaveCreate()
            Screen.Verify -> setup.backToBackup()
            Screen.SetPin -> {
                pin.reset()
                settings.cancelChangePin()
                stack = stack.dropLast(1)
            }
            Screen.ImportWords -> setup.leaveImport()
            Screen.Unlock -> unlock.leave()
            Screen.Export -> export.finish()
            Screen.Settings,
            Screen.Receive -> openHome()
            Screen.Manifest -> scan.leaveManifest()
            Screen.ReceiveAddresses -> stack = stack.dropLast(1)
            Screen.Scan -> scan.close()
            Screen.Signed -> scan.finishSigned()
            Screen.ViewWords -> settings.leaveViewWords()
            Screen.Review,
            Screen.CheckAddress,
            Screen.FingerprintMismatch,
            Screen.ParseFailed,
            Screen.Forbidden,
            Screen.CameraDenied -> scan.exitRequest()
            else -> return false
        }
        return true
    }

    // A3

    /** 没有落盘的钱包就没有首页,验证没通过时走不到这里. */
    fun openHome() {
        if (session.meta != null) stack = listOf(Screen.Home)
    }

    fun openSettings() {
        if (session.meta != null) stack = listOf(Screen.Settings)
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

    private fun walletLost() {
        scan.dropRequest()
        stack = listOf(Screen.NeedsRecovery)
    }

    /** 先删硬件密钥,这一步之后密文就解不开了.中途断电时下次启动进入"需恢复",不会留下可用的钱包. */
    private fun eraseData() {
        hardwareKey.delete()
        store.wipe()
    }

    private fun clearSession() {
        player.dispose()
        scan.dropRequest()
        session.clear()
        receive.clear()
        manifest.clear()
        settings.clear()
        unlock.clear()
    }

    /** 离开前台时清掉正在输入的 PIN 和 BIP39 密码. */
    fun onBackground() {
        unlock.resetEntry()
        pin.clearInput()
        scan.holdCancel()
        // 系统屏幕锁确认也会走到这里,操作进行中不动页面.
        if (busy) return
        if (settings.changingPin) settings.leaveChangePin()
        if (Screen.ViewWords in stack) settings.leaveViewWords()
        if (session.passphraseRequired) forgetSessionKeys()
    }

    private fun forgetSessionKeys() {
        session.forgetKeys()
        receive.forgetAddress()
    }

    /** 错误 10 次之后的"重新开始". */
    fun restartAfterWipe() {
        stack = listOf(Screen.Booting)
        onResume()
    }
}

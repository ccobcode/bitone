package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.platform.walletMeta
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

    /** 操作进行中离开了前台,[onBackground] 没有动页面.系统屏幕锁确认也会这样,回到前台时清掉. */
    private var leftWhileBusy = false

    var report by mutableStateOf<SelfCheckReport?>(null)
        private set

    /** 这次操作里用户确认过的未通过项.只在内存里,下一次操作重新确认. */
    private var acknowledged = emptySet<CheckItem>()

    /** 确认之后接着做的操作. */
    private var resume: (suspend () -> Unit)? = null

    /** AS 可以确认后继续.只有生成新钱包的路上不行,这时还没有要找回的资金. */
    val canAcknowledge: Boolean
        get() = Screen.Backup !in stack

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

        override suspend fun recheckNewWallet(): Boolean {
            acknowledged = emptySet()
            return this@WalletFlow.recheck()
        }

        override fun operation(block: suspend () -> Unit) = guarded { begin(block) }

        override fun unlock(request: UnlockRequest) = this@WalletFlow.unlock.open(request)

        /** A7 先打开,自检有失败时 AS 盖在上面,确认后回到 A7. */
        override suspend fun loadKeys() {
            acknowledged = emptySet()
            this@WalletFlow.unlock.open(keysRequest)
            this@WalletFlow.recheck()
        }

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

    /** 启动和每次回到前台都调用. */
    fun onResume() {
        leftWhileBusy = false
        scope.launch {
            recheck()
            // 从系统设置授权回来,直接进扫码.
            if (screen == Screen.CameraDenied && !scan.cameraBroken && scan.scanner.hasPermission()) scan.rescan()
            // BIP39 密码钱包的公钥在后台时清掉了,收款页要重新读.
            if (screen == Screen.Receive && receive.address == null) guarded { receive.load() }
        }
    }

    /**
     * 重新跑一次自检,并按结果决定去哪一页.返回 true 表示没有要处理的未通过项.
     *
     * 有未确认的失败时停在 AS.确认后执行 [retry],没有 [retry] 就回到原来的页面.生成新钱包不能确认,见 [canAcknowledge].
     */
    private suspend fun recheck(retry: (suspend () -> Unit)? = null): Boolean {
        class Probe(val result: SelfCheckReport, val saved: WalletMeta?, val lost: Boolean)
        val probe =
            withContext(work) {
                val saved = store.walletMeta()
                Probe(evaluate(platform.selfCheck.collect()), saved, saved != null && hardwareKey.isLost())
            }
        report = probe.result
        session.meta = probe.saved
        val root = if (probe.saved != null) Screen.Home else Screen.Welcome
        val pending = probe.result.failures - acknowledged
        when {
            probe.lost && Screen.NeedsRecovery !in stack -> walletLost()
            pending.isNotEmpty() -> {
                // 助记词不留在警告下面.
                if (Screen.ViewWords in stack) settings.leaveViewWords()
                if (retry != null) resume = retry
                if (screen != Screen.SelfCheckFailed) push(Screen.SelfCheckFailed)
            }
            screen == Screen.SelfCheckFailed -> {
                resume = null
                leaveSelfCheck(root)
            }
            screen == Screen.Booting -> stack = listOf(root)
            screen == Screen.NeedsRecovery && !probe.lost -> stack = listOf(root)
        }
        return pending.isEmpty()
    }

    private fun leaveSelfCheck(root: Screen) {
        val below = stack.dropLast(1)
        stack = if (below.isEmpty() || below.last() == Screen.Booting) listOf(root) else below
    }

    /** 一次操作的入口,上一次的确认到这里作废. */
    private suspend fun begin(block: suspend () -> Unit) {
        acknowledged = emptySet()
        proceed(block)
    }

    private suspend fun proceed(block: suspend () -> Unit) {
        if (recheck(retry = { proceed(block) })) block()
    }

    /** AS 页的"重新检测". */
    fun retryCheck() = guarded { recheck() }

    /** AS 页的"仍然继续".这次确认只放行当前的操作,不落盘. */
    fun acknowledgeCheck() {
        if (screen != Screen.SelfCheckFailed || !canAcknowledge) return
        guarded {
            acknowledged = report?.failures.orEmpty()
            leaveSelfCheck(if (session.meta != null) Screen.Home else Screen.Welcome)
            val next = resume
            resume = null
            next?.invoke()
        }
    }

    private fun guarded(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                block()
            } finally {
                busy = false
                // 操作结束时人还不在,助记词不留在页面上.
                if (leftWhileBusy && Screen.ViewWords in stack) settings.leaveViewWords()
                leftWhileBusy = false
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
            Screen.ImportOverwrite -> stack = stack.dropLast(1)
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

    /** A7 的"忘记 PIN".放弃这次操作,走导入.不用 PIN:被替换的密文反正打不开. */
    fun forgotPin() {
        if (busy) return
        unlock.resetEntry()
        pin.reset()
        settings.cancelChangePin()
        scan.dropRequest()
        stack = listOf(Screen.Home)
        setup.startImport()
    }

    /** 有窗口盖住应用时触摸被丢弃,要告诉用户,不然界面像卡死了. */
    fun touchObscured() = showNotice("有其他窗口盖住了应用,关掉它才能操作", failed = true)

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
        scan.cancelSecondConfirm()
        // 系统屏幕锁确认也会走到这里,操作进行中不动页面.
        if (busy) {
            leftWhileBusy = true
            return
        }
        if (settings.changingPin) settings.leaveChangePin()
        if (Screen.ViewWords in stack) settings.leaveViewWords()
        if (session.passphraseRequired) forgetSessionKeys()
        // 离开前台,这次确认作废.
        acknowledged = emptySet()
        resume = null
    }

    private fun forgetSessionKeys() {
        session.forgetKeys()
        receive.forgetAddress()
    }
}

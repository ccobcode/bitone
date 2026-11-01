package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.ManifestEntry
import com.ccobcode.bitone.platform.NATIVE_CONTRACT
import com.ccobcode.bitone.platform.QrMatrix
import com.ccobcode.bitone.platform.WalletCoreException
import kotlinx.coroutines.withContext

/** A11 收款. */
class ReceiveFlow internal constructor(private val host: FlowHost) {
    private val core = host.platform.core
    private val store = host.platform.store
    private val session = host.session
    private val work = host.work

    var account by mutableStateOf<AccountRow?>(null)
        private set

    /** 内置清单里这些账户能收的资产.资产只决定页面上写什么,地址跟着账户走. */
    private var assets by mutableStateOf(emptyList<ManifestEntry>())

    var asset by mutableStateOf<ManifestEntry?>(null)
        private set

    /** 可选的资产,同一个币只列一次. */
    val tokens: List<ManifestEntry>
        get() = assets.distinctBy { it.icon }

    /** 能收当前资产的账户. */
    val accounts: List<AccountRow>
        get() = session.accounts.filter { row -> assets.any { it.icon == asset?.icon && row.holds(it) } }

    var address by mutableStateOf<String?>(null)
        private set

    var qr by mutableStateOf<QrMatrix?>(null)
        private set

    /** 这个账户发放过的地址,最新的在前.使用新地址后旧地址仍然能收款. */
    var history by mutableStateOf<List<String>>(emptyList())
        private set

    /** Bitcoin 每次收款可以换一个地址,其他链只有一个. */
    val hasNext: Boolean
        get() = account?.network == Network.Bitcoin

    /** 底部导航进来时保持上次选的账户,A3 点账户进来时选中那个账户. */
    fun open(next: AccountRow? = null) {
        if (session.meta == null) return
        host.guarded {
            val current = next ?: account ?: session.accounts.first()
            account = current
            host.stack = listOf(Screen.Receive)
            assets =
                withContext(work) { core.manifestEntries(session.manifestPackage()) }
                    .filter { entry -> session.accounts.any { it.holds(entry) } }
            if (next != null || asset == null) {
                asset = assets.firstOrNull { current.holds(it) && it.contract == NATIVE_CONTRACT }
            }
            load()
        }
    }

    /** 当前账户收不了这个币时,换到第一个能收的账户. */
    fun pickAsset(entry: ManifestEntry) {
        asset = entry
        if (account !in accounts) accounts.firstOrNull()?.let(::select)
    }

    fun select(next: AccountRow) {
        if (next == account) return
        host.guarded {
            account = next
            load()
        }
    }

    /** 序号只增不减,每次都是没有发放过的地址.从地址列表发起,发完回到收款页显示新地址. */
    fun nextAddress() {
        val current = account?.takeIf { hasNext } ?: return
        host.guarded {
            withContext(work) { store.writeReceiveIndex(current.path, store.readReceiveIndex(current.path) + 1u) }
            load()
            if (host.screen == Screen.ReceiveAddresses) host.pop()
        }
    }

    fun openAddresses() {
        if (host.screen == Screen.Receive) host.push(Screen.ReceiveAddresses)
    }

    /** 回到收款页显示发放过的地址.序号不变,下一次"使用新地址"仍然是新的. */
    fun show(shown: String) = host.guarded {
        try {
            qr = withContext(work) { core.qrText(shown) }
            address = shown
        } catch (_: WalletCoreException) {
            host.showNotice("地址生成失败", failed = true)
        }
        if (host.screen == Screen.ReceiveAddresses) host.pop()
    }

    /** 只用公钥,不需要 PIN.BIP39 密码钱包的公钥不在内存时先读公钥,回来再走这里. */
    internal suspend fun load() {
        val current = account ?: return
        address = null
        qr = null
        history = emptyList()
        val keys = withContext(work) { session.reviewKeys() }
        if (keys == null) {
            host.loadKeys()
            return
        }
        try {
            val (issued, code) =
                withContext(work) {
                    val last = if (current.network == Network.Bitcoin) store.readReceiveIndex(current.path) else 0u
                    val issued = (last downTo 0u).map { core.receiveAddress(keys, current.path, it) }
                    issued to core.qrText(issued.first())
                }
            history = issued
            address = issued.first()
            qr = code
        } catch (_: WalletCoreException) {
            host.showNotice("地址生成失败", failed = true)
        }
    }

    internal fun clear() {
        account = null
        asset = null
        address = null
        qr = null
        history = emptyList()
    }

    /** 公钥离开内存后,从它们算出的地址也不留. */
    internal fun forgetAddress() {
        address = null
        qr = null
    }
}

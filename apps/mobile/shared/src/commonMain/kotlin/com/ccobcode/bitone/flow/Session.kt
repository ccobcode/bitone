package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.UnlockedWallet
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletMeta
import com.ccobcode.bitone.platform.walletMeta
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext

/** 已保存的钱包和从公钥缓存算出的地址.这里只有公开数据,解开的钱包不进任何字段. */
class Session internal constructor(platform: Platform, private val work: CoroutineDispatcher) {
    private val core = platform.core
    private val store = platform.store

    var meta by mutableStateOf(store.walletMeta())
        internal set

    /** 本机密文是旧格式,里面没有 BIP39 密码.和 [meta] 一起在每次自检时重读,下一次解开时换成当前格式. */
    internal var outdated = store.readVault()?.let { !core.isCurrentVault(it) } == true

    val fingerprint: String?
        get() = meta?.fingerprint

    /** 旧格式的 BIP39 密码钱包,解开之前要先输两遍密码.元数据丢了时也问,可以留空. */
    internal val upgradeNeedsPassphrase: Boolean
        get() = outdated && meta?.passphrase == true

    /** 派生路径到地址.公钥缓存里没有的账户不显示地址. */
    var addresses by mutableStateOf(emptyMap<String, String>())
        private set

    /** A3 和 A11 列出的账户,公钥读到之后换成核心库给的. */
    var accounts by mutableStateOf(Accounts)
        private set

    /** 由 A3 驱动,解锁回来时会重算. */
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

    /** 解不开的公钥缓存当作没有,下次解锁时重新生成. */
    internal fun reviewKeys(): ByteArray? =
        store.readPublicKeys()?.takeIf { keys ->
            try {
                core.accountRows(keys)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    /** 本机保存的清单包.核心库读不了时当作没有. */
    internal fun manifestPackage(): ByteArray? =
        store.readManifest()?.takeIf { bytes ->
            try {
                core.manifestVersion(bytes)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    /** 公钥缓存缺失时顺手补上. */
    internal fun keysOf(wallet: UnlockedWallet, saved: WalletMeta): ByteArray {
        val keys = wallet.publicKeys(saved.imported)
        if (reviewKeys() == null) store.writePublicKeys(keys)
        return keys
    }

    /** 新钱包落盘,密文是当前格式. */
    internal fun saved(next: WalletMeta) {
        meta = next
        outdated = false
    }

    internal fun clear() {
        meta = null
        outdated = false
        addresses = emptyMap()
        accounts = Accounts
    }
}

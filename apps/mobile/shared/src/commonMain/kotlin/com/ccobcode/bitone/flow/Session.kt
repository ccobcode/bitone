package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.UnlockedWallet
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletMeta
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.withContext

/** 已保存的钱包和这次解锁留下的公钥信息.这里只有公开数据,解开的钱包不进任何字段. */
class Session internal constructor(platform: Platform, private val work: CoroutineDispatcher) {
    private val core = platform.core
    private val store = platform.store

    var meta by mutableStateOf(store.readMeta())
        internal set

    /** 本次解锁算出的指纹,只留在内存.启用 BIP39 密码时只有它是真指纹. */
    private var sessionFingerprint by mutableStateOf<String?>(null)

    val fingerprint: String?
        get() = sessionFingerprint ?: meta?.fingerprint

    val passphraseRequired: Boolean
        get() = meta?.passphrase == true

    /** BIP39 密码钱包的公钥只放这里,不落盘. */
    private var sessionKeys: ByteArray? = null

    /** 派生路径到地址.公钥缓存里没有的账户不显示地址. */
    var addresses by mutableStateOf(emptyMap<String, String>())
        private set

    /** A3 和 A11 列出的账户,公钥读到之后换成核心库给的. */
    var accounts by mutableStateOf(Accounts)
        private set

    /** 指纹变更后不提供抹除,输对 PIN 后恢复. */
    var canWipe by mutableStateOf(true)
        internal set

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

    internal fun reviewKeys(): ByteArray? = if (meta?.passphrase == true) sessionKeys else cachedKeys()

    /** 解不开的公钥缓存当作没有,下次解锁时重新生成. */
    private fun cachedKeys(): ByteArray? =
        store.readPublicKeys()?.takeIf { keys ->
            try {
                core.accountRows(keys)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    /** 本机保存的清单包.验不过就当没有,退回内置清单,不把损坏的文件交给核对. */
    internal fun manifestPackage(): ByteArray? =
        store.readManifest()?.takeIf { bytes ->
            try {
                core.manifestVersion(bytes)
                true
            } catch (_: WalletCoreException) {
                false
            }
        }

    /** 公钥缓存缺失时顺手补上.BIP39 密码钱包的公钥不落盘,由 [remember] 放进内存. */
    internal fun keysOf(wallet: UnlockedWallet, saved: WalletMeta): ByteArray {
        val keys = wallet.publicKeys(saved.imported)
        if (!saved.passphrase && cachedKeys() == null) store.writePublicKeys(keys)
        return keys
    }

    internal fun remember(keys: ByteArray, fingerprint: String) {
        sessionFingerprint = fingerprint
        if (meta?.passphrase == true) sessionKeys = keys
    }

    /** 新钱包落盘,上一个钱包这次解锁留下的东西作废. */
    internal fun saved(next: WalletMeta) {
        meta = next
        sessionFingerprint = null
        sessionKeys = null
    }

    /** BIP39 密码钱包的公钥只在内存里,离开前台就清掉,从它们算出的地址也不留.再用到时走 LoadKeys. */
    internal fun forgetKeys() {
        sessionKeys = null
        addresses = emptyMap()
    }

    internal fun clear() {
        meta = null
        sessionFingerprint = null
        sessionKeys = null
        addresses = emptyMap()
        accounts = Accounts
    }
}

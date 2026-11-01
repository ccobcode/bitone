package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.WalletMeta
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope

/**
 * 子流程共用的能力,由 [WalletFlow] 实现.
 *
 * 子流程只认识这里,互相不知道对方.一件事牵涉几个子流程时,由 [WalletFlow] 协调.
 */
internal interface FlowHost {
    val platform: Platform
    val scope: CoroutineScope
    val work: CoroutineDispatcher
    val session: Session

    var stack: List<Screen>
    val screen: Screen
    val busy: Boolean

    fun push(next: Screen)

    fun showOnly(next: Screen)

    fun pop()

    fun guarded(block: suspend () -> Unit)

    fun showNotice(text: String, failed: Boolean = false)

    /** 重新跑一次自检,并按结果决定去哪一页.返回 true 表示没有要处理的未通过项:都通过了,或这次操作已经确认过. */
    suspend fun recheck(): Boolean

    /** 生成新钱包之前的自检.确认不算数,有未通过项就停在 AS. */
    suspend fun recheckNewWallet(): Boolean

    /** 一次操作.自检有未确认的失败时停在 AS,确认后接着做 [block].确认只对这一次操作有效. */
    fun operation(block: suspend () -> Unit)

    /** 进 A7,解开钱包做完 [request] 这一件事. */
    fun unlock(request: UnlockRequest)

    /** 公钥不在手边时进 A7 读公钥,读到之后回到发起的页面.这也算一次操作. */
    suspend fun loadKeys()

    fun walletSaved(saved: WalletMeta)

    /** 硬件密钥丢失,只能用助记词恢复. */
    fun walletLost()

    /** 在后台线程调用. */
    fun eraseData()

    /** 用户抹除了钱包,清掉全部子流程的状态. */
    fun walletErased()

    suspend fun manifestChanged()
}

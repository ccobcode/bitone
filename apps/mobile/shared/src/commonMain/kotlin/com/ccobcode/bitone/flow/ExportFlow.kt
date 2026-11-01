package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.FrameStream

private class Exported(val stream: FrameStream, val fingerprint: String, val keys: ByteArray)

/** A4 导出观察钱包. */
class ExportFlow internal constructor(private val host: FlowHost, private val player: FramePlayer) {
    private val session = host.session

    /** 解锁相当于签名前的一步,先重新自检. */
    fun open() = host.operation { host.unlock(UnlockRequest(UnlockPurpose.Export) { exportTask() }) }

    private fun exportTask() =
        UnlockTask(
            use = { wallet, saved ->
                Exported(wallet.exportAccounts(saved.imported), wallet.fingerprint(), session.keysOf(wallet, saved))
            },
            finish = { exported ->
                session.remember(exported.keys, exported.fingerprint)
                player.show(exported.stream)
                host.showOnly(Screen.Export)
            },
        )

    fun finish() {
        player.dispose()
        host.pop()
    }
}

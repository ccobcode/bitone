package com.ccobcode.bitone.flow

/** A4 导出观察钱包. */
class ExportFlow internal constructor(private val host: FlowHost, private val player: FramePlayer) {
    private val session = host.session

    /** 解锁相当于签名前的一步,先重新自检. */
    fun open() = host.operation { host.unlock(UnlockRequest(UnlockPurpose.Export) { exportTask() }) }

    private fun exportTask() =
        UnlockTask(
            use = { wallet, saved ->
                session.keysOf(wallet, saved)
                wallet.exportAccounts(saved.imported)
            },
            finish = { stream ->
                player.show(stream)
                host.showOnly(Screen.Export)
            },
        )

    fun finish() {
        player.dispose()
        host.pop()
    }
}

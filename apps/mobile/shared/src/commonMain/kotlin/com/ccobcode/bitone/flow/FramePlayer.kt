package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import com.ccobcode.bitone.platform.FrameStream
import com.ccobcode.bitone.platform.QrMatrix
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.ui.theme.Qr
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext

/** 动态二维码的轮播.A4 导出和 A8 签名结果共用. */
class FramePlayer internal constructor(private val host: FlowHost) {
    private val core = host.platform.core
    private val work = host.work

    private var frames: FrameStream? = null
    private var playing = false

    var frame by mutableStateOf<QrMatrix?>(null)
        private set

    var index by mutableStateOf(1)
        private set

    var total by mutableStateOf(1)
        private set

    /** 轮播暂停.喷泉码后面的帧也能用,恢复后接着发新帧. */
    var paused by mutableStateOf(false)
        private set

    /** [Qr.intervals] 的下标,0 是默认速度. */
    var speed by mutableStateOf(0)
        private set

    fun togglePaused() {
        paused = !paused
    }

    fun nextSpeed() {
        speed = (speed + 1) % Qr.intervals.size
    }

    /** 循环取帧,间隔见 [Qr.intervals].由页面的协程驱动,离开页面时取消.取帧在后台线程,所以结束时由这里关闭,不能在主线程直接关掉正在用的对象. */
    suspend fun play() {
        val stream = frames ?: return
        val count = total
        var sent = 0
        playing = true
        try {
            while (frames === stream) {
                val next =
                    try {
                        withContext(work) { core.qrMatrix(stream.nextPart()) }
                    } catch (_: WalletCoreException) {
                        host.showNotice("二维码生成失败", failed = true)
                        break
                    }
                frame = next
                index = sent % count + 1
                sent++
                delay(Qr.intervals[speed].toLong())
                snapshotFlow { paused }.first { !it }
            }
        } finally {
            playing = false
            if (frames !== stream) stream.close()
        }
    }

    internal fun show(stream: FrameStream) {
        dispose()
        frames = stream
        frame = null
        index = 1
        total = stream.fragmentCount.coerceAtLeast(1)
        paused = false
        speed = 0
    }

    internal fun dispose() {
        val stream = frames
        frames = null
        frame = null
        if (!playing) stream?.close()
    }
}

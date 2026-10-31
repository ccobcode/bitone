package com.ccobcode.bitone.platform

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

// 只在 debug 构建里存在，release 里是另一份什么都不放行的实现。
// 模拟器必须开 adb，又只有软件密钥库，自检和建钱包都过不去。放了标记文件才放行，其余检查仍是真实结果。
// 写标记：adb shell run-as com.ccobcode.bitone touch files/dev_skip_debug_checks
// 模拟器的摄像头扫不了真实二维码，A5 打开时从帧文件循环喂帧，一行一帧，读完删除。
// 写帧：adb push frames.txt /data/local/tmp/ && adb shell run-as com.ccobcode.bitone cp /data/local/tmp/frames.txt
// files/dev_frames.txt

private const val FEED_INTERVAL_MS = 120L
private const val FEED_POLL_MS = 300L

private fun marked(context: Context) = File(context.filesDir, "dev_skip_debug_checks").exists()

internal fun softwareKeystoreAllowed(context: Context): Boolean = marked(context)

internal fun adjustFactsForDevelopment(context: Context, facts: DeviceFacts): DeviceFacts {
    if (!marked(context)) return facts
    return facts.copy(
        rooted = false,
        bootloaderLocked = true,
        adbEnabled = false,
        usbDataConnected = false,
        keystore = if (facts.keystore == HardwareLevel.None) HardwareLevel.Tee else facts.keystore,
    )
}

/** 循环喂帧，和网页端循环显示二维码一样，走真实的 FrameReceiver。 */
internal suspend fun feedDevelopmentFrames(context: Context, onCode: (String) -> Unit) {
    if (!marked(context)) return
    val file = File(context.filesDir, "dev_frames.txt")
    var frames = emptyList<String>()
    while (true) {
        val fresh =
            withContext(Dispatchers.IO) {
                if (file.exists()) file.readLines().filter(String::isNotBlank).also { file.delete() } else null
            }
        if (fresh != null) frames = fresh
        frames.forEach { part ->
            onCode(part)
            delay(FEED_INTERVAL_MS)
        }
        if (frames.isEmpty()) delay(FEED_POLL_MS)
    }
}

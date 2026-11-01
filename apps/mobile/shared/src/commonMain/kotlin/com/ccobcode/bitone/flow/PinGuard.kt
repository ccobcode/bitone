package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.BootClock
import com.ccobcode.bitone.platform.PinRecord
import com.ccobcode.bitone.platform.SecureStore

const val PIN_LENGTH = 6

/** 第 5 次错误起等待,30 秒翻倍到 5 分钟封顶.错多少次都不抹除. */
fun pinDelayMs(failures: Int): Long =
    when {
        failures < 5 -> 0
        failures == 5 -> 30_000
        failures == 6 -> 60_000
        failures == 7 -> 120_000
        failures == 8 -> 240_000
        else -> 300_000
    }

/** 指纹变更后:第 6 次错误起等 1,5,15,60 分钟,之后每次 60 分钟. */
fun changedLockDelayMs(failures: Int): Long =
    when {
        failures < 6 -> 0
        failures == 6 -> 60_000
        failures == 7 -> 300_000
        failures == 8 -> 900_000
        else -> 3_600_000
    }

/** PIN 错误计数和延时.计数在派生密钥之前写盘,尝试途中杀进程或断电都会被计入.计数没有防回滚,Root 后恢复旧文件可以重置,见 docs/04 §7. */
class PinGuard(
    private val store: SecureStore,
    private val clock: BootClock,
    private val lockChanged: () -> Boolean,
) {

    val failures: Int
        get() = store.readPinRecord().failures

    /** 还要等多久.重启后单调时钟归零,无法知道已过去多久,所以从头计时. */
    fun remainingMs(): Long {
        val record = store.readPinRecord()
        val delay = if (lockChanged()) changedLockDelayMs(record.failures) else pinDelayMs(record.failures)
        if (delay == 0L) return 0
        val now = clock.now()
        if (now.bootId != record.bootId || now.elapsedMs < record.elapsedMs) {
            store.writePinRecord(PinRecord(record.failures, now.bootId, now.elapsedMs))
            return delay
        }
        return (delay - (now.elapsedMs - record.elapsedMs)).coerceAtLeast(0)
    }

    /** 先记一次失败,解锁成功后再用 [clear] 清掉. */
    fun beginAttempt() {
        val now = clock.now()
        store.writePinRecord(PinRecord(store.readPinRecord().failures + 1, now.bootId, now.elapsedMs))
    }

    fun clear() {
        store.writePinRecord(PinRecord.Clean)
    }
}

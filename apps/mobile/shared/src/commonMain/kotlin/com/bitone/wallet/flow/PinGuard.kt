package com.bitone.wallet.flow

import com.bitone.wallet.platform.BootClock
import com.bitone.wallet.platform.PinRecord
import com.bitone.wallet.platform.SecureStore

const val PIN_LENGTH = 6
const val MAX_PIN_FAILURES = 10

/** 第 5 次错误起等待，30 秒翻倍到 5 分钟封顶。第 10 次抹除，不再等待。 */
fun pinDelayMs(failures: Int): Long =
    when {
        failures < 5 -> 0
        failures == 5 -> 30_000
        failures == 6 -> 60_000
        failures == 7 -> 120_000
        failures == 8 -> 240_000
        else -> 300_000
    }

/** 指纹变更后按 iOS 的规格：第 6 次错误起等 1、5、15、60 分钟，之后每次 60 分钟。 */
fun changedLockDelayMs(failures: Int): Long =
    when {
        failures < 6 -> 0
        failures == 6 -> 60_000
        failures == 7 -> 300_000
        failures == 8 -> 900_000
        else -> 3_600_000
    }

/** PIN 错误计数和延时。计数在派生密钥之前写盘，尝试途中杀进程或断电都会被计入。 计数防回滚还没有做，Root 后恢复旧文件可以重置，见 docs/04 §7。 */
class PinGuard(
    private val store: SecureStore,
    private val clock: BootClock,
    private val lockChanged: () -> Boolean,
) {

    val failures: Int
        get() = store.readPinRecord().failures

    /** 指纹变更后不抹除。 */
    fun mayWipe(): Boolean = !lockChanged()

    /** 还要等多久。重启后单调时钟归零，无法知道已过去多久，所以从头计时。 */
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

    /** 先记一次失败，解锁成功后再用 [clear] 清掉。指纹变更时计数停在上限之前，恢复后这段时间的尝试也不会触发抹除。 */
    fun beginAttempt(): Int {
        val now = clock.now()
        val next = store.readPinRecord().failures + 1
        val failures = if (lockChanged()) next.coerceAtMost(MAX_PIN_FAILURES - 1) else next
        store.writePinRecord(PinRecord(failures, now.bootId, now.elapsedMs))
        return failures
    }

    fun clear() {
        store.writePinRecord(PinRecord.Clean)
    }
}

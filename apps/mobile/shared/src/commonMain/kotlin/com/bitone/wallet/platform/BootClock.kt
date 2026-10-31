package com.bitone.wallet.platform

/** 单调时钟的一次读数。[bootId] 每次开机不同。 */
class ClockReading(val bootId: Long, val elapsedMs: Long)

/** PIN 延时用单调时钟，改系统时间绕不过去。 */
interface BootClock {
    fun now(): ClockReading
}

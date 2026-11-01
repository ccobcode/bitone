package com.ccobcode.bitone.platform

/** 平台只负责采集事实,判定规则在 [com.ccobcode.bitone.flow.evaluate],对应 docs/04 §2.2.null 表示这一项没读出来,判定时按警告处理. */
data class DeviceFacts(
    val airplaneMode: Boolean?,
    val wifiOn: Boolean?,
    val bluetoothOn: Boolean?,
    val nfcOn: Boolean?,
    val rooted: Boolean?,
    val bootloaderLocked: Boolean?,
    val adbEnabled: Boolean?,
    val usbDataConnected: Boolean?,
    val keystore: HardwareLevel?,
    val screenLockSet: Boolean?,
    val thirdPartyAccessibility: Int?,
    val overlayApps: Int?,
)

/** 只记住确定的结果.探测失败时下次重试,"重新检测"才是真的重新检测. */
class DefiniteResult<T : Any>(private val probe: () -> T?) {
    private var value: T? = null

    fun get(): T? = value ?: probe().also { value = it }
}

interface SelfCheck {
    suspend fun collect(): DeviceFacts
}

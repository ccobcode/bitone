package com.ccobcode.bitone.platform

/** 平台只负责采集事实,判定规则在 [com.ccobcode.bitone.flow.evaluate]. */
data class DeviceFacts(
    val airplaneMode: Boolean,
    val wifiOn: Boolean,
    val bluetoothOn: Boolean,
    val nfcOn: Boolean,
    val rooted: Boolean,
    /** null 表示无法确认,按未通过处理. */
    val bootloaderLocked: Boolean?,
    val adbEnabled: Boolean,
    val usbDataConnected: Boolean,
    val keystore: HardwareLevel,
    val screenLockSet: Boolean,
    val thirdPartyAccessibility: Int,
    val overlayApps: Int,
)

interface SelfCheck {
    suspend fun collect(): DeviceFacts
}

package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.DeviceFacts
import com.ccobcode.bitone.platform.HardwareLevel

enum class CheckState {
    Pass,
    Warn,
    Fail,
}

class CheckItem(val label: String, val state: CheckState, val value: String)

class SelfCheckReport(val items: List<CheckItem>) {
    val failed: Int
        get() = items.count { it.state == CheckState.Fail }

    val warned: Int
        get() = items.count { it.state == CheckState.Warn }

    /** 有未通过项时,创建,解锁,显示助记词都不允许. */
    val blocked: Boolean
        get() = failed > 0

    val summary: String
        get() =
            when {
                failed > 0 -> "$failed 项未通过"
                warned > 0 -> "$warned 项警告"
                else -> "全部通过"
            }
}

/** 判定规则,和 docs/04 §2.2 的表逐行对应. */
fun evaluate(f: DeviceFacts): SelfCheckReport =
    SelfCheckReport(
        listOf(
            radios(f),
            rootAndBoot(f),
            debugAndUsb(f),
            keystore(f),
            CheckItem(
                "系统屏幕锁",
                if (f.screenLockSet) CheckState.Pass else CheckState.Fail,
                if (f.screenLockSet) "已设置" else "未设置",
            ),
            CheckItem(
                "无障碍服务",
                if (f.thirdPartyAccessibility == 0) CheckState.Pass else CheckState.Fail,
                if (f.thirdPartyAccessibility == 0) "无第三方" else "${f.thirdPartyAccessibility} 个第三方",
            ),
            CheckItem(
                "悬浮窗应用",
                if (f.overlayApps == 0) CheckState.Pass else CheckState.Warn,
                if (f.overlayApps == 0) "无授权" else "${f.overlayApps} 个应用",
            ),
        )
    )

private fun radios(f: DeviceFacts): CheckItem {
    val problem =
        when {
            f.wifiOn -> "Wi-Fi 已开启"
            f.bluetoothOn -> "蓝牙已开启"
            f.nfcOn -> "NFC 已开启"
            !f.airplaneMode -> "飞行模式未开"
            else -> null
        }
    return CheckItem("飞行模式和无线", if (problem == null) CheckState.Pass else CheckState.Fail, problem ?: "已隔离")
}

private fun rootAndBoot(f: DeviceFacts): CheckItem {
    val problem =
        when {
            f.rooted -> "检测到 Root"
            f.bootloaderLocked == null -> "无法确认 Bootloader"
            !f.bootloaderLocked -> "Bootloader 未锁定"
            else -> null
        }
    return CheckItem(
        "Root 和 Bootloader",
        if (problem == null) CheckState.Pass else CheckState.Fail,
        problem ?: "未 Root,已锁定",
    )
}

private fun debugAndUsb(f: DeviceFacts): CheckItem {
    val problem =
        when {
            f.adbEnabled -> "USB 调试已开启"
            f.usbDataConnected -> "USB 已连接数据"
            else -> null
        }
    return CheckItem("调试和 USB", if (problem == null) CheckState.Pass else CheckState.Fail, problem ?: "已关闭")
}

private fun keystore(f: DeviceFacts): CheckItem =
    when (f.keystore) {
        HardwareLevel.StrongBox -> CheckItem("硬件密钥库", CheckState.Pass, "StrongBox")
        HardwareLevel.Tee -> CheckItem("硬件密钥库", CheckState.Warn, "TEE")
        HardwareLevel.None -> CheckItem("硬件密钥库", CheckState.Fail, "不可用")
    }

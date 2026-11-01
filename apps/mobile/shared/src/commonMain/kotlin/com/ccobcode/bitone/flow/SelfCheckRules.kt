package com.ccobcode.bitone.flow

import com.ccobcode.bitone.platform.DeviceFacts
import com.ccobcode.bitone.platform.HardwareLevel

enum class CheckState {
    Pass,
    Warn,
    Fail,
}

data class CheckItem(val label: String, val state: CheckState, val value: String)

class SelfCheckReport(val items: List<CheckItem>) {
    /** 用户确认的就是这几项,之后新出现的失败要重新确认. */
    val failures: Set<CheckItem>
        get() = items.filter { it.state == CheckState.Fail }.toSet()

    val failed: Int
        get() = failures.size

    /** 不用确认,但签名之前也要让用户看到. */
    val warnings: List<CheckItem>
        get() = items.filter { it.state == CheckState.Warn }

    val warned: Int
        get() = warnings.size

    /** 有未通过项时不能创建钱包.已有钱包时每次操作都要用户确认. */
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

/** 判定规则,按顺序给出每一项的结果. */
fun evaluate(f: DeviceFacts): SelfCheckReport =
    SelfCheckReport(
        listOf(
            radios(f),
            rootAndBoot(f),
            debugAndUsb(f),
            keystore(f),
            item(
                "系统屏幕锁",
                problem = "未设置".takeIf { f.screenLockSet == false },
                unknown = UNCONFIRMED.takeIf { f.screenLockSet == null },
                pass = "已设置",
            ),
            item(
                "无障碍服务",
                problem = f.thirdPartyAccessibility?.takeIf { it > 0 }?.let { "$it 个第三方" },
                unknown = UNCONFIRMED.takeIf { f.thirdPartyAccessibility == null },
                pass = "无第三方",
            ),
            CheckItem(
                "悬浮窗应用",
                if (f.overlayApps == 0) CheckState.Pass else CheckState.Warn,
                when (f.overlayApps) {
                    null -> UNCONFIRMED
                    0 -> "无授权"
                    else -> "${f.overlayApps} 个应用"
                },
            ),
        )
    )

private const val UNCONFIRMED = "无法确认"

/** 确定的问题优先.没有问题但有没读出来的项时是警告,读不出不等于有问题. */
private fun item(label: String, problem: String?, unknown: String?, pass: String): CheckItem =
    when {
        problem != null -> CheckItem(label, CheckState.Fail, problem)
        unknown != null -> CheckItem(label, CheckState.Warn, unknown)
        else -> CheckItem(label, CheckState.Pass, pass)
    }

private fun radios(f: DeviceFacts): CheckItem =
    item(
        "飞行模式和无线",
        problem =
            when {
                f.wifiOn == true -> "Wi-Fi 已开启"
                f.bluetoothOn == true -> "蓝牙已开启"
                f.nfcOn == true -> "NFC 已开启"
                f.airplaneMode == false -> "飞行模式未开"
                else -> null
            },
        unknown = UNCONFIRMED.takeIf { null in listOf(f.wifiOn, f.bluetoothOn, f.nfcOn, f.airplaneMode) },
        pass = "已隔离",
    )

private fun rootAndBoot(f: DeviceFacts): CheckItem =
    item(
        "Root 和 Bootloader",
        problem =
            when {
                f.rooted == true -> "检测到 Root"
                f.bootloaderLocked == false -> "Bootloader 未锁定"
                else -> null
            },
        unknown =
            when {
                f.bootloaderLocked == null -> "无法确认 Bootloader"
                f.rooted == null -> "无法确认 Root"
                else -> null
            },
        pass = "未 Root,已锁定",
    )

private fun debugAndUsb(f: DeviceFacts): CheckItem =
    item(
        "调试和 USB",
        problem =
            when {
                f.adbEnabled == true -> "USB 调试已开启"
                f.usbDataConnected == true -> "USB 已连接数据"
                else -> null
            },
        unknown = UNCONFIRMED.takeIf { f.adbEnabled == null || f.usbDataConnected == null },
        pass = "已关闭",
    )

private fun keystore(f: DeviceFacts): CheckItem =
    when (f.keystore) {
        HardwareLevel.StrongBox -> CheckItem("硬件密钥库", CheckState.Pass, "StrongBox")
        HardwareLevel.Tee -> CheckItem("硬件密钥库", CheckState.Warn, "TEE")
        HardwareLevel.None -> CheckItem("硬件密钥库", CheckState.Fail, "不可用")
        null -> CheckItem("硬件密钥库", CheckState.Warn, UNCONFIRMED)
    }

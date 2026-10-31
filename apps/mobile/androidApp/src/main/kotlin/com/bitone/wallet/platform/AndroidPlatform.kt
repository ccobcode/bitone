package com.bitone.wallet.platform

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.SystemClock
import android.provider.Settings
import android.view.WindowManager
import java.security.MessageDigest

/** 开机次数加开机后的单调时钟。 */
class AndroidBootClock(private val context: Context) : BootClock {
    override fun now() =
        ClockReading(
            bootId = Settings.Global.getInt(context.contentResolver, Settings.Global.BOOT_COUNT, 0).toLong(),
            elapsedMs = SystemClock.elapsedRealtime(),
        )
}

/** 当前 APK 签名证书的 SHA-256，每 4 个十六进制字符一组，和发布页的写法一致。 */
fun signingCertificate(context: Context): String {
    val info = context.packageManager.getPackageInfo(context.packageName, PackageManager.GET_SIGNING_CERTIFICATES)
    val signer = info.signingInfo?.apkContentsSigners?.firstOrNull() ?: return ""
    val digest = MessageDigest.getInstance("SHA-256").digest(signer.toByteArray())
    return digest.joinToString("") { "%02X".format(it) }.chunked(4).joinToString(" ")
}

/** 当前 Activity，窗口相关的实现都从这里取。 */
class CurrentActivity {
    var activity: Activity? = null
}

/** FLAG_SECURE 和隐藏悬浮窗。全应用只有一个窗口，弹层都画在同一窗口里，所以不用 Dialog。 */
class AndroidScreenGuard(private val current: CurrentActivity) : ScreenGuard {
    override fun protect() {
        val window = current.activity?.window ?: return
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        window.setHideOverlayWindows(true)
    }
}

class AndroidBrightness(private val current: CurrentActivity) : Brightness {
    override fun maximize() = apply(WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_FULL, keepOn = true)

    override fun restore() = apply(WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE, keepOn = false)

    private fun apply(level: Float, keepOn: Boolean) {
        val window = current.activity?.window ?: return
        window.attributes = window.attributes.apply { screenBrightness = level }
        if (keepOn) {
            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }
}

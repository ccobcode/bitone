package com.ccobcode.bitone

import android.app.Application
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.AndroidBootClock
import com.ccobcode.bitone.platform.AndroidBrightness
import com.ccobcode.bitone.platform.AndroidQrScanner
import com.ccobcode.bitone.platform.AndroidScreenGuard
import com.ccobcode.bitone.platform.AndroidSelfCheck
import com.ccobcode.bitone.platform.CurrentActivity
import com.ccobcode.bitone.platform.FileSecureStore
import com.ccobcode.bitone.platform.KeystoreHardwareKey
import com.ccobcode.bitone.platform.Platform
import com.ccobcode.bitone.platform.UniffiWalletCore
import com.ccobcode.bitone.platform.signingCertificate
import java.util.Locale
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

/** 流程状态放在进程级，Activity 重建时助记词草稿不丢，也不用进 Bundle。 */
class BitOneApplication : Application() {
    val current = CurrentActivity()

    val platform: Platform by lazy {
        Platform(
            core = UniffiWalletCore(),
            hardwareKey = KeystoreHardwareKey(this) { current.activity },
            selfCheck = AndroidSelfCheck(this),
            screenGuard = AndroidScreenGuard(current),
            brightness = AndroidBrightness(current),
            store = FileSecureStore(this),
            clock = AndroidBootClock(this),
            scanner = AndroidQrScanner(this, current),
            systemLanguage = Locale.getDefault().language,
            appVersion = packageManager.getPackageInfo(packageName, 0).versionName.orEmpty(),
            certificate = signingCertificate(this),
        )
    }

    val flow: WalletFlow by lazy {
        WalletFlow(platform, CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate), Dispatchers.Default)
    }
}

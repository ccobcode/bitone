package com.bitone.wallet

import android.app.Application
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.platform.AndroidBootClock
import com.bitone.wallet.platform.AndroidBrightness
import com.bitone.wallet.platform.AndroidQrScanner
import com.bitone.wallet.platform.AndroidScreenGuard
import com.bitone.wallet.platform.AndroidSelfCheck
import com.bitone.wallet.platform.CurrentActivity
import com.bitone.wallet.platform.FileSecureStore
import com.bitone.wallet.platform.KeystoreHardwareKey
import com.bitone.wallet.platform.Platform
import com.bitone.wallet.platform.UniffiWalletCore
import com.bitone.wallet.platform.signingCertificate
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

package com.ccobcode.bitone.platform

import android.Manifest
import android.app.KeyguardManager
import android.bluetooth.BluetoothManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.nfc.NfcAdapter
import android.os.BatteryManager
import android.os.Build
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.GeneralSecurityException
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.ProviderException
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

private const val ANDROID_KEYSTORE = "AndroidKeyStore"
private const val PROBE_ALIAS = "bitone.probe"
private const val ATTEST_ALIAS = "bitone.attest"
private const val USB_STATE_ACTION = "android.hardware.usb.action.USB_STATE"

private val USB_DATA_FUNCTIONS = listOf("mtp", "ptp", "rndis", "adb", "midi", "accessory", "ncm", "uvc", "audio_source")
private val ROOT_PACKAGES =
    listOf("com.topjohnwu.magisk", "eu.chainfire.supersu", "me.weishu.kernelsu", "org.lsposed.manager")
private val SU_DIRECTORIES =
    listOf(
        "/system/bin",
        "/system/xbin",
        "/sbin",
        "/vendor/bin",
        "/system/sbin",
        "/su/bin",
        "/data/local/bin",
        "/data/local/xbin",
    )

/** 用系统 API 采集 7 项状态.判定规则在共享层. */
class AndroidSelfCheck(private val context: Context) : SelfCheck {

    // 这两项探测要生成密钥,比较慢,而且一次开机内不会变.
    private val keystoreLevel: HardwareLevel by lazy { probeKeystore() }
    private val bootloaderLocked: Boolean? by lazy { attestBootloader() }

    override suspend fun collect(): DeviceFacts {
        val facts =
            DeviceFacts(
                airplaneMode = globalFlag(Settings.Global.AIRPLANE_MODE_ON),
                wifiOn = wifiOn(),
                bluetoothOn = bluetoothOn(),
                nfcOn = NfcAdapter.getDefaultAdapter(context)?.isEnabled == true,
                rooted = rooted(),
                bootloaderLocked = bootloaderLocked,
                adbEnabled = globalFlag(Settings.Global.ADB_ENABLED),
                usbDataConnected = usbDataConnected(),
                keystore = keystoreLevel,
                screenLockSet = context.getSystemService(KeyguardManager::class.java).isDeviceSecure,
                thirdPartyAccessibility = thirdPartyAccessibilityServices(),
                overlayApps = thirdPartyOverlayApps(),
            )
        return adjustFactsForDevelopment(context, facts)
    }

    private fun globalFlag(name: String): Boolean = Settings.Global.getInt(context.contentResolver, name, 0) != 0

    private fun wifiOn(): Boolean = context.getSystemService(WifiManager::class.java)?.isWifiEnabled == true

    /** Android 12 起 isEnabled 不需要蓝牙权限. */
    private fun bluetoothOn(): Boolean =
        context.getSystemService(BluetoothManager::class.java)?.adapter?.isEnabled == true

    /** Root 可以被隐藏,这里只能发现常见痕迹. */
    private fun rooted(): Boolean {
        if (Build.TAGS?.contains("test-keys") == true) return true
        val path = System.getenv("PATH").orEmpty().split(':').filter { it.isNotBlank() }
        if ((path + SU_DIRECTORIES).any { File(it, "su").exists() }) return true
        return ROOT_PACKAGES.any { installed(it) }
    }

    private fun installed(packageName: String): Boolean =
        try {
            context.packageManager.getApplicationInfo(packageName, 0)
            true
        } catch (_: PackageManager.NameNotFoundException) {
            false
        }

    /** 用硬件密钥证明读 deviceLocked,这是 TEE 报告的,比系统属性可信.只验到这一层,没有校验证书链到 Google 根证书,Root 过的系统能伪造结果. */
    private fun attestBootloader(): Boolean? =
        try {
            val spec =
                KeyGenParameterSpec.Builder(ATTEST_ALIAS, KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .setAttestationChallenge("bitone-self-check".encodeToByteArray())
                    .build()
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, ANDROID_KEYSTORE)
                .apply { initialize(spec) }
                .generateKeyPair()
            val store = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
            val leaf = store.getCertificateChain(ATTEST_ALIAS)?.firstOrNull() as? X509Certificate
            val extension = leaf?.getExtensionValue(KeyAttestation.OID)
            store.deleteEntry(ATTEST_ALIAS)
            extension?.let(KeyAttestation::deviceLocked)
        } catch (_: GeneralSecurityException) {
            null
        } catch (_: ProviderException) {
            null
        } catch (_: java.io.IOException) {
            null
        }

    private fun usbDataConnected(): Boolean {
        val state = context.registerReceiver(null, IntentFilter(USB_STATE_ACTION), Context.RECEIVER_NOT_EXPORTED)
        if (state != null) {
            return state.getBooleanExtra("connected", false) &&
                USB_DATA_FUNCTIONS.any { state.getBooleanExtra(it, false) }
        }
        // 没有粘性广播时退到电池状态,插 USB 就按连接数据处理.
        val battery: Intent? =
            context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED), Context.RECEIVER_NOT_EXPORTED)
        return battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) == BatteryManager.BATTERY_PLUGGED_USB
    }

    /** StrongBox 优先.只认 KeyInfo 报告的实际级别,功能声明可能不准. */
    private fun probeKeystore(): HardwareLevel {
        val store = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        val hasStrongBox = context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
        val attempts = if (hasStrongBox) listOf(true, false) else listOf(false)
        for (strongBox in attempts) {
            val level =
                try {
                    KeystoreHardwareKey.keyLevel(probeKey(strongBox))
                } catch (_: GeneralSecurityException) {
                    continue
                } catch (_: ProviderException) {
                    continue
                } finally {
                    store.deleteEntry(PROBE_ALIAS)
                }
            return level
        }
        return HardwareLevel.None
    }

    private fun probeKey(strongBox: Boolean): SecretKey {
        val spec =
            KeyGenParameterSpec.Builder(PROBE_ALIAS, KeyProperties.PURPOSE_ENCRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setIsStrongBoxBacked(strongBox)
                .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
            .apply { init(spec) }
            .generateKey()
    }

    /** 读系统设置而不是 AccessibilityManager,后者的结果受包可见性过滤,可能漏掉服务. */
    private fun thirdPartyAccessibilityServices(): Int {
        val resolver = context.contentResolver
        if (Settings.Secure.getInt(resolver, Settings.Secure.ACCESSIBILITY_ENABLED, 0) == 0) return 0
        val enabled = Settings.Secure.getString(resolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES).orEmpty()
        return enabled
            .split(':')
            .mapNotNull { ComponentName.unflattenFromString(it)?.packageName }
            .distinct()
            .count { !isSystemApp(it) }
    }

    private fun isSystemApp(packageName: String): Boolean =
        try {
            context.packageManager.getApplicationInfo(packageName, 0).flags and ApplicationInfo.FLAG_SYSTEM != 0
        } catch (_: PackageManager.NameNotFoundException) {
            false
        }

    /** 第三方应用查不到别人的悬浮窗授权(AppOps 需要系统权限),所以统计声明了这个权限的第三方应用.比"已授权"更严,只影响警告,不锁定. */
    private fun thirdPartyOverlayApps(): Int =
        context.packageManager
            .getInstalledPackages(PackageManager.PackageInfoFlags.of(PackageManager.GET_PERMISSIONS.toLong()))
            .count { info ->
                val system = (info.applicationInfo?.flags ?: 0) and ApplicationInfo.FLAG_SYSTEM != 0
                !system &&
                    info.packageName != context.packageName &&
                    info.requestedPermissions?.contains(Manifest.permission.SYSTEM_ALERT_WINDOW) == true
            }
}

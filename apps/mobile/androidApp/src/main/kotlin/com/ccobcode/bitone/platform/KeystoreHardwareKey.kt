package com.ccobcode.bitone.platform

import android.content.Context
import android.content.pm.PackageManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricManager.Authenticators
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import com.ccobcode.bitone.i18n.translate
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.security.ProviderException
import java.security.UnrecoverableKeyException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.Mac
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext

private const val ANDROID_KEYSTORE = "AndroidKeyStore"
private const val PIN_ALIAS = "bitone.pin"
private const val HMAC = "HmacSHA256"
/** 旧格式外层加密的密钥,迁移后删除. */
private const val LEGACY_ALIAS = "bitone.seal"
private const val PROBE_ALIAS = "bitone.enrollment"
private const val TRANSFORMATION = "AES/GCM/NoPadding"
private const val IV_BYTES = 12
private const val TAG_BITS = 128

/** 0 表示每次使用都要认证,没有时间窗. */
private const val AUTH_EVERY_USE = 0

private val AAD = "bitone-vault-v1".encodeToByteArray()

/**
 * Keystore 里不可导出的 HMAC-SHA256 密钥,优先 StrongBox,其次 TEE,都没有时拒绝创建.PIN 要经过它才能派生出解密密钥.
 *
 * 认证方式里有屏幕锁凭据,增删指纹和修改屏幕锁时密钥不失效,指纹变更靠单独的探针发现.移除屏幕锁时永久失效,对应"需恢复".
 *
 * setUnlockedDeviceRequired 只在 Android 15 及以上设置.12 到 14 上这类密钥依赖屏幕锁凭据,移除屏幕锁时会被删除.
 */
class KeystoreHardwareKey(
    private val context: Context,
    private val activityContext: () -> Context?,
) : HardwareKey {

    private fun keyStore(): KeyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }

    private fun existingKey(alias: String): SecretKey? =
        try {
            keyStore().getKey(alias, null) as? SecretKey
        } catch (_: UnrecoverableKeyException) {
            null
        }

    override suspend fun bind(pin: String): ByteArray {
        val key = existingKey(PIN_ALIAS) ?: throw HardwareKeyException.Lost("硬件密钥已不存在")
        return authenticatedMac(key, pin, HardwareKeyException.Lost("硬件密钥已失效"))
    }

    override suspend fun enroll(pin: String): ByteArray {
        val key = existingKey(PIN_ALIAS) ?: createKey()
        return authenticatedMac(key, pin, HardwareKeyException.Unavailable("硬件密钥不可用"))
    }

    /** 这次认证只对这一个 Mac 对象有效,所以一次确认只能算一个 PIN. */
    private suspend fun authenticatedMac(key: SecretKey, pin: String, failure: HardwareKeyException): ByteArray =
        rethrowing(failure) {
            val mac = Mac.getInstance(HMAC)
            mac.init(key)
            confirmCredential(BiometricPrompt.CryptoObject(mac))
            val bytes = pin.encodeToByteArray()
            try {
                mac.doFinal(bytes)
            } finally {
                bytes.fill(0)
            }
        }

    override suspend fun unwrap(wrapped: ByteArray): ByteArray {
        val key = existingKey(LEGACY_ALIAS) ?: throw HardwareKeyException.Lost("硬件密钥已不存在")
        if (wrapped.size <= IV_BYTES) throw HardwareKeyException.Lost("密文损坏")
        return authorized(HardwareKeyException.Lost("硬件密钥已失效")) {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, wrapped, 0, IV_BYTES))
            cipher.updateAAD(AAD)
            cipher.doFinal(wrapped, IV_BYTES, wrapped.size - IV_BYTES)
        }
    }

    override fun isLost(): Boolean =
        try {
            val current = existingKey(PIN_ALIAS)
            val legacy = existingKey(LEGACY_ALIAS)
            when {
                // 初始化会占用一个硬件操作,StrongBox 能同时跑的操作很少,探测完就释放.
                current != null -> Mac.getInstance(HMAC).apply { init(current) }.reset()
                legacy != null ->
                    Cipher.getInstance(TRANSFORMATION)
                        .init(Cipher.DECRYPT_MODE, legacy, GCMParameterSpec(TAG_BITS, ByteArray(IV_BYTES)))
                else -> return true
            }
            false
        } catch (_: UserNotAuthenticatedException) {
            false
        } catch (_: GeneralSecurityException) {
            // 含 KeyPermanentlyInvalidatedException.其他错误也按失效处理,不删任何数据,下次检测恢复后自动回首页.
            true
        } catch (_: ProviderException) {
            true
        }

    override fun delete() {
        keyStore().deleteEntry(PIN_ALIAS)
        keyStore().deleteEntry(LEGACY_ALIAS)
        keyStore().deleteEntry(PROBE_ALIAS)
    }

    override fun dropLegacy() {
        keyStore().deleteEntry(LEGACY_ALIAS)
    }

    override suspend fun confirmDeviceLock() = confirmCredential()

    /**
     * 硬件密钥带时间窗,指纹变更不会让它失效,所以另放一把只用来探测的密钥.它绑定当前录入的指纹,新增或删光指纹时系统让它永久失效.
     *
     * 只有系统明确报失效才算变更,其他错误不算.ponytail: 从没录过指纹的手机第一次录入时才建探针,这一次变更看不出来.
     */
    override fun lockChanged(): Boolean =
        try {
            val probe = keyStore().getKey(PROBE_ALIAS, null) as? SecretKey
            if (probe == null) createProbe() else Cipher.getInstance(TRANSFORMATION).init(Cipher.ENCRYPT_MODE, probe)
            false
        } catch (_: KeyPermanentlyInvalidatedException) {
            true
        } catch (_: GeneralSecurityException) {
            false
        } catch (_: ProviderException) {
            false
        }

    override fun acceptLock() {
        keyStore().deleteEntry(PROBE_ALIAS)
    }

    private fun createProbe() {
        val biometrics = context.getSystemService(BiometricManager::class.java)
        if (biometrics.canAuthenticate(Authenticators.BIOMETRIC_STRONG) != BiometricManager.BIOMETRIC_SUCCESS) return
        val spec =
            KeyGenParameterSpec.Builder(PROBE_ALIAS, KeyProperties.PURPOSE_ENCRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(true)
                .build()
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE).apply { init(spec) }.generateKey()
    }

    private fun createKey(): SecretKey {
        val preferStrongBox = context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
        val key =
            try {
                try {
                    generate(strongBox = preferStrongBox)
                } catch (e: ProviderException) {
                    // StrongBoxUnavailableException 是 ProviderException 的子类.
                    if (preferStrongBox) generate(strongBox = false) else throw e
                }
            } catch (e: GeneralSecurityException) {
                throw HardwareKeyException.Unavailable(e.javaClass.simpleName)
            } catch (e: ProviderException) {
                throw HardwareKeyException.Unavailable(e.javaClass.simpleName)
            }
        // 没有安全硬件时 Keystore 会悄悄退回软件实现,必须事后确认.
        if (keyLevel(key) == HardwareLevel.None && !softwareKeystoreAllowed(context)) {
            delete()
            throw HardwareKeyException.Unavailable("只有软件密钥库")
        }
        return key
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val spec =
            KeyGenParameterSpec.Builder(PIN_ALIAS, KeyProperties.PURPOSE_SIGN)
                .setKeySize(256)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(
                    AUTH_EVERY_USE,
                    KeyProperties.AUTH_DEVICE_CREDENTIAL or KeyProperties.AUTH_BIOMETRIC_STRONG,
                )
                .setIsStrongBoxBacked(strongBox)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            spec.setUnlockedDeviceRequired(true)
        }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_HMAC_SHA256, ANDROID_KEYSTORE)
            .apply { init(spec.build()) }
            .generateKey()
    }

    /** 旧格式的时间窗过期时弹一次系统屏幕锁再重试. */
    private suspend fun <T> authorized(failure: HardwareKeyException, block: () -> T): T =
        rethrowing(failure) {
            try {
                block()
            } catch (_: UserNotAuthenticatedException) {
                confirmCredential()
                block()
            }
        }

    /** 密钥失效统一换成 [failure]. */
    private suspend fun <T> rethrowing(failure: HardwareKeyException, block: suspend () -> T): T =
        try {
            block()
        } catch (e: HardwareKeyException) {
            throw e
        } catch (_: KeyPermanentlyInvalidatedException) {
            throw HardwareKeyException.Lost("硬件密钥已失效")
        } catch (_: GeneralSecurityException) {
            throw failure
        } catch (_: ProviderException) {
            throw failure
        }

    private suspend fun confirmCredential(crypto: BiometricPrompt.CryptoObject? = null) =
        withContext(Dispatchers.Main) {
            suspendCancellableCoroutine { continuation ->
                val signal = CancellationSignal()
                continuation.invokeOnCancellation { signal.cancel() }
                val callback =
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                            continuation.resume(Unit)
                        }

                        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                            continuation.resumeWithException(HardwareKeyException.AuthCancelled())
                        }
                    }
                val prompt =
                    BiometricPrompt.Builder(activityContext() ?: context)
                        .setTitle(translate("确认屏幕锁"))
                        .setAllowedAuthenticators(Authenticators.DEVICE_CREDENTIAL or Authenticators.BIOMETRIC_STRONG)
                        .build()
                if (crypto == null) {
                    prompt.authenticate(signal, context.mainExecutor, callback)
                } else {
                    prompt.authenticate(crypto, signal, context.mainExecutor, callback)
                }
            }
        }

    companion object {
        /** 只看密钥实际落在哪,不信功能声明. */
        fun keyLevel(key: SecretKey): HardwareLevel {
            val info =
                SecretKeyFactory.getInstance(key.algorithm, ANDROID_KEYSTORE).getKeySpec(key, KeyInfo::class.java)
                    as KeyInfo
            return when (info.securityLevel) {
                KeyProperties.SECURITY_LEVEL_STRONGBOX -> HardwareLevel.StrongBox
                KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT,
                KeyProperties.SECURITY_LEVEL_UNKNOWN_SECURE -> HardwareLevel.Tee
                else -> HardwareLevel.None
            }
        }
    }
}

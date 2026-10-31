package com.bitone.wallet.platform

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
import com.bitone.wallet.i18n.translate
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.security.ProviderException
import java.security.UnrecoverableKeyException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext

private const val ANDROID_KEYSTORE = "AndroidKeyStore"
private const val ALIAS = "bitone.seal"
private const val PROBE_ALIAS = "bitone.enrollment"
private const val TRANSFORMATION = "AES/GCM/NoPadding"
private const val IV_BYTES = 12
private const val TAG_BITS = 128

/** 认证时间窗。解锁手机后这么久内不用再确认，超过时弹系统屏幕锁。 */
private const val AUTH_WINDOW_SECONDS = 300

private val AAD = "bitone-vault-v1".encodeToByteArray()

/**
 * 第二层加密：Keystore 里不可导出的 AES-256-GCM 密钥，优先 StrongBox，其次 TEE，都没有时拒绝创建。
 *
 * 绑定设备解锁状态的做法（docs/03 §11 留的问题）：
 *
 * 不把 setUnlockedDeviceRequired 作为唯一依赖。Android 12 到 14 上这类密钥依赖屏幕锁凭据， 没有屏幕锁时不能生成和使用，移除屏幕锁时会被删除。Android 15 才修好，官方建议只在 15
 * 及以上使用。
 *
 * 所以分两层：
 * 1. 所有版本都用 setUserAuthenticationRequired 加时间窗。这是硬件强制的，超过时间窗必须重新认证。 移除屏幕锁时密钥永久失效，这是文档写明的行为，正好对应「需恢复」。
 *    没有屏幕锁时生成会失败，也正好对应「拒绝创建钱包」。
 * 2. Android 15 及以上再加 setUnlockedDeviceRequired，手机一锁屏密钥立刻不可用。
 *
 * 残余差异：Android 12 到 14 上，时间窗内手机被锁屏时密钥仍然可用，此时种子还有 PIN 这一层保护。
 */
class KeystoreHardwareKey(
    private val context: Context,
    private val activityContext: () -> Context?,
) : HardwareKey {

    private fun keyStore(): KeyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }

    private fun existingKey(): SecretKey? =
        try {
            keyStore().getKey(ALIAS, null) as? SecretKey
        } catch (_: UnrecoverableKeyException) {
            null
        }

    override suspend fun wrap(plain: ByteArray): ByteArray {
        val key = existingKey() ?: createKey()
        return authorized(HardwareKeyException.Unavailable("硬件密钥不可用")) {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, key)
            cipher.updateAAD(AAD)
            val body = cipher.doFinal(plain)
            cipher.iv + body
        }
    }

    override suspend fun unwrap(wrapped: ByteArray): ByteArray {
        val key = existingKey() ?: throw HardwareKeyException.Lost("硬件密钥已不存在")
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
            val key = existingKey()
            if (key == null) {
                true
            } else {
                Cipher.getInstance(TRANSFORMATION)
                    .init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, ByteArray(IV_BYTES)))
                false
            }
        } catch (_: UserNotAuthenticatedException) {
            false
        } catch (_: GeneralSecurityException) {
            // 含 KeyPermanentlyInvalidatedException。其他错误也按失效处理，不删任何数据，下次检测恢复后自动回首页。
            true
        } catch (_: ProviderException) {
            true
        }

    override fun delete() {
        keyStore().deleteEntry(ALIAS)
        keyStore().deleteEntry(PROBE_ALIAS)
    }

    override suspend fun confirmDeviceLock() = confirmCredential()

    /**
     * 硬件密钥带时间窗，指纹变更不会让它失效，所以另放一把只用来探测的密钥。它绑定当前录入的指纹，新增或删光指纹时系统让它永久失效。
     *
     * 只有系统明确报失效才算变更，其他错误不算。ponytail: 从没录过指纹的手机第一次录入时才建探针，这一次变更看不出来。
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
                    // StrongBoxUnavailableException 是 ProviderException 的子类。
                    if (preferStrongBox) generate(strongBox = false) else throw e
                }
            } catch (e: GeneralSecurityException) {
                throw HardwareKeyException.Unavailable(e.javaClass.simpleName)
            } catch (e: ProviderException) {
                throw HardwareKeyException.Unavailable(e.javaClass.simpleName)
            }
        // 没有安全硬件时 Keystore 会悄悄退回软件实现，必须事后确认。
        if (keyLevel(key) == HardwareLevel.None && !softwareKeystoreAllowed(context)) {
            delete()
            throw HardwareKeyException.Unavailable("只有软件密钥库")
        }
        return key
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val spec =
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(
                    AUTH_WINDOW_SECONDS,
                    KeyProperties.AUTH_DEVICE_CREDENTIAL or KeyProperties.AUTH_BIOMETRIC_STRONG,
                )
                .setIsStrongBoxBacked(strongBox)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            spec.setUnlockedDeviceRequired(true)
        }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
            .apply { init(spec.build()) }
            .generateKey()
    }

    /** 时间窗过期时弹一次系统屏幕锁再重试。密钥失效统一换成 [failure]。 */
    private suspend fun <T> authorized(failure: HardwareKeyException, block: () -> T): T =
        try {
            try {
                block()
            } catch (_: UserNotAuthenticatedException) {
                confirmCredential()
                block()
            }
        } catch (e: HardwareKeyException) {
            throw e
        } catch (_: KeyPermanentlyInvalidatedException) {
            throw HardwareKeyException.Lost("硬件密钥已失效")
        } catch (_: GeneralSecurityException) {
            throw failure
        } catch (_: ProviderException) {
            throw failure
        }

    private suspend fun confirmCredential() =
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
                BiometricPrompt.Builder(activityContext() ?: context)
                    .setTitle(translate("确认屏幕锁"))
                    .setAllowedAuthenticators(Authenticators.DEVICE_CREDENTIAL or Authenticators.BIOMETRIC_STRONG)
                    .build()
                    .authenticate(signal, context.mainExecutor, callback)
            }
        }

    companion object {
        /** 只看密钥实际落在哪，不信功能声明。 */
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

package com.bitone.wallet.platform

enum class HardwareLevel {
    StrongBox,
    Tee,
    None,
}

sealed class HardwareKeyException(message: String) : Exception(message) {
    /** 设备没有硬件密钥库，或系统屏幕锁未设置。拒绝创建钱包。 */
    class Unavailable(message: String) : HardwareKeyException(message)

    /** 密钥被系统删除或永久失效，只能用助记词恢复。 */
    class Lost(message: String) : HardwareKeyException(message)

    /** 用户取消了系统屏幕锁确认。不算 PIN 错误。 */
    class AuthCancelled : HardwareKeyException("用户取消确认")
}

/** 第二层加密。密钥不可导出，只能在硬件里用。 */
interface HardwareKey {
    /** 用现有密钥包裹，没有密钥时先创建。 */
    suspend fun wrap(plain: ByteArray): ByteArray

    suspend fun unwrap(wrapped: ByteArray): ByteArray

    /** 钱包存在时用来判断是否进入「需恢复」。 */
    fun isLost(): Boolean

    fun delete()

    /** 弹一次系统屏幕锁，不看时间窗。用户取消时抛 [HardwareKeyException.AuthCancelled]。 */
    suspend fun confirmDeviceLock()

    /** 指纹被更换或关闭之后是 true，直到 [acceptLock]。 */
    fun lockChanged(): Boolean

    /** 输对 PIN 证明了身份，以当前的指纹为准。 */
    fun acceptLock()
}

package com.ccobcode.bitone.platform

enum class HardwareLevel {
    StrongBox,
    Tee,
    None,
}

sealed class HardwareKeyException(message: String) : Exception(message) {
    /** 设备没有硬件密钥库,或系统屏幕锁未设置.拒绝创建钱包. */
    class Unavailable(message: String) : HardwareKeyException(message)

    /** 密钥被系统删除或永久失效,只能用助记词恢复. */
    class Lost(message: String) : HardwareKeyException(message)

    /** 用户取消了系统屏幕锁确认.不算 PIN 错误. */
    class AuthCancelled : HardwareKeyException("用户取消确认")
}

/** 硬件里的 HMAC 密钥,不可导出.PIN 要经过它才能派生出解密密钥,所以猜 PIN 离不开这台手机. */
interface HardwareKey {
    /** 硬件对这个 PIN 算出的结果.每次都弹系统屏幕锁,用户取消时抛 [HardwareKeyException.AuthCancelled]. */
    suspend fun bind(pin: String): ByteArray

    /** 和 [bind] 一样,没有密钥时先创建.只在建钱包和迁移时用. */
    suspend fun enroll(pin: String): ByteArray

    /** 去掉旧格式外面的硬件加密,只用于迁移. */
    suspend fun unwrap(wrapped: ByteArray): ByteArray

    /** 钱包存在时用来判断是否进入"需恢复". */
    fun isLost(): Boolean

    fun delete()

    /** 迁移完成后删掉旧格式的密钥. */
    fun dropLegacy()

    /** 弹一次系统屏幕锁,不看时间窗.只有旧格式的钱包要用. */
    suspend fun confirmDeviceLock()

    /** 指纹被更换或关闭之后是 true,直到 [acceptLock]. */
    fun lockChanged(): Boolean

    /** 输对 PIN 证明了身份,以当前的指纹为准. */
    fun acceptLock()
}

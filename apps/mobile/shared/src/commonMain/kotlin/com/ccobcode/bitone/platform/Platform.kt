package com.ccobcode.bitone.platform

/** 共享层需要的全部平台能力,启动时注入. */
class Platform(
    val core: WalletCore,
    val hardwareKey: HardwareKey,
    val selfCheck: SelfCheck,
    val screenGuard: ScreenGuard,
    val brightness: Brightness,
    val store: SecureStore,
    val clock: BootClock,
    val scanner: QrScanner,
    val appVersion: String,
    /** 系统语言,比如 `zh`,`en`. */
    val systemLanguage: String,
    /** 签名证书的 SHA-256,方便断网前核对,见 docs/04 §2.10. */
    val certificate: String,
)

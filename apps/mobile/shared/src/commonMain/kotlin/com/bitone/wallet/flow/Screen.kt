package com.bitone.wallet.flow

/** 页面编号与原型一致。 */
sealed interface Screen {
    /** 启动时等待第一次自检。 */
    data object Booting : Screen

    /** A1 */
    data object Welcome : Screen

    /** A1b */
    data object ImportWords : Screen

    /** A2 */
    data object Backup : Screen

    /** A2b */
    data object Verify : Screen

    /** A2c */
    data object SetPin : Screen

    /** A3 */
    data object Home : Screen

    /** A7，按 [UnlockPurpose] 用于签名、导出、读公钥、查看助记词和修改 PIN。 */
    data object Unlock : Screen

    /** A4 */
    data object Export : Screen

    /** A5 */
    data object Scan : Screen

    /** A6 到 A6e，按 [reviewStyle] 切换样式。 */
    data object Review : Screen

    /** A8 */
    data object Signed : Screen

    /** A9 */
    data object CheckAddress : Screen

    /** A10 的查看助记词。 */
    data object ViewWords : Screen

    /** A10 */
    data object Settings : Screen

    /** A11 */
    data object Receive : Screen

    data object ReceiveAddresses : Screen

    /** A12 */
    data object Manifest : Screen

    /** AS 自检失败 */
    data object SelfCheckFailed : Screen

    /** AS 指纹不匹配 */
    data object FingerprintMismatch : Screen

    /** AS 解析失败 */
    data object ParseFailed : Screen

    /** AS 禁止签名 */
    data object Forbidden : Screen

    /** AS 摄像头被拒 */
    data object CameraDenied : Screen

    /** AS 需恢复 */
    data object NeedsRecovery : Screen

    /** 错误 10 次后 */
    data object Wiped : Screen
}

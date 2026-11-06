package com.ccobcode.bitone.flow

/** 注释里是页面编号,其他代码和测试也用编号指代页面. */
enum class Screen {
    /** 启动时等待第一次自检. */
    Booting,
    /** A1 */
    Welcome,
    /** A1b */
    ImportWords,
    /** A2 */
    Backup,
    /** A2b */
    Verify,
    /** A2d,启用 BIP39 密码时在 A2c 之前. */
    Passphrase,
    /** A2c */
    SetPin,
    /** A3 */
    Home,
    /** A7,按 [UnlockPurpose] 用于签名,导出,读公钥,查看助记词和修改 PIN.旧格式的 BIP39 密码钱包先在这里输两遍密码. */
    Unlock,
    /** A4 */
    Export,
    /** A5 */
    Scan,
    /** A6 到 A6e,按 [reviewStyle] 切换样式. */
    Review,
    /** A8 */
    Signed,
    /** A9 */
    CheckAddress,
    /** A10 的查看助记词. */
    ViewWords,
    /** A10 */
    Settings,
    /** A11 */
    Receive,
    ReceiveAddresses,
    /** A12 */
    Manifest,
    /** AS 自检失败 */
    SelfCheckFailed,
    /** AS 指纹不匹配 */
    FingerprintMismatch,
    /** AS 解析失败 */
    ParseFailed,
    /** AS 禁止签名 */
    Forbidden,
    /** AS 摄像头被拒 */
    CameraDenied,
    /** AS 需恢复 */
    NeedsRecovery,
    /** AS 导入会覆盖本机钱包 */
    ImportOverwrite,
}

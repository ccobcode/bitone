package com.ccobcode.bitone.platform

/** 不含秘密的钱包元数据。 */
class WalletMeta(
    /** 导出时要额外带上旧路径账户，所以必须保存。 */
    val imported: Boolean,
    /** 只记是否启用，密码本身不落盘。 */
    val passphrase: Boolean,
    /** 启用 BIP39 密码时为 null，真正的指纹要解锁后才知道。 */
    val fingerprint: String?,
)

/** PIN 错误计数，[bootId] 和 [elapsedMs] 是最后一次失败的时刻。 */
class PinRecord(val failures: Int, val bootId: Long, val elapsedMs: Long) {
    companion object {
        val Clean = PinRecord(0, 0, 0)
    }
}

/** 应用私有存储。写操作都要在返回前落盘。 */
interface SecureStore {
    fun readVault(): ByteArray?

    fun readMeta(): WalletMeta?

    /** 写入新钱包：密文、元数据和公钥。PIN 计数、最高找零索引和收款索引都归零，旧钱包的公钥不留下。 BIP39 密码钱包的 [publicKeys] 传 null，公钥不落盘。 */
    fun saveWallet(vault: ByteArray, meta: WalletMeta, publicKeys: ByteArray?)

    /** 修改 PIN：只换密文，其余都不动。写入是原子的，中途断电时旧密文仍然完整。 */
    fun replaceVault(vault: ByteArray)

    /** 公钥缓存。核对交易不用解锁。 */
    fun readPublicKeys(): ByteArray?

    fun writePublicKeys(keys: ByteArray)

    fun readHighestChange(): ChangeIndexes

    /** [testnet] 选存储的槽，不是模式。 */
    fun writeHighestChange(testnet: Boolean, index: UInt)

    /** Bitcoin 账户当前的收款地址序号，按账户路径分开记。 */
    fun readReceiveIndex(accountPath: String): UInt

    fun writeReceiveIndex(accountPath: String, index: UInt)

    /** 扫码更新的清单包原文。读出后还要重新验签，本机文件也不信任。 */
    fun readManifest(): ByteArray?

    fun writeManifest(bytes: ByteArray)

    fun readPinRecord(): PinRecord

    fun writePinRecord(record: PinRecord)

    /** 用户选的语言，null 是自动，跟随系统。 */
    fun readLanguage(): String?

    fun writeLanguage(code: String?)

    /** 展示二维码时把屏幕调到最亮，默认开。 */
    fun readAutoBrightness(): Boolean

    fun writeAutoBrightness(on: Boolean)

    /** 删除钱包数据。公钥、清单和设置项不是秘密，一并清除。 */
    fun wipe()
}

package com.ccobcode.bitone.platform

/** Rust 核心的平台无关接口。助记词和种子留在 Rust 对象里，共享层只持有句柄。 所有方法都可能很慢（Argon2id），调用方要放到后台线程。 */
interface WalletCore {
    fun generateDraft(wordCount: Int): WalletDraft

    fun importDraft(words: List<String>): WalletDraft

    /** [tag] 是 [HardwareKey.bind] 的结果。 */
    fun open(sealed: ByteArray, pin: String, tag: ByteArray, passphrase: String): UnlockedWallet

    /** 解开旧格式，只用于迁移。[sealed] 是去掉硬件那层之后的内容。 */
    fun openLegacy(sealed: ByteArray, pin: String, passphrase: String): UnlockedWallet

    /** 本机密文是不是当前格式。不是时要先走迁移。 */
    fun isBoundVault(vault: ByteArray): Boolean

    fun qrMatrix(part: String): QrMatrix

    /** 收款码是纯文本，不是 UR 帧。大小写原样保留，二维码里的内容就是地址本身。 */
    fun qrText(text: String): QrMatrix

    /** 设置和修改 PIN 时用，规则在核心库。 */
    fun isWeakPin(pin: String): Boolean

    fun isValidWord(word: String): Boolean

    fun wordSuggestions(prefix: String, limit: Int): List<String>

    fun newReceiver(): FrameReceiver

    /** 不需要 PIN。解析失败和不可签名都体现在返回值里，不抛异常。 */
    fun reviewRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): ReviewResult

    fun reviewAddress(message: ByteArray, keys: ByteArray): AddressReview

    /** 从公钥缓存算出 A3 显示的地址，不需要 PIN。 */
    fun accountRows(keys: ByteArray): List<AccountAddress>

    /** 账户的第 [index] 个收款地址，不需要 PIN。只有 Bitcoin 的 [index] 可以大于 0。 */
    fun receiveAddress(keys: ByteArray, accountPath: String, index: UInt): String

    /** 校验清单包，返回版本号。版本不高于 [currentVersion] 时抛异常。 */
    fun verifyManifest(bytes: ByteArray, currentVersion: UInt): UInt

    /** [bytes] 为空时是内置清单的版本。 */
    fun manifestVersion(bytes: ByteArray?): UInt

    /** 清单里的全部代币，原生币也在内。[bytes] 为空时是内置清单。 */
    fun manifestEntries(bytes: ByteArray?): List<ManifestEntry>
}

/** 建钱包过程中的助记词。设置 PIN 之后就关闭。 */
interface WalletDraft {
    val imported: Boolean

    /** 只在 A2 显示时调用。字符串经过 JVM 后无法清除，不要长期持有。 */
    fun words(): List<String>

    fun quiz(rounds: Int): List<QuizItem>

    fun checkWord(index: Int, word: String): Boolean

    /** [tag] 是 [HardwareKey.enroll] 的结果。 */
    fun seal(pin: String, tag: ByteArray): ByteArray

    fun close()
}

/** 解锁后的钱包，关闭时清除种子。用完立刻关闭。 */
interface UnlockedWallet {
    fun fingerprint(): String

    fun exportAccounts(imported: Boolean): FrameStream

    /** 字符串经过 JVM 后无法清除，只在查看助记词时调用。 */
    fun words(): List<String>

    /** 用新 PIN 重新封一层，种子不离开 Rust。返回的密文平台层再用硬件密钥包一层。 */
    fun seal(pin: String, tag: ByteArray): ByteArray

    /** 全部账户的公钥，核对交易用。 */
    fun publicKeys(imported: Boolean): ByteArray

    /** 重新核对后签名。核对不通过时抛异常，不接触私钥。 */
    fun signRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): SignOutcome

    fun close()
}

/** 只含公钥数据。 */
interface FrameStream {
    val fragmentCount: Int

    fun nextPart(): String

    fun close()
}

/** 扫码接收。帧的顺序不重要，收够就能取出消息。 */
interface FrameReceiver {
    /** 返回已解出的比例，0 到 1。 */
    fun receive(part: String): Float

    val complete: Boolean
    val urType: String?

    /** 这条消息一共多少帧，收到第一帧之前是 0。 */
    val fragmentCount: Int

    fun message(): ByteArray

    fun close()
}

/** 核心库给的一个主账户。[kind] 决定怎么显示，见 [com.ccobcode.bitone.flow.accountsFor]。 */
class AccountAddress(val kind: String, val path: String, val address: String)

/** A12 的一行。原生币的 [contract] 是 [NATIVE_CONTRACT]。 */
class ManifestEntry(
    val network: String,
    val networkLabel: String,
    val symbol: String,
    val contract: String,
    val icon: String,
)

const val NATIVE_CONTRACT = "native"

/** 第 [index] 个词（从 0 起）是 [options] 里的哪一个。故意不用 data class，防止 toString 打出词。 */
class QuizItem(val index: Int, val options: List<String>)

/** 按行排列，true 是深色模块。 */
class QrMatrix(val size: Int, val modules: BooleanArray)

sealed class WalletCoreException(message: String) : Exception(message) {
    class InvalidMnemonic : WalletCoreException("助记词无效")

    class InvalidPin : WalletCoreException("PIN 格式错误")

    class WrongPin : WalletCoreException("PIN 错误或数据损坏")

    class Other(message: String) : WalletCoreException(message)
}

package com.ccobcode.bitone.platform

/** 从低到高排序,分类全在核心库里完成,界面只负责画出来. */
enum class Risk {
    Transfer,
    Recognized,
    Unknown,
    Approval,
    PermissionChange,
    Forbidden,
}

enum class ConfirmMode {
    Tap,
    Hold,
    Refuse,
}

class ReviewRow(val label: String, val value: String, val mono: Boolean)

/** 金额.[asset] 是内置清单的条目,[icon] 是这一条的图标名,核心库按网络加合约查到才有.查不到时 [symbol] 也为空,[text] 是原始整数. */
class ReviewAmount(
    val text: String,
    val symbol: String?,
    val asset: String?,
    val icon: String?,
    val unlimited: Boolean,
)

class ReviewAction(val risk: Risk, val title: String, val rows: List<ReviewRow>)

/** 原始数据的一行.[note] 是核心库分析出的提示,比如疑似地址. */
class RawLine(val text: String, val note: String?)

/** 核对页要显示的全部内容. */
class ReviewResult(
    val risk: Risk,
    val confirm: ConfirmMode,
    val network: String,
    val title: String,
    val amount: ReviewAmount?,
    val counterpartyLabel: String?,
    val counterparty: String?,
    val alerts: List<String>,
    val actions: List<ReviewAction>,
    val rows: List<ReviewRow>,
    val raw: List<RawLine>,
    /** 指纹对不上,要提示检查 BIP39 密码. */
    val fingerprintMismatch: Boolean,
    /** 请求里声明的指纹.数据解析不出来时为空. */
    val requestFingerprint: String?,
) {
    val refused: Boolean
        get() = confirm == ConfirmMode.Refuse || risk == Risk.Forbidden
}

/** A9 的内容. */
class AddressReview(
    val belongs: Boolean,
    val network: String,
    val path: String,
    val address: String,
    val derived: String,
    /** 地址类型,比如 Native SegWit. */
    val kind: String,
    val fingerprintMismatch: Boolean,
    val requestFingerprint: String? = null,
)

/** 最高找零索引,主账户和测试币账户分开记.只是内部存储,界面不出现. */
class ChangeIndexes(val mainnet: UInt, val testnet: UInt)

/** [changeIndex] 要写回本机,下一笔核对时传入.[changeIsTestnet] 决定写到 [ChangeIndexes] 的哪个槽. */
class SignOutcome(val frames: FrameStream, val changeIndex: UInt?, val changeIsTestnet: Boolean = false)

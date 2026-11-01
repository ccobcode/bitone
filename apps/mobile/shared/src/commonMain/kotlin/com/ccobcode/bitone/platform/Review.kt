package com.ccobcode.bitone.platform

/** 从低到高排序,分类全在核心库里完成,界面只负责画出来.无法识别排在权限变更之上. */
enum class Risk {
    Transfer,
    Recognized,
    Approval,
    PermissionChange,
    Unknown,
    Forbidden,
}

enum class ConfirmMode {
    Tap,
    Hold,
    /** 长按之后还要明确确认一次.裸哈希和整段未知载荷走这里. */
    HoldConfirm,
    Refuse,
}

/** 金额.[asset] 是内置清单的条目,[icon] 是这一条的图标名,核心库按网络加合约查到才有.查不到时 [symbol] 也为空,[text] 是原始整数. */
class ReviewAmount(
    val text: String,
    val symbol: String?,
    val asset: String?,
    val icon: String?,
    val unlimited: Boolean,
)

/** A6d 的一组控制方.[current] 为空时当前控制方就是付款账户,[next] 为空时这笔权限变更没有新控制方. */
class ReviewController(val current: String?, val next: String?)

/** 权限变更打包的转账或调用的对方. */
class ReviewParty(val label: String, val value: String)

/** 页面顶部,在字段列表上方,不能代替列表.[controllers],[parties],[payer] 给 A6d 和 A8,通用列表不按字段名分支,表达不了这些. */
class ReviewSummary(
    val title: String,
    val amount: ReviewAmount?,
    val counterpartyLabel: String?,
    val counterparty: String?,
    val controllers: List<ReviewController>,
    val parties: List<ReviewParty>,
    val payer: String?,
)

enum class FieldKind {
    Bool,
    Int,
    Uint,
    Amount,
    Address,
    Hash,
    Bytes,
    Text,
    Enum,
    Time,
    Script,
    Struct,
    List,
    Opaque,
}

enum class FieldSource {
    Signed,
    Context,
    Derived,
    Manifest,
    Descriptor,
    Hint,
}

/** 解码树的一个节点,前序排列.[value] 已经由核心库格式化,这里原样显示. */
class FieldRow(
    val id: UInt,
    val parent: UInt?,
    val depth: UInt,
    val name: String,
    val label: String,
    val kind: FieldKind,
    val value: String,
    val source: FieldSource,
    val offset: UInt?,
    val len: UInt?,
    val risk: Risk?,
    val note: String?,
    val own: Boolean,
)

/** 核对页要显示的全部内容. */
class ReviewResult(
    val risk: Risk,
    val confirm: ConfirmMode,
    val network: String,
    /** 页面顶部显示主网提示.不能确认是测试网时都是 true. */
    val mainnet: Boolean,
    val summary: ReviewSummary,
    val alerts: List<String>,
    /** 解码树的全部节点.只有拒绝时才是空的. */
    val fields: List<FieldRow>,
    /** 签名时原样传回核心库. */
    val reviewDigest: ByteArray,
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

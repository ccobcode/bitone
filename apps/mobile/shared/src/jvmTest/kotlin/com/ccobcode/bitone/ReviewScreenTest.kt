package com.ccobcode.bitone

import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.ComposeUiTest
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.runSkikoComposeUiTest
import androidx.compose.ui.test.swipeUp
import androidx.compose.ui.text.TextLayoutResult
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.FieldRow
import com.ccobcode.bitone.platform.FieldSource
import com.ccobcode.bitone.platform.ReviewAmount
import com.ccobcode.bitone.platform.ReviewController
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.ui.theme.BitOneTheme
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import org.junit.Ignore

private const val PIN = "123456"
private const val REQUEST = "bitone-request"

/** 360x800 的手机去掉状态栏和导航栏后的可用区域,密度是 1,像素就是 dp.第一屏的断言都以它为准. */
private val Phone = Size(360f, 740f)

/** 比最长的内容需要的次数多,滚到底之后再滑没有影响. */
private const val SWIPES_TO_THE_END = 4
private const val LITTLE = 80f

private const val CONFIRM = "确认并签名"
private const val HOLD = "长按签名"
private const val SCROLL_FIRST = "滚到底部后确认"
private const val SECOND_CONFIRM = "再次确认并签名"
private const val SECOND_WARNING = "本机看不到这段内容的含义,签名后无法撤回."
private const val BACK_TO_REVIEW = "返回核对"
private const val REFUSE = "拒绝"
private const val PIN_PAGE = "输入 PIN"
private const val MAINNET = "主网,真实资产"
private const val CURRENT = "当前控制方"
private const val NEXT = "新控制方"
private const val EXTERNAL = "外部地址"
private const val FEE_LABEL = "手续费上限"
private const val FEE = "0.000063 ETH"
private const val TRON_FEE = "30 TRX"
private const val USDT_ASSET = "ethereum:0xdAC17F958D2ee523a2206206994597C13D831ec7"
private const val MAX_ALLOWANCE = "115792089237316195423570985008687907853269984665640564039457"
private const val UNLIMITED_ALERT = "额度是无限,对方可随时转走全部这种资产"

private const val RECIPIENT = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
private const val RECIPIENT_SHOWN = "0x7099 7970 C518 12dc 3A01 0C7d 01b5 0e0d 17dc 79C8"
private const val SPENDER = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
private const val SPENDER_SHOWN = "0x3C44 CdDd B6a9 00fa 2b58 5dd2 99e0 3d12 FA42 93BC"
private const val OWNER = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
private const val OWNER_SHOWN = "0xf39F d6e5 1aad 88F6 F4ce 6aB8 8272 79cf fFb9 2266"
private const val DELEGATE = "0x90F79bf6EB2c4f870365E785982E1f101E93b906"
private const val DELEGATE_SHOWN = "0x90F7 9bf6 EB2c 4f87 0365 E785 982E 1f10 1E93 b906"

private val Ether = ReviewAmount("1.5", "ETH", "ethereum:native", "eth", false)

/** EIP-1559 交易的字段,顺序和核心库相同:手续费上限紧跟根节点. */
private val EvmFields =
    listOf(
        fieldOf("transaction", depth = 0u, label = "交易", value = ""),
        fieldOf("fee_cap", FieldSource.Derived, label = FEE_LABEL, value = FEE),
        fieldOf("tx_type", label = "交易类型", value = "EIP-1559"),
        fieldOf("chain_id", label = "Chain ID", value = "1"),
        fieldOf("nonce", label = "Nonce", value = "7"),
        fieldOf("max_priority_fee_per_gas", label = "优先费", value = "2 Gwei"),
        fieldOf("max_fee_per_gas", label = "最高单价", value = "3 Gwei"),
        fieldOf("gas_limit", label = "Gas Limit", value = "21000"),
        fieldOf("to", label = "收款地址或合约", value = RECIPIENT),
        fieldOf("value", label = "金额", value = "1.5 ETH"),
    )

/** TRC-20 转账的字段,顺序和核心库相同:`raw_data` 按字段编号走,`fee_limit` 在合约的全部字段之后. */
private val TronFields =
    listOf(
        fieldOf("raw_data", depth = 0u, label = "交易内容", value = ""),
        fieldOf("ref_block_bytes", label = "引用区块高度", value = "abcd"),
        fieldOf("ref_block_hash", label = "引用区块哈希", value = "0102030405060708"),
        fieldOf("expiration", label = "过期时间", value = "2026-11-01 12:30:00 UTC"),
        fieldOf("contract", label = "合约", value = ""),
        fieldOf("type", depth = 3u, label = "类型", value = "TriggerSmartContract"),
        fieldOf("parameter", depth = 3u, label = "合约参数", value = ""),
        fieldOf("type_url", depth = 4u, label = "类型地址", value = "type.googleapis.com/protocol.TriggerSmartContract"),
        fieldOf("value", depth = 4u, label = "值", value = ""),
        fieldOf("owner_address", depth = 5u, label = "发起账户", value = "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"),
        fieldOf("contract_address", depth = 5u, label = "合约地址", value = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"),
        fieldOf("data", depth = 5u, label = "数据", value = "a9059cbb0000000000000000000000000000000000000000"),
        fieldOf("timestamp", label = "创建时间", value = "2026-11-01 12:00:00 UTC"),
        fieldOf("fee_limit", label = FEE_LABEL, value = TRON_FEE),
    )

/** 无法识别的调用多一行不进签名原文的 `context`,除了普通交易它都默认展开. */
private val UnknownFields =
    EvmFields +
        listOf(
            fieldOf("data", label = "十六进制", value = "deadbeef00112233"),
            fieldOf("path", FieldSource.Context, label = "派生路径", value = "m/44'/60'/0'/0/0"),
        )

private fun transfer(fields: List<FieldRow> = EvmFields) = reviewOf(amount = Ether, fields = fields)

private fun approval(amount: ReviewAmount, alerts: List<String> = emptyList()) =
    reviewOf(
        Risk.Approval,
        ConfirmMode.Hold,
        alerts,
        amount = amount,
        counterparty = SPENDER,
        counterpartyLabel = "被授权方",
        fields = EvmFields,
    )

private fun unlimitedApproval() =
    approval(ReviewAmount(MAX_ALLOWANCE, "USDT", USDT_ASSET, "usdt", true), listOf(UNLIMITED_ALERT))

private fun unknownCall(confirm: ConfirmMode = ConfirmMode.Hold) =
    reviewOf(
        Risk.Unknown,
        confirm,
        listOf("本机不认识这个合约调用,无法判断签名后果,签名前请确认来源和调用数据"),
        amount = null,
        counterparty = SPENDER,
        counterpartyLabel = "目标合约",
        fields = UnknownFields,
    )

private fun permission(vararg controllers: ReviewController, fields: List<FieldRow> = EvmFields) =
    reviewOf(
        Risk.PermissionChange,
        ConfirmMode.Hold,
        controllers = controllers.toList(),
        payer = OWNER,
        amount = null,
        fields = fields,
    )

@OptIn(ExperimentalTestApi::class, ExperimentalCoroutinesApi::class)
class ReviewScreenTest {
    /** 流程的协程在这里排队,长按的 3 秒由测试推进. */
    private val flowScope = TestScope()

    /** 走真实流程扫进核对页,渲染整个应用,断言只看屏幕上有什么. */
    private fun reviewing(review: ReviewResult, checks: ComposeUiTest.() -> Unit) =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN) { it.core.review = review }
            flowScope.openScan(rig.flow)
            flowScope.feed(rig.flow, frames(REQUEST))
            setContent { App(rig.flow, FakeBrightness()) }
            checks()
        }

    /** 同上,检查里还能拿到流程和假的核心库. */
    private fun reviewingWith(review: ReviewResult, checks: ComposeUiTest.(Rig) -> Unit) =
        runSkikoComposeUiTest(Phone) {
            val rig = flowScope.readyRig(PIN) { it.core.review = review }
            flowScope.openScan(rig.flow)
            flowScope.feed(rig.flow, frames(REQUEST))
            setContent { App(rig.flow, FakeBrightness()) }
            checks(rig)
        }

    /** 不滚动就能读完:`assertIsDisplayed` 露出一个像素也算,所以还要求没有被滚动区域的下沿切开.吸底区在下沿之下. */
    private fun ComposeUiTest.onFirstScreen(vararg texts: String) {
        val fold =
            onAllNodes(hasScrollAction()).fetchSemanticsNodes().firstOrNull()?.boundsInRoot?.bottom ?: Phone.height
        texts.forEach { text ->
            val node = onNodeWithText(text).assertIsDisplayed().fetchSemanticsNode()
            val top = node.positionInRoot.y
            assertTrue(top >= fold || top + node.size.height <= fold, "$text 被切开")
        }
    }

    private fun ComposeUiTest.absent(vararg texts: String) = texts.forEach { text ->
        assertEquals(0, onAllNodesWithText(text).fetchSemanticsNodes().size, text)
    }

    private fun ComposeUiTest.settle() {
        flowScope.advanceUntilIdle()
        waitForIdle()
    }

    private fun ComposeUiTest.scrollToEnd() =
        repeat(SWIPES_TO_THE_END) {
            onNode(hasScrollAction()).performTouchInput { swipeUp() }
        }

    private fun ComposeUiTest.scrollALittle() =
        onNode(hasScrollAction()).performTouchInput {
            swipeUp(startY = centerY, endY = centerY - LITTLE)
        }

    /** 按住够 3 秒再松手. */
    private fun ComposeUiTest.hold(label: String) {
        onNodeWithText(label).performTouchInput { down(center) }
        settle()
        onRoot().performTouchInput { up() }
        settle()
    }

    private fun ComposeUiTest.tap(label: String) {
        onNodeWithText(label).performClick()
        settle()
    }

    /** 红色和黄色是这个页面要传达的信息,它们来自文字实际的颜色. */
    private fun ComposeUiTest.assertColor(text: String, color: Color) {
        val layouts = mutableListOf<TextLayoutResult>()
        onNodeWithText(text).fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action?.invoke(layouts)
        assertEquals(color, layouts.single().layoutInput.style.color, text)
    }

    /** 长按按钮没有点击动作,这里按角色数,才不会漏掉它. */
    private fun ComposeUiTest.buttons(): List<String?> =
        onAllNodes(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Button)).fetchSemanticsNodes().map {
            it.config.getOrNull(SemanticsProperties.Text)?.joinToString()
        }

    // A6 普通转账

    @Test
    fun aPlainTransferShowsAmountAndRecipientOnTheFirstScreen() =
        reviewing(transfer()) {
            onFirstScreen("普通转账", "原生币转账", "Ethereum", "1.5", "ETH", "收款地址", RECIPIENT_SHOWN)
            absent(HOLD)
        }

    @Test
    fun aPlainTransferIsConfirmedByTap() =
        reviewing(transfer()) {
            scrollToEnd()
            onNodeWithText(CONFIRM).assertIsEnabled()
            tap(CONFIRM)
            onFirstScreen(PIN_PAGE)
        }

    // A6b 授权

    @Test
    fun anApprovalIsRedAndNamesTheSpender() =
        reviewing(approval(ReviewAmount("100", "USDT", USDT_ASSET, "usdt", false))) {
            onFirstScreen("高风险", "授权", "授权交易", "对方可以动用授权额度内的资产.", "100", "USDT", "被授权方", SPENDER_SHOWN)
            assertColor("授权交易", BitOneTheme.colors.danger)
        }

    @Test
    fun anUnlimitedApprovalSaysSoAndNeedsAHold() =
        reviewing(unlimitedApproval()) {
            onFirstScreen("无限额度授权", "对方可随时转走你全部的 USDT.", UNLIMITED_ALERT, "无限", "USDT", SPENDER_SHOWN)
            absent(MAX_ALLOWANCE)
            assertColor("无限额度授权", BitOneTheme.colors.danger)
            assertColor("无限", BitOneTheme.colors.danger)

            scrollToEnd()
            absent(CONFIRM)
            tap(HOLD)
            absent(PIN_PAGE)
            hold(HOLD)
            onFirstScreen(PIN_PAGE)
        }

    // A6c 无法识别

    @Test
    fun anUnknownCallWarnsAndNeedsAHold() =
        reviewing(unknownCall()) {
            onFirstScreen("无法识别", "本机无法解析这笔交易", "只在确认来源可信时签名.", "目标合约", "未知合约", SPENDER_SHOWN)
            assertColor("本机无法解析这笔交易", BitOneTheme.colors.warn)

            scrollToEnd()
            absent(CONFIRM)
            tap(HOLD)
            absent(PIN_PAGE)
            hold(HOLD)
            onFirstScreen(PIN_PAGE)
        }

    @Test
    fun anUnknownCallShowsEveryField() =
        reviewing(unknownCall()) {
            UnknownFields.filter { it.value.isNotEmpty() }
                .forEach {
                    onNodeWithText(it.label).performScrollTo().assertIsDisplayed()
                    onNodeWithText(it.value).performScrollTo().assertIsDisplayed()
                }
        }

    // A6d 权限变更

    @Test
    fun aPermissionChangeShowsBothControllers() =
        reviewing(permission(ReviewController(OWNER, DELEGATE))) {
            onFirstScreen("高风险", "账户控制权将被改变", CURRENT, OWNER_SHOWN, "本账户", NEXT, DELEGATE_SHOWN, EXTERNAL)
            assertColor("账户控制权将被改变", BitOneTheme.colors.danger)
        }

    @Test
    fun aPermissionChangeWithoutANewControllerSaysSo() =
        reviewing(permission(ReviewController(OWNER, null))) {
            onFirstScreen(CURRENT, OWNER_SHOWN, NEXT, "没有新控制方")
            // 字段列表里的收款地址不能顶替新控制方.
            absent(EXTERNAL, RECIPIENT_SHOWN)
        }

    @Test
    fun twoNewControllersBothAppear() {
        val delegates =
            EvmFields +
                listOf(
                    fieldOf("authorization_list", label = "授权列表", value = ""),
                    fieldOf("address", depth = 3u, label = "地址", value = SPENDER),
                    fieldOf("address", depth = 3u, label = "地址", value = DELEGATE),
                )
        reviewing(permission(ReviewController(OWNER, SPENDER), ReviewController(OWNER, DELEGATE), fields = delegates)) {
            onNodeWithText(SPENDER).performScrollTo().assertIsDisplayed()
            onNodeWithText(DELEGATE).performScrollTo().assertIsDisplayed()
            // 只画一张卡片会让人以为控制方只有一个.
            absent(NEXT, EXTERNAL)
        }
    }

    // 网络

    @Test
    fun mainnetIsAnnouncedAtTheTop() =
        reviewing(transfer()) {
            onFirstScreen(MAINNET, "Ethereum")
            assertColor(MAINNET, BitOneTheme.colors.warn)
        }

    @Test
    fun aTestnetHasNoMainnetBanner() =
        reviewing(reviewOf(network = "sepolia", mainnet = false, fields = EvmFields)) {
            onFirstScreen("Sepolia")
            absent(MAINNET)
        }

    // 滚动门槛

    @Test
    fun tapStaysUnavailableUntilTheEnd() =
        reviewing(transfer()) {
            absent(CONFIRM)
            onNodeWithText(SCROLL_FIRST).assertIsNotEnabled()
            tap(SCROLL_FIRST)
            absent(PIN_PAGE)

            scrollALittle()
            absent(CONFIRM)
            onNodeWithText(SCROLL_FIRST).assertIsNotEnabled()

            scrollToEnd()
            absent(SCROLL_FIRST)
            onNodeWithText(CONFIRM).assertIsEnabled()
        }

    @Test
    fun holdStaysUnavailableUntilTheEnd() =
        reviewing(unknownCall(ConfirmMode.HoldConfirm)) {
            absent(HOLD)
            hold(SCROLL_FIRST)
            absent(SECOND_CONFIRM)

            scrollALittle()
            absent(HOLD)
            hold(SCROLL_FIRST)
            absent(SECOND_CONFIRM)

            scrollToEnd()
            absent(SCROLL_FIRST)
            hold(HOLD)
            onFirstScreen(SECOND_CONFIRM)
        }

    @Test
    fun whatFitsOnOneScreenCanBeConfirmedAtOnce() =
        reviewing(transfer(fields = EvmFields.take(1))) {
            absent(SCROLL_FIRST)
            onNodeWithText(CONFIRM).assertIsEnabled()
        }

    // 二次确认

    @Test
    fun theSecondConfirmationCanBeCancelled() =
        reviewing(unknownCall(ConfirmMode.HoldConfirm)) {
            scrollToEnd()
            absent(SECOND_CONFIRM, SECOND_WARNING)

            hold(HOLD)
            onFirstScreen(SECOND_WARNING, REFUSE, SECOND_CONFIRM, BACK_TO_REVIEW)
            onNodeWithText(SECOND_CONFIRM).assertIsEnabled()
            absent(HOLD, PIN_PAGE)

            tap(BACK_TO_REVIEW)
            onFirstScreen(HOLD, REFUSE)
            absent(SECOND_CONFIRM, SECOND_WARNING, PIN_PAGE)
        }

    @Test
    fun theSecondConfirmationLeadsToThePin() =
        reviewing(unknownCall(ConfirmMode.HoldConfirm)) {
            scrollToEnd()
            hold(HOLD)
            tap(SECOND_CONFIRM)
            onFirstScreen(PIN_PAGE)
        }

    // 行数核对

    /** 核心库只收到真正经过画行代码的行数.普通转账的附加字段默认收起,收起的行也在组合里. */
    @Test
    fun everyRowThatReachesTheScreenIsCounted() =
        reviewingWith(transfer(UnknownFields)) { rig ->
            scrollToEnd()
            onNodeWithText("附加字段").assertIsDisplayed()
            absent("派生路径")
            tap(CONFIRM)
            flowScope.submit(rig.flow, PIN)
            settle()
            assertEquals(UnknownFields.size.toUInt(), rig.core.signCalls.single().renderedFields)
        }

    // 拒绝

    /** 被拒绝的请求进不了核对页,所以这里看用户实际停在哪一页. */
    @Test
    fun aRefusedRequestHasNoWayToSign() =
        reviewing(reviewOf(Risk.Forbidden, ConfirmMode.Refuse, listOf("Chain ID 与网络不符"), fields = EvmFields)) {
            onFirstScreen("这笔请求不能签名", "Chain ID 与网络不符")
            assertEquals(listOf<String?>("退出"), buttons())
        }

    @Test
    fun aRefusalWinsOverALowRisk() =
        reviewing(reviewOf(Risk.Transfer, ConfirmMode.Refuse, fields = EvmFields)) {
            onFirstScreen("这笔请求不能签名")
            assertEquals(listOf<String?>("退出"), buttons())
        }

    // 手续费

    @Test
    fun theFeeIsOnTheFirstScreen() =
        reviewing(transfer()) {
            onFirstScreen(FEE_LABEL, FEE)
            // 反证这个断言分得清屏幕内外:最后一行要滚动才看得到.
            onNodeWithText(EvmFields.last().value).assertIsNotDisplayed()
        }

    @Ignore("360x740 上授权页的横幅和卡片把手续费挤出第一屏,393x800 放得下")
    @Test
    fun theFeeIsOnTheFirstScreenOfAnApproval() = reviewing(unlimitedApproval()) { onFirstScreen(FEE_LABEL, FEE) }

    @Test fun theFeeIsOnTheFirstScreenOfAnUnknownCall() = reviewing(unknownCall()) { onFirstScreen(FEE_LABEL, FEE) }

    @Ignore("360x740 上权限变更页的控制方卡片把手续费挤出第一屏,393x800 放得下")
    @Test
    fun theFeeIsOnTheFirstScreenOfAPermissionChange() =
        reviewing(permission(ReviewController(OWNER, DELEGATE))) { onFirstScreen(FEE_LABEL, FEE) }

    @Test
    fun theFeeIsOnTheFirstScreenOfAHighRiskTransfer() =
        reviewing(reviewOf(Risk.Transfer, ConfirmMode.Hold, listOf("手续费明显偏高"), amount = Ether, fields = EvmFields)) {
            onFirstScreen(FEE_LABEL, FEE)
        }

    @Ignore("TRON 的 fee_limit 排在合约字段之后,第一屏看不到手续费")
    @Test
    fun theTronFeeIsOnTheFirstScreen() =
        reviewing(
            reviewOf(
                network = "tron",
                amount = ReviewAmount("25", "USDT", "tron:TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t", "usdt", false),
                fields = TronFields,
            )
        ) {
            onFirstScreen(FEE_LABEL, TRON_FEE)
        }
}

package com.ccobcode.bitone

import com.ccobcode.bitone.flow.CURRENT_CONTROLLER
import com.ccobcode.bitone.flow.NEW_CONTROLLER
import com.ccobcode.bitone.flow.ReviewStyle
import com.ccobcode.bitone.flow.actionRows
import com.ccobcode.bitone.flow.controllerCard
import com.ccobcode.bitone.flow.evaluate
import com.ccobcode.bitone.flow.fieldBlocks
import com.ccobcode.bitone.flow.isHighRisk
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.FieldSource
import com.ccobcode.bitone.platform.HardwareLevel
import com.ccobcode.bitone.platform.ReviewAction
import com.ccobcode.bitone.platform.ReviewResult
import com.ccobcode.bitone.platform.ReviewRow
import com.ccobcode.bitone.platform.Risk
import com.ccobcode.bitone.ui.screens.FIRST_SCREEN_LABELS
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

private const val PAYER = "payer"
private const val HARMLESS = "harmless"
private const val ATTACKER = "attacker"

class ReviewRulesTest {
    private fun change(current: String, next: String) =
        ReviewAction(
            Risk.PermissionChange,
            "SetAuthority",
            listOf(
                ReviewRow(CURRENT_CONTROLLER, current, true),
                ReviewRow(NEW_CONTROLLER, next, true),
                ReviewRow("账户", "token", true),
            ),
        )

    private fun permission(vararg actions: ReviewAction, payerLabel: String = "付款账户", counterparty: String = "") =
        reviewOf(
            Risk.PermissionChange,
            ConfirmMode.Hold,
            actions = actions.toList(),
            rows = listOf(ReviewRow(payerLabel, PAYER, true)),
            counterparty = counterparty,
        )

    /** 页面在动作里列出的某个标签的值,按动作分组. */
    private fun shown(review: ReviewResult, label: String) =
        actionRows(review, ReviewStyle.Permission).map { rows -> rows.filter { it.label == label }.map { it.value } }

    @Test
    fun theOrderIsTheStandardOrder() {
        assertEquals(
            listOf(
                Risk.Transfer,
                Risk.Recognized,
                Risk.Approval,
                Risk.PermissionChange,
                Risk.Unknown,
                Risk.Forbidden,
            ),
            Risk.entries,
        )
    }

    @Test
    fun unknownKeepsItsOwnStyleThoughItRanksAboveHighRisk() {
        assertFalse(isHighRisk(Risk.Unknown))
        assertTrue(isHighRisk(Risk.Approval))
        assertTrue(isHighRisk(Risk.PermissionChange))
        assertTrue(isHighRisk(Risk.Forbidden))
        assertFalse(isHighRisk(Risk.Recognized))
    }

    @Test
    fun onlyContextRowsFold() {
        val blocks =
            fieldBlocks(
                listOf(
                    fieldOf("signed"),
                    fieldOf("first", FieldSource.Context),
                    fieldOf("second", FieldSource.Context),
                    fieldOf("derived", FieldSource.Derived),
                    fieldOf("hint", FieldSource.Hint),
                    fieldOf("third", FieldSource.Context),
                )
            )
        assertEquals(listOf(false, true, false, true), blocks.map { it.foldable })
        assertEquals(listOf(1, 2, 2, 1), blocks.map { it.rows.size })
    }

    @Test
    fun blocksKeepEveryRowInOrder() {
        val fields =
            listOf(
                fieldOf("a", FieldSource.Context),
                fieldOf("b"),
                fieldOf("c", FieldSource.Context),
                fieldOf("d", FieldSource.Manifest),
                fieldOf("e"),
            )
        assertEquals(fields, fieldBlocks(fields).flatMap { it.rows })
    }

    @Test
    fun noFieldsMeansNoBlocks() {
        assertTrue(fieldBlocks(emptyList()).isEmpty())
    }

    @Test
    fun everyNewControllerStaysInItsAction() {
        val review = permission(change(PAYER, HARMLESS), change(PAYER, ATTACKER))
        assertNull(controllerCard(review))
        assertEquals(listOf(listOf(HARMLESS), listOf(ATTACKER)), shown(review, NEW_CONTROLLER))
        assertEquals(listOf(listOf(PAYER), listOf(PAYER)), shown(review, CURRENT_CONTROLLER))
    }

    /** EVM 把最后一个委托放进 counterparty,其余的委托不能因此消失. */
    @Test
    fun severalDelegatesAreAllShown() {
        val review = permission(change(PAYER, HARMLESS), change(PAYER, ATTACKER), counterparty = ATTACKER)
        assertNull(controllerCard(review))
        assertEquals(listOf(listOf(HARMLESS), listOf(ATTACKER)), shown(review, NEW_CONTROLLER))
    }

    @Test
    fun differentCurrentControllersAreNotFolded() {
        val review = permission(change(PAYER, ATTACKER), change("other", ATTACKER))
        assertNull(controllerCard(review))
        assertEquals(listOf(listOf(PAYER), listOf("other")), shown(review, CURRENT_CONTROLLER))
    }

    @Test
    fun oneControlChangeMovesToTheCard() {
        val review = permission(change(PAYER, ATTACKER), change(PAYER, ATTACKER))
        val card = assertNotNull(controllerCard(review))
        assertEquals(PAYER, card.current)
        assertEquals(ATTACKER, card.next)
        assertEquals(listOf(emptyList(), emptyList()), shown(review, NEW_CONTROLLER))
        assertEquals(listOf(listOf("token"), listOf("token")), shown(review, "账户"))
    }

    @Test
    fun ownChipNeedsThePayer() {
        assertTrue(controllerCard(permission(change(PAYER, ATTACKER)))!!.own)
        assertTrue(controllerCard(permission(change(PAYER, ATTACKER), payerLabel = "手续费支付者"))!!.own)
        assertFalse(controllerCard(permission(change("other", ATTACKER)))!!.own)
        assertFalse(controllerCard(permission(change("缺失", ATTACKER), payerLabel = "别的标签"))!!.own)
    }

    @Test
    fun otherPagesKeepEveryRow() {
        val action = change(PAYER, ATTACKER)
        assertEquals(listOf(action.rows), actionRows(permission(action), ReviewStyle.Plain))
    }

    @Test
    fun priorityFeeIsOnTheFirstScreen() {
        assertTrue("优先费" in FIRST_SCREEN_LABELS)
    }

    @Test
    fun warningsReachTheReviewPage() {
        val report = evaluate(facts(keystore = HardwareLevel.Tee, overlayApps = 2))
        assertFalse(report.blocked)
        assertEquals(listOf("硬件密钥库", "悬浮窗应用"), report.warnings.map { it.label })
        assertTrue(evaluate(GoodFacts).warnings.isEmpty())
    }
}

package com.ccobcode.bitone

import com.ccobcode.bitone.flow.controllerCard
import com.ccobcode.bitone.flow.evaluate
import com.ccobcode.bitone.flow.fieldBlocks
import com.ccobcode.bitone.flow.isHighRisk
import com.ccobcode.bitone.platform.ConfirmMode
import com.ccobcode.bitone.platform.FieldSource
import com.ccobcode.bitone.platform.HardwareLevel
import com.ccobcode.bitone.platform.ReviewController
import com.ccobcode.bitone.platform.Risk
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
    private fun change(current: String?, next: String?) = ReviewController(current, next)

    private fun permission(vararg controllers: ReviewController, payer: String? = PAYER, counterparty: String = "") =
        reviewOf(
            Risk.PermissionChange,
            ConfirmMode.Hold,
            controllers = controllers.toList(),
            payer = payer,
            counterparty = counterparty,
        )

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
    fun severalNewControllersHaveNoCard() {
        assertNull(controllerCard(permission(change(PAYER, HARMLESS), change(PAYER, ATTACKER))))
    }

    /** EVM 把最后一个委托放进 counterparty,其余的委托不能因此消失. */
    @Test
    fun severalDelegatesAreNotFoldedIntoTheLastOne() {
        val review = permission(change(PAYER, HARMLESS), change(PAYER, ATTACKER), counterparty = ATTACKER)
        assertNull(controllerCard(review))
    }

    @Test
    fun differentCurrentControllersAreNotFolded() {
        assertNull(controllerCard(permission(change(PAYER, ATTACKER), change("other", ATTACKER))))
    }

    @Test
    fun oneControlChangeMovesToTheCard() {
        val review = permission(change(PAYER, ATTACKER), change(PAYER, ATTACKER))
        val card = assertNotNull(controllerCard(review))
        assertEquals(PAYER, card.current)
        assertEquals(ATTACKER, card.next)
    }

    @Test
    fun theCurrentControllerFallsBackToThePayer() {
        val card = assertNotNull(controllerCard(permission(change(null, ATTACKER))))
        assertEquals(PAYER, card.current)
        assertTrue(card.own)
    }

    /** 没有新控制方时卡片说明这一点,不能拿这笔交易的其他对方顶替. */
    @Test
    fun aMissingNewControllerIsNotFilledFromTheCounterparty() {
        val card = assertNotNull(controllerCard(permission(change(PAYER, null), counterparty = HARMLESS)))
        assertEquals(PAYER, card.current)
        assertNull(card.next)
        assertNull(assertNotNull(controllerCard(permission(counterparty = HARMLESS))).next)
    }

    @Test
    fun aChangeWithoutANewControllerDoesNotFoldIntoOneWithIt() {
        assertNull(controllerCard(permission(change(PAYER, ATTACKER), change(PAYER, null))))
    }

    @Test
    fun ownChipNeedsThePayer() {
        assertTrue(controllerCard(permission(change(PAYER, ATTACKER)))!!.own)
        assertFalse(controllerCard(permission(change("other", ATTACKER)))!!.own)
        assertFalse(controllerCard(permission(change("缺失", ATTACKER), payer = null))!!.own)
    }

    @Test
    fun warningsReachTheReviewPage() {
        val report = evaluate(facts(keystore = HardwareLevel.Tee, overlayApps = 2))
        assertFalse(report.blocked)
        assertEquals(listOf("硬件密钥库", "悬浮窗应用"), report.warnings.map { it.label })
        assertTrue(evaluate(GoodFacts).warnings.isEmpty())
    }
}

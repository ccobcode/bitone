package com.ccobcode.bitone

import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.i18n.I18n
import com.ccobcode.bitone.i18n.Lang
import com.ccobcode.bitone.i18n.translate
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest

class I18nTest {
    @AfterTest
    fun reset() {
        I18n.lang = Lang.Zh
    }

    private fun TestScope.rig(system: String, store: FakeStore = FakeStore()) =
        Rig(this, StandardTestDispatcher(testScheduler), store = store, systemLanguage = system)

    @Test
    fun chineseIsThePassThrough() {
        assertEquals("已错 3 次", translate("已错 3 次"))
    }

    @Test
    fun exactTemplateAndNestedTextAreTranslated() {
        I18n.lang = Lang.En
        assertEquals("Transfer", translate("普通转账"))
        assertEquals("3 wrong attempts", translate("已错 3 次"))
        // 模板里的动态内容原样保留，内容本身是文案时也要翻译。
        assertEquals("Retry in 5 min 30 s", translate("5 分 30 秒 后可重试"))
    }

    @Test
    fun theMostSpecificTemplateWins() {
        I18n.lang = Lang.En
        assertEquals("3 digits entered", translate("已输入 3 位"))
        assertEquals("Warnings: 1", translate("1 项警告"))
    }

    @Test
    fun amountsAndAddressesAreNeverTouched() {
        I18n.lang = Lang.En
        assertEquals("1.5 ETH", translate("1.5 ETH"))
        assertEquals("0xabc", translate("0xabc"))
    }

    @Test
    fun unknownTextFallsBackToTheOriginal() {
        I18n.lang = Lang.En
        assertEquals("这句没有译文", translate("这句没有译文"))
    }

    @Test
    fun languageFollowsTheSystemUntilChosen() = runTest {
        val store = FakeStore()
        rig("en-US", store)
        assertEquals(Lang.En, I18n.lang)
        rig("zh", store)
        assertEquals(Lang.Zh, I18n.lang)

        val flow = rig("zh", store).flow
        flow.toggleLanguage()
        assertEquals(Lang.En, I18n.lang)
        assertEquals("en", store.language)
        rig("zh", store)
        assertEquals(Lang.En, I18n.lang)
    }

    @Test
    fun autoBrightnessIsOnByDefaultAndRemembered() = runTest {
        val store = FakeStore()
        val flow = rig("zh", store).flow
        assertTrue(flow.autoBrightness)
        flow.toggleAutoBrightness()
        assertFalse(flow.autoBrightness)
        assertFalse(store.autoBrightness)
        assertFalse(rig("zh", store).flow.autoBrightness)
        assertEquals(Screen.Booting, flow.screen)
    }
}

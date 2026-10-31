package com.ccobcode.bitone.i18n

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** 语言选择器和系统语言匹配都读这张表。 */
enum class Lang(val code: String, val label: String) {
    En("en", "English"),
    Zh("zh", "中文");

    companion object {
        fun of(code: String): Lang = entries.firstOrNull { code.lowercase().startsWith(it.code) } ?: En
    }
}

/** 当前语言。放在快照状态里，切换时界面自动重组。 */
object I18n {
    var lang by mutableStateOf(Lang.Zh)
}

private val Slot = Regex("\\{(\\d+)\\}")

/** 固定文字多的模板排在前面，「已输入 {0} 位」不能被「{0} 位」抢先匹配。 */
private val Templates by lazy {
    EN.filterKeys(Slot::containsMatchIn)
        .entries
        .sortedByDescending { Slot.replace(it.key, "").length }
        .map { (key, english) ->
            Regex(key.split(Slot).joinToString("(.+?)", "^", "$") { Regex.escape(it) }) to english
        }
}

private fun String.hasChinese() = any { it.code in 0x4E00..0x9FFF }

/**
 * 数据层的文案一律是中文，核对逻辑靠它判断，只在显示时翻译。词典以中文原文为键，来源是 design/i18n.json。
 *
 * 带动态内容的文案按模板匹配，内容原样保留。译不出来就显示原文，不会显示错的内容。
 */
fun translate(text: String): String {
    if (I18n.lang == Lang.Zh || !text.hasChinese()) return text
    EN[text]?.let {
        return it
    }
    for ((regex, english) in Templates) {
        val match = regex.matchEntire(text) ?: continue
        return Slot.replace(english) { slot ->
            translate(match.groupValues[slot.groupValues[1].toInt() + 1])
        }
    }
    return text
}

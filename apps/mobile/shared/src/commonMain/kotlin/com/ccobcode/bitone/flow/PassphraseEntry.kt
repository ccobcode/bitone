package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/**
 * A2d 的输入,BIP39 密码要输两次.建钱包,导入和旧钱包第一次解锁共用这一页.
 *
 * 输错不会报错,只会得到另一个钱包,所以两遍一致才算数.之后密码交给发起方,这里清空.
 */
class PassphraseEntry internal constructor() {
    var input by mutableStateOf("")
        private set

    /** 第一遍,输第二遍时拿来比. */
    private var first by mutableStateOf<String?>(null)

    var mismatch by mutableStateOf(false)
        private set

    val confirming: Boolean
        get() = first != null

    fun type(char: Char) {
        input += char
        mismatch = false
    }

    fun delete() {
        input = input.dropLast(1)
    }

    /** 键盘的"完成".第一遍先记下,第二遍一致时返回密码,不一致时从第一遍重来.[allowEmpty] 为假时空的不算. */
    internal fun next(allowEmpty: Boolean): String? {
        val typed = input
        if (typed.isEmpty() && !allowEmpty) return null
        input = ""
        val previous = first
        if (previous == null) {
            first = typed
            return null
        }
        first = null
        mismatch = previous != typed
        return typed.takeUnless { mismatch }
    }

    internal fun reset() {
        input = ""
        first = null
        mismatch = false
    }
}

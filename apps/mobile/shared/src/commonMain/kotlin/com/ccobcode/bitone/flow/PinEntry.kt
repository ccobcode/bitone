package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.WalletCore

/** A2c 的输入,新 PIN 要输两次.建钱包和修改 PIN 共用这一页. */
class PinEntry internal constructor(private val core: WalletCore) {
    var first by mutableStateOf("")
        private set

    var input by mutableStateOf("")
        private set

    var mismatch by mutableStateOf(false)
        private set

    /** 第一遍输完被核心库判定太简单. */
    var weak by mutableStateOf(false)
        private set

    var error by mutableStateOf<String?>(null)
        internal set

    val confirming: Boolean
        get() = first.length == PIN_LENGTH

    val done: Boolean
        get() = confirming && input == first

    fun digit(digit: Char) {
        if (done || input.length >= PIN_LENGTH) return
        val next = input + digit
        when {
            next.length < PIN_LENGTH -> {
                input = next
                mismatch = false
                weak = false
            }
            !confirming && core.isWeakPin(next) -> {
                input = ""
                mismatch = false
                weak = true
            }
            !confirming -> {
                first = next
                input = ""
                mismatch = false
                weak = false
            }
            next == first -> input = next
            else -> {
                first = ""
                input = ""
                mismatch = true
            }
        }
    }

    fun delete() {
        if (!done) input = input.dropLast(1)
    }

    internal fun reset() {
        clearInput()
        error = null
    }

    /** 离开前台时只清输入,保存失败的提示留着. */
    internal fun clearInput() {
        first = ""
        input = ""
        mismatch = false
        weak = false
    }
}

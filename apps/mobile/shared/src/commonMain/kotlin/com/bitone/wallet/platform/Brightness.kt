package com.bitone.wallet.platform

/** 展示二维码时用最高亮度并保持常亮，离开后恢复。 */
interface Brightness {
    fun maximize()

    fun restore()
}

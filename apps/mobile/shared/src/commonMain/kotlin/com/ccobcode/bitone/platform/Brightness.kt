package com.ccobcode.bitone.platform

/** 展示二维码和扫码时屏幕常亮，离开后恢复。 */
interface Brightness {
    /** 保持常亮。[maximize] 时同时调到最高亮度。 */
    fun hold(maximize: Boolean)

    fun restore()
}

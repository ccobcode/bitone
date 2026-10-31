package com.bitone.wallet.platform

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier

/** 扫码。解码在本机完成，帧的顺序不重要。 */
interface QrScanner {
    fun hasPermission(): Boolean

    /** 弹系统授权框，返回是否同意。 */
    suspend fun requestPermission(): Boolean

    /** 打开系统设置里本应用的页面。 */
    fun openSettings()

    /** 取景画面。每解出一个二维码调用一次 [onCode]，相机打不开时调用 [onFailure]，都在主线程。 页面离开时释放相机。 */
    @Composable fun Viewfinder(modifier: Modifier, torch: Boolean, onCode: (String) -> Unit, onFailure: () -> Unit)
}

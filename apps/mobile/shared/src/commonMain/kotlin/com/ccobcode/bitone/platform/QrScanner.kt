package com.ccobcode.bitone.platform

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier

/** 扫码.解码在本机完成,帧的顺序不重要. */
interface QrScanner {
    fun hasPermission(): Boolean

    /** 弹系统授权框,返回是否同意. */
    suspend fun requestPermission(): Boolean

    /** 打开系统设置里本应用的页面. */
    fun openSettings()

    /** 取景画面.每解出一个二维码调用一次 [onCode],相机打不开时调用 [onFailure],都在主线程.页面离开时释放相机. */
    @Composable fun Viewfinder(modifier: Modifier, onCode: (String) -> Unit, onFailure: () -> Unit)
}

/** 依次试 [options],第一个绑定成功就停.都失败时抛出最后一个的错误. */
fun <T> firstBound(options: List<T>, bind: (T) -> Unit) {
    options.dropLast(1).forEach { if (runCatching { bind(it) }.isSuccess) return }
    bind(options.last())
}

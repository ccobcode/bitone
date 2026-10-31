package com.ccobcode.bitone

import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.view.MotionEvent
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.addCallback
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge

class MainActivity : ComponentActivity() {
    private val app: BitOneApplication
        get() = application as BitOneApplication

    override fun onCreate(savedInstanceState: Bundle?) {
        // 要在任何内容上屏之前，所以排在 super.onCreate 和 setContent 前面。
        app.current.activity = this
        app.platform.screenGuard.protect()
        val bars = SystemBarStyle.dark(Color.TRANSPARENT)
        enableEdgeToEdge(bars, bars)
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) setRecentsScreenshotEnabled(false)
        window.isNavigationBarContrastEnforced = false

        onBackPressedDispatcher.addCallback(this) {
            if (!app.flow.back()) finish()
        }
        setContent {
            App(app.flow, app.platform.brightness, BundledFonts)
        }
    }

    override fun onResume() {
        super.onResume()
        app.current.activity = this
        app.flow.onResume()
    }

    override fun onStop() {
        super.onStop()
        app.flow.onBackground()
    }

    override fun onDestroy() {
        if (app.current.activity === this) app.current.activity = null
        super.onDestroy()
    }

    /** 被其他窗口遮挡时丢弃触摸。在这里过滤覆盖全部按钮，比逐个标记敏感按钮可靠。 filterTouchesWhenObscured 只看 FLAG_WINDOW_IS_OBSCURED，这里连部分遮挡一起挡。 */
    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val obscured = MotionEvent.FLAG_WINDOW_IS_OBSCURED or MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED
        if (event.flags and obscured != 0) return false
        return super.dispatchTouchEvent(event)
    }
}

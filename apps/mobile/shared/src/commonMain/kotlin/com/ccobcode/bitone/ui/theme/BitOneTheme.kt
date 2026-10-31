package com.ccobcode.bitone.ui.theme

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp

/** 字体由平台注入，共享层不依赖资源库。 */
@Immutable
class BitOneFonts(
    val sans: FontFamily = FontFamily.Default,
    val mono: FontFamily = FontFamily.Monospace,
)

/** 系统字号再大也按这个倍数算，再往上按钮和底部导航放不下。 */
private const val MAX_FONT_SCALE = 1.3f

private val LocalFonts = staticCompositionLocalOf { BitOneFonts() }

object BitOneTheme {
    /** 只有深色。二维码固定用浅色令牌，见 [com.ccobcode.bitone.ui.components.QrView]。 */
    val colors = DarkColors

    val elevation = DarkElevation

    val fonts: BitOneFonts
        @Composable get() = LocalFonts.current
}

/** 按令牌逐层画阴影，和网页端的 box-shadow 一致。深色模式的 elevation-1 是空的，卡片靠明度分层。 */
fun Modifier.elevation(layers: List<ShadowLayer>, shape: Shape): Modifier =
    layers.fold(this) { modifier, layer ->
        modifier.dropShadow(shape, Shadow(radius = layer.blur, color = layer.color, offset = DpOffset(0.dp, layer.y)))
    }

@Composable
fun BitOneTheme(fonts: BitOneFonts = BitOneFonts(), content: @Composable () -> Unit) {
    val density = LocalDensity.current
    CompositionLocalProvider(
        LocalDensity provides Density(density.density, minOf(density.fontScale, MAX_FONT_SCALE)),
        LocalFonts provides fonts,
        content = content,
    )
}

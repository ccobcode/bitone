// 由 design/generate.mjs 生成，不要手改。
package com.ccobcode.bitone.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

data class ColorTokens(
    val bg: Color,
    val surface: Color,
    val surface2: Color,
    val surface3: Color,
    val line: Color,
    val lineStrong: Color,
    val text: Color,
    val text2: Color,
    val text3: Color,
    val accent: Color,
    val accentHover: Color,
    val accentText: Color,
    val accentSoft: Color,
    val onAccent: Color,
    val ok: Color,
    val okSoft: Color,
    val warn: Color,
    val warnSoft: Color,
    val danger: Color,
    val dangerSoft: Color,
    val testnet: Color,
    val onTestnet: Color,
    val camera: Color,
)

val LightColors = ColorTokens(
    bg = Color(0xFFF4F5F7),
    surface = Color(0xFFFFFFFF),
    surface2 = Color(0xFFF1F2F5),
    surface3 = Color(0xFFE7E9ED),
    line = Color(0xFFECEDF1),
    lineStrong = Color(0xFFDADDE3),
    text = Color(0xFF0F1115),
    text2 = Color(0xFF464C58),
    text3 = Color(0xFF626976),
    accent = Color(0xFF1F5EFF),
    accentHover = Color(0xFF1A52E0),
    accentText = Color(0xFF1A52E0),
    accentSoft = Color(0xFFEDF2FF),
    onAccent = Color(0xFFFFFFFF),
    ok = Color(0xFF0A7342),
    okSoft = Color(0xFFE8F6EE),
    warn = Color(0xFF8F5400),
    warnSoft = Color(0xFFFEF4E0),
    danger = Color(0xFFB42318),
    dangerSoft = Color(0xFFFDEDEB),
    testnet = Color(0xFFF5A524),
    onTestnet = Color(0xFF1F1600),
    camera = Color(0xFF050607),
)

val DarkColors = ColorTokens(
    bg = Color(0xFF0B0C10),
    surface = Color(0xFF15171C),
    surface2 = Color(0xFF1C1F25),
    surface3 = Color(0xFF262A32),
    line = Color(0xFF22252C),
    lineStrong = Color(0xFF343843),
    text = Color(0xFFF3F4F6),
    text2 = Color(0xFFBDC3CE),
    text3 = Color(0xFF969DAA),
    accent = Color(0xFF1F5EFF),
    accentHover = Color(0xFF2F6AFF),
    accentText = Color(0xFF8FB2FF),
    accentSoft = Color(0xFF1A2547),
    onAccent = Color(0xFFFFFFFF),
    ok = Color(0xFF52C791),
    okSoft = Color(0xFF12291E),
    warn = Color(0xFFF2B04A),
    warnSoft = Color(0xFF2E2412),
    danger = Color(0xFFFF8177),
    dangerSoft = Color(0xFF351A1C),
    testnet = Color(0xFFF5A524),
    onTestnet = Color(0xFF1F1600),
    camera = Color(0xFF050607),
)

/** 一层阴影，对应 CSS box-shadow 的纵向偏移、模糊半径和颜色。 */
data class ShadowLayer(val y: Dp, val blur: Dp, val color: Color)

data class ElevationTokens(
    val elevation1: List<ShadowLayer>,
    val elevation2: List<ShadowLayer>,
    val elevation3: List<ShadowLayer>,
)

val DarkElevation = ElevationTokens(
    elevation1 = listOf(),
    elevation2 = listOf(ShadowLayer(12.dp, 32.dp, Color(0x66000000))),
    elevation3 = listOf(ShadowLayer(24.dp, 56.dp, Color(0x99000000))),
)

object Space {
    val s1 = 4.dp
    val s2 = 8.dp
    val s3 = 12.dp
    val s4 = 16.dp
    val s5 = 20.dp
    val s6 = 24.dp
    val s8 = 32.dp
    val s10 = 40.dp
    val s12 = 48.dp
    val s16 = 64.dp
}

object Radius {
    val r1 = 8.dp
    val r2 = 12.dp
    val r3 = 16.dp
    val r4 = 24.dp
    val rf = 999.dp
}

object FontSize {
    val t1 = 12.sp
    val t2 = 13.sp
    val t3 = 15.sp
    val t4 = 17.sp
    val t5 = 20.sp
    val t6 = 24.sp
    val t7 = 32.sp
    val t8 = 40.sp
}

object LineHeight {
    val t1 = 16.sp
    val t2 = 20.sp
    val t3 = 24.sp
    val t4 = 24.sp
    val t5 = 28.sp
    val t6 = 32.sp
    val t7 = 40.sp
    val t8 = 48.sp
}

object Size {
    val hBtn = 52.dp
    val hInput = 48.dp
    val hRow = 56.dp
    val gutter = 16.dp
    val contentMax = 560.dp
    val switchW = 44.dp
    val switchH = 24.dp
    val switchDot = 20.dp
    val scanFrame = 260.dp
    val scanArm = 44.dp
    val scanStroke = 4.dp
}

/** 动效令牌。时长单位是毫秒。 */
object Motion {
    const val fast = 120
    const val normal = 220
    const val slow = 360
    const val stagger = 30
    const val toast = 5000
    val standardEasing = CubicBezierEasing(0.2f, 0f, 0f, 1f)
    val emphasizedEasing = CubicBezierEasing(0.16f, 1f, 0.3f, 1f)
    val exitEasing = CubicBezierEasing(0.4f, 0f, 1f, 1f)
    const val pressScale = 0.97f
    const val popScale = 0.96f
    const val scrim = 0.45f
    const val staggerMax = 8
    val pageIn = 24.dp
    val pageOut = 8.dp
    val rise = 8.dp
}

/** 二维码的静区、分段上限和轮播间隔，两端相同。间隔单位是毫秒，第一项是默认速度。 */
object Qr {
    const val quiet = 2
    const val maxSegments = 40
    val intervals = listOf(500, 1000, 250)
}

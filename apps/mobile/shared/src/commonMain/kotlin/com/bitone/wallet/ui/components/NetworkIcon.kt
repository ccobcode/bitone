package com.bitone.wallet.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import com.bitone.wallet.flow.Network
import com.bitone.wallet.ui.theme.BitOneTheme

private class Layer(val path: String, val brush: Brush, val evenOdd: Boolean)

private class ParsedLayer(val path: Path, val brush: Brush)

/** 路径和颜色来自 design/icons/networks，带底色，裁成圆形。 */
private fun layers(network: Network): List<Layer> =
    when (network) {
        Network.Bitcoin ->
            listOf(
                Layer("M24 0H0v24h24z", SolidColor(Color(0xFFF7931A)), evenOdd = false),
                Layer(
                    "M18.011 10.432c.249-1.684-1.026-2.582-2.783-3.192l.57-2.27-1.388-.345-.553 2.214-1.107-.265.561-2.23L11.924 4l-.57 2.278-.881-.209v-.008L8.556 5.58l-.37 1.483s1.027.242 1.01.25c.563.144.659.513.643.801l-.642 2.599.145.048-.153-.032-.906 3.633c-.064.169-.241.425-.633.32.015.024-1.003-.24-1.003-.24l-.69 1.58 1.805.449.986.257-.577 2.302 1.387.344.562-2.277 1.114.288-.569 2.27 1.387.345.57-2.302c2.365.449 4.146.273 4.892-1.869.602-1.724-.032-2.71-1.275-3.368.906-.2 1.588-.802 1.764-2.03zm-3.168 4.443c-.425 1.725-3.328.786-4.266.561l.762-3.055c.938.24 3.954.698 3.504 2.486zm.425-4.467c-.385 1.564-2.807.77-3.585.578l.69-2.768c.786.193 3.304.562 2.896 2.19",
                    SolidColor(Color(0xFFFFFFFF)),
                    evenOdd = false,
                ),
            )
        Network.Ethereum ->
            listOf(
                Layer("M24 0H0v24h24z", SolidColor(Color(0xFF000000)), evenOdd = false),
                Layer("M12 4v5.912l5 2.237z", SolidColor(Color(0xFF8FFCF3)), evenOdd = false),
                Layer("m12 4-5 8.148 5-2.235z", SolidColor(Color(0xFFCABCF8)), evenOdd = false),
                Layer("M12 15.98V20l5-6.92z", SolidColor(Color(0xFFCBA7F5)), evenOdd = false),
                Layer("M12 20v-4.02l-5-2.9z", SolidColor(Color(0xFF74A0F3)), evenOdd = false),
                Layer("m12 15.049 5-2.9-5-2.236z", SolidColor(Color(0xFFCBA7F5)), evenOdd = false),
                Layer("m7 12.149 5 2.9V9.913z", SolidColor(Color(0xFF74A0F3)), evenOdd = false),
                Layer(
                    "m12 15.048-5-2.9L12 4l5 8.148zm-4.67-3.136 4.588-7.475v5.435zm-.068.204 4.656-2.068v4.768zm4.816-2.068v4.768l4.653-2.7zm0-.176 4.588 2.04-4.588-7.475z",
                    SolidColor(Color(0xFF202699)),
                    evenOdd = true,
                ),
                Layer(
                    "m12 15.917-5-2.84L12 20l5-6.924zm-4.44-2.341 4.36 2.48v3.56zm4.519 2.48v3.56l4.36-6.04z",
                    SolidColor(Color(0xFF202699)),
                    evenOdd = true,
                ),
            )
        Network.Bsc ->
            listOf(
                Layer("M24 0H0v24h24z", SolidColor(Color(0xFFF0B90B)), evenOdd = false),
                Layer(
                    "M7.635 6.449 12 4l4.364 2.449-1.6.907L12 5.809 9.24 7.356zm8.73 3.093-1.6-.906L12 10.182 9.24 8.636l-1.605.906v1.81l2.756 1.546v3.089l1.609.906 1.604-.906v-3.09l2.76-1.546zm0 4.903V12.63l-1.6.907v1.809zm1.141.64-2.76 1.542v1.813l4.365-2.453v-4.898l-1.605.902zm-1.604-7.09 1.6.907v1.81l1.609-.903V7.996l-1.605-.907L15.902 8zm-5.511 9.29v1.808L12 20l1.604-.907V17.29l-1.604.9-1.605-.907zM7.635 14.44l1.6.907v-1.81l-1.6-.906v1.813zm2.756-6.444L12 8.902l1.604-.906L12 7.089 10.395 8zm-3.898.906 1.605-.906-1.6-.907L4.888 8v1.813l1.605.903zm0 3.09-1.604-.903v4.898l4.364 2.453v-1.818L6.498 15.08v-3.093z",
                    SolidColor(Color(0xFFFFFFFF)),
                    evenOdd = false,
                ),
            )
        Network.Polygon ->
            listOf(
                Layer(
                    "M24 0H0v24h24z",
                    Brush.linearGradient(
                        0.0f to Color(0xFFA726C1),
                        0.88f to Color(0xFF803BDF),
                        1.0f to Color(0xFF7B3FE4),
                        start = Offset(3.948f, 16.617f),
                        end = Offset(19.217f, 7.645f),
                    ),
                    evenOdd = false,
                ),
                Layer(
                    "m15.88 14.86 3.794-2.165a.64.64 0 0 0 .326-.558v-4.33a.64.64 0 0 0-.326-.556L15.88 5.086a.66.66 0 0 0-.65 0L11.432 7.25a.64.64 0 0 0-.325.557v7.737l-2.662 1.517-2.661-1.517v-3.036l2.661-1.517 1.755 1.001V9.958l-1.43-.816a.66.66 0 0 0-.65 0l-3.796 2.165a.64.64 0 0 0-.325.557v4.33c0 .229.124.442.325.557l3.796 2.165c.2.114.45.114.65 0l3.796-2.165a.64.64 0 0 0 .325-.557V8.455l.048-.026 2.613-1.49 2.661 1.516v3.036l-2.661 1.517-1.753-.999v2.037l1.427.814a.66.66 0 0 0 .651 0z",
                    SolidColor(Color(0xFFFFFFFF)),
                    evenOdd = false,
                ),
            )
        Network.Tron ->
            listOf(
                Layer("M24 0H0v24h24z", SolidColor(Color(0xFFC4342B)), evenOdd = false),
                Layer(
                    "M5.261 4.165a.5.5 0 0 1 .492-.15l10.58 2.59a.5.5 0 0 1 .176.082l2.153 1.57a.504.504 0 0 1 .12.687L11.374 19.78a.5.5 0 0 1-.887-.116L5.161 4.669a.5.5 0 0 1 .1-.503m1.746 2.705 3.697 10.41.609-5.394zm5.296 5.19-.618 5.488 5.23-7.653zm5.084-3.494-3.37 1.582 2.258-2.392zm-2.13-1.194L7.032 5.361l4.823 5.616z",
                    SolidColor(Color(0xFFFFFFFF)),
                    evenOdd = true,
                ),
            )
        Network.Solana ->
            listOf(
                Layer(
                    "M24 0H0v24h24z",
                    Brush.linearGradient(
                        0.0f to Color(0xFF599DB0),
                        1.0f to Color(0xFF47F8C3),
                        start = Offset(4.001f, 50.258f),
                        end = Offset(20.408f, 50.108f),
                    ),
                    evenOdd = false,
                ),
                Layer(
                    "M17.7 8.358a.55.55 0 0 1-.365.145H4.515c-.455 0-.684-.52-.37-.825l2.106-2.03a.54.54 0 0 1 .364-.15h12.87c.46 0 .685.524.365.83zm0 10.005c-.1.09-.23.14-.365.14H4.515c-.455 0-.684-.516-.37-.82l2.106-2.036a.53.53 0 0 1 .364-.145h12.87c.46 0 .685.52.365.825zm0-7.72a.55.55 0 0 0-.365-.14H4.515c-.455 0-.684.515-.37.82l2.106 2.035c.1.091.23.143.364.144h12.87c.46 0 .685-.52.365-.824z",
                    SolidColor(Color(0xFFFFFFFF)),
                    evenOdd = false,
                ),
            )
    }

/** 路径来自 design/icons/assets。这里只有两个非原生币，原生币用网络图标。 */
private val UsdtLayers =
    listOf(
        Layer("M24 0H0v24h24z", SolidColor(Color(0xFF009393)), evenOdd = false),
        Layer(
            "m12 18.4-8-7.892L7.052 5.6h9.896L20 10.508zm.8-7.2v-.976c1.44.072 2.784.352 3.2.716-.484.424-2.216.732-4 .732s-3.516-.308-4-.732c.412-.364 1.76-.64 3.2-.72v.98zM8 10.936v.588c.412.364 1.756.64 3.2.72V14.4h1.6v-2.16c1.44-.072 2.788-.352 3.2-.716v-1.172c-.412-.364-1.76-.644-3.2-.72V8.8h2.4V7.6H8.8v1.2h2.4v.832c-1.444.076-2.788.356-3.2.72z",
            SolidColor(Color(0xFFFFFFFF)),
            evenOdd = false,
        ),
    )

private val UsdcLayers =
    listOf(
        Layer("M24 0H0v24h24z", SolidColor(Color(0xFF0B53BF)), evenOdd = false),
        Layer(
            "M12 20c4.435 0 8-3.565 8-8s-3.565-8-8-8-8 3.565-8 8 3.565 8 8 8m2.2-6.735c0-1.165-.7-1.565-2.1-1.73-1-.135-1.2-.4-1.2-.87 0-.465.335-.765 1-.765.6 0 .935.2 1.1.7.035.1.135.165.235.165h.53c.135 0 .235-.1.235-.23v-.036c-.135-.734-.735-1.434-1.5-1.5v-.734c0-.135-.1-.235-.265-.265h-.44c-.135 0-.26.1-.295.265V9c-1 .135-1.665.9-1.665 1.735 0 1.1.665 1.53 2.065 1.7.935.165 1.235.365 1.235.9 0 .53-.47.9-1.1.9-.87 0-1.17-.37-1.27-.87-.03-.13-.13-.2-.23-.2h-.57c-.13 0-.23.1-.23.235v.035c.13.83.665 1.4 1.765 1.565v.74c0 .135.1.224.265.26h.48c.13 0 .22-.09.255-.26V15c1-.165 1.7-.835 1.7-1.735m-5.566 2.49c.483.443 1.05.786 1.666 1.01.1.07.2.2.2.3v.47c0 .064 0 .1-.035.13-.03.134-.165.2-.3.134a6 6 0 0 1 0-11.435c.035-.03.1-.03.135-.03.135.03.2.13.2.265v.465c0 .17-.065.27-.2.335a4.93 4.93 0 0 0-2.965 2.965 4.965 4.965 0 0 0 1.299 5.391M13.535 6.5c.03-.135.165-.2.3-.135a6.05 6.05 0 0 1 3.9 3.935c1 3.165-.735 6.535-3.9 7.535-.035.03-.1.03-.135.03-.135-.03-.2-.13-.2-.265v-.465c0-.17.065-.27.2-.335a4.93 4.93 0 0 0 2.965-2.965 4.967 4.967 0 0 0-2.965-6.4c-.1-.07-.2-.2-.2-.335v-.465c0-.07 0-.1.035-.135",
            SolidColor(Color(0xFFFFFFFF)),
            evenOdd = true,
        ),
    )

@Composable
private fun LayeredIcon(layers: List<Layer>, size: Dp, modifier: Modifier = Modifier) {
    val parsed =
        remember(layers) {
            layers.map { layer ->
                val path = PathParser().parsePathString(layer.path).toPath()
                if (layer.evenOdd) path.fillType = PathFillType.EvenOdd
                ParsedLayer(path, layer.brush)
            }
        }
    Canvas(modifier.size(size).clip(CircleShape)) {
        scale(this.size.width / 24f, pivot = Offset.Zero) {
            parsed.forEach { drawPath(it.path, it.brush) }
        }
    }
}

@Composable
fun NetworkIcon(network: Network, size: Dp, modifier: Modifier = Modifier) {
    val parsed = remember(network) { layers(network) }
    LayeredIcon(parsed, size, modifier)
}

/** 不在内置清单的代币，虚线问号，见 docs/06-ui.md 第 6 节。 */
@Composable
fun UnlistedIcon(size: Dp, modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    val help = remember { PathParser().parsePathString("M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17h.01").toPath() }
    Canvas(modifier.size(size)) {
        val ratio = this.size.width / 24f
        scale(ratio, pivot = Offset.Zero) {
            drawCircle(
                c.text3,
                radius = 11f,
                center = Offset(12f, 12f),
                style = Stroke(1.5f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(3f, 3f))),
            )
            drawPath(help, c.text2, style = Stroke(1.8f, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

/** 内置清单里的原生币图标名。 */
private fun nativeIcon(name: String): Network? =
    when (name) {
        "btc" -> Network.Bitcoin
        "eth" -> Network.Ethereum
        "bnb" -> Network.Bsc
        "pol" -> Network.Polygon
        "trx" -> Network.Tron
        "sol" -> Network.Solana
        else -> null
    }

/** 金额的图标。[icon] 是内置清单的图标名，为空说明核心库按网络加合约没查到，一律问号。 清单里有但应用没打包的图标退回 [network] 的图标。代币在非原生链上时右下角加网络角标。 */
@Composable
fun AssetIcon(icon: String?, network: Network?, size: Dp, modifier: Modifier = Modifier) {
    val token =
        when (icon) {
            "usdt" -> UsdtLayers
            "usdc" -> UsdcLayers
            else -> null
        }
    val coin = icon?.let(::nativeIcon) ?: network
    Box(modifier.size(size)) {
        when {
            icon == null -> UnlistedIcon(size)
            token != null -> {
                LayeredIcon(token, size)
                if (network != null) NetworkIcon(network, size * 0.42f, Modifier.align(Alignment.BottomEnd))
            }
            coin != null -> NetworkIcon(coin, size)
            else -> UnlistedIcon(size)
        }
    }
}

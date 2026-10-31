package com.bitone.wallet.ui.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.bitone.wallet.platform.QrMatrix
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.FontSize
import com.bitone.wallet.ui.theme.LightColors
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Space

/** 四周静区的码元数，见界面规范 §4.5，真机扫码测试后再定。 */
private const val QUIET_MODULES = 2

/**
 * 动态二维码。任何主题下都是白底深色码点，所以固定用浅色令牌。
 *
 * 外框永远铺满可用宽度，版本不同的码切换时页面不跳。码元取整数像素保证码点清晰，放不满的部分是白边。
 *
 * 换帧不做任何过渡，半透明的中间帧扫不出来，帧序号和进度也跟着硬切。[total] 为 0 时是静态码，不显示帧进度。
 */
@Composable
fun QrView(matrix: QrMatrix?, frame: Int, total: Int, label: String, modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    BoxWithConstraints(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        val density = LocalDensity.current
        val sidePx = constraints.maxWidth
        val side = with(density) { sidePx.toDp() }
        val shape = RoundedCornerShape(Radius.r3)
        val bar = RoundedCornerShape(Radius.rf)
        Column(Modifier.width(side), verticalArrangement = Arrangement.spacedBy(Space.s3)) {
            if (matrix == null) {
                Skeleton(Modifier.size(side))
            } else {
                val modulePx = sidePx / (matrix.size + QUIET_MODULES * 2)
                val inset = (sidePx - modulePx * matrix.size) / 2
                Canvas(Modifier.size(side).clip(shape).semantics { contentDescription = label }) {
                    drawRect(LightColors.surface)
                    val module = Size(modulePx.toFloat(), modulePx.toFloat())
                    for (y in 0 until matrix.size) {
                        for (x in 0 until matrix.size) {
                            if (matrix.modules[y * matrix.size + x]) {
                                drawRect(
                                    LightColors.text,
                                    topLeft =
                                        Offset((inset + x * modulePx).toFloat(), (inset + y * modulePx).toFloat()),
                                    size = module,
                                )
                            }
                        }
                    }
                }
            }
            if (total > 0) {
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.s3),
                ) {
                    Box(Modifier.weight(1f).height(Space.s1).clip(bar).background(c.lineStrong)) {
                        Box(Modifier.fillMaxHeight().fillMaxWidth(frame.toFloat() / total).background(c.accent))
                    }
                    Txt("$frame/$total", size = FontSize.t2, color = c.text2)
                }
            }
        }
    }
}

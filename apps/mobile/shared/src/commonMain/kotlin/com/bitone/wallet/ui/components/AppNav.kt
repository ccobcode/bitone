package com.bitone.wallet.ui.components

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.bitone.wallet.ui.theme.BitOneTheme
import com.bitone.wallet.ui.theme.Motion
import com.bitone.wallet.ui.theme.Radius
import com.bitone.wallet.ui.theme.Size
import com.bitone.wallet.ui.theme.Space
import com.bitone.wallet.ui.theme.elevation

enum class NavItem(val label: String, val icon: String) {
    Wallet("钱包", Icons.Wallet),
    Receive("收款", Icons.Receive),
    Settings("设置", Icons.Sliders),
}

/** 悬浮胶囊，高度等于列表行高。 */
val AppNavHeight = Size.hRow

/** 底色块包住图标和文字，在项目之间滑动。未选中项也显示文字，切换时布局不跳动。 */
@Composable
fun AppNav(active: NavItem, onSelect: (NavItem) -> Unit, modifier: Modifier = Modifier) {
    val c = BitOneTheme.colors
    val shape = RoundedCornerShape(Radius.rf)
    SlidingTrack(
        count = NavItem.entries.size,
        selected = active.ordinal,
        modifier =
            modifier
                .fillMaxWidth()
                .height(AppNavHeight)
                .elevation(BitOneTheme.elevation.elevation2, shape)
                .background(c.surface, shape)
                .padding(Space.s1),
        indicator = { Box(Modifier.fillMaxSize().background(c.accentSoft, shape)) },
    ) {
        NavItem.entries.forEach { item ->
            val foreground by
                animateColorAsState(
                    if (item == active) c.accentText else c.text2,
                    stateSpec(Motion.fast),
                    label = "nav",
                )
            Row(
                Modifier.weight(1f).fillMaxHeight().tap(role = Role.Tab) { onSelect(item) },
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(Space.s2, Alignment.CenterHorizontally),
            ) {
                LineIcon(item.icon, 20.dp, foreground)
                Txt(item.label, weight = Medium, color = foreground)
            }
        }
    }
}

package com.ccobcode.bitone.ui.screens

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterExitState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.AppNav
import com.ccobcode.bitone.ui.components.AppNavHeight
import com.ccobcode.bitone.ui.components.NavItem
import com.ccobcode.bitone.ui.components.contentColumn
import com.ccobcode.bitone.ui.components.ignoreTouches
import com.ccobcode.bitone.ui.components.pageTransition
import com.ccobcode.bitone.ui.components.rememberLatest
import com.ccobcode.bitone.ui.theme.BitOneTheme
import com.ccobcode.bitone.ui.theme.Size
import com.ccobcode.bitone.ui.theme.Space

private val FadeHeight = Space.s6

/** 悬浮导航上下留的距离,页面内容要让开这一段. */
internal val NavSpace = AppNavHeight + FadeHeight

private val Tabs = listOf(Screen.Home, Screen.Receive, Screen.Settings)

/**
 * A3,A11 和 A10 是底部导航的三个标签页,共用一个外层,导航不随页面滑动,选中块才能在项目之间连续滑动.
 *
 * 抹除确认,语言选择和收款的账户选择也放在这一层,要盖住导航.
 */
@Composable
fun MainTabs(flow: WalletFlow, brightness: Brightness) {
    val tab = rememberLatest(flow.screen.takeIf { it in Tabs }) ?: Screen.Home
    val density = LocalDensity.current
    var confirming by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    var choosingLanguage by remember { mutableStateOf(false) }
    LaunchedEffect(tab) {
        confirming = false
        picking = false
        choosingLanguage = false
    }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        AnimatedContent(
            targetState = tab,
            transitionSpec = { pageTransition(back = Tabs.indexOf(targetState) < Tabs.indexOf(initialState), density) },
            label = "tab",
        ) { screen ->
            Box(Modifier.ignoreTouches(transition.targetState != EnterExitState.Visible)) {
                when (screen) {
                    Screen.Home -> HomeScreen(flow)
                    Screen.Receive -> ReceiveScreen(flow, brightness) { picking = true }
                    else -> SettingsScreen(flow, onLanguage = { choosingLanguage = true }) { confirming = true }
                }
            }
        }
        MainNav(flow, NavItem.entries[Tabs.indexOf(tab)])
        AccountSheet(picking, flow) { picking = false }
        LanguageSheet(choosingLanguage, flow) { choosingLanguage = false }
        WipeSheet(
            flow,
            confirming,
            onCancel = { confirming = false },
            onConfirm = {
                confirming = false
                flow.settings.startWipe()
            },
        )
    }
}

/** 底色从透明过渡到 `bg`,内容从导航后面滚过时不会和按钮叠在一起. */
@Composable
private fun MainNav(flow: WalletFlow, active: NavItem) {
    val bg = BitOneTheme.colors.bg
    Box(
        Modifier.fillMaxWidth()
            .background(Brush.verticalGradient(listOf(Color.Transparent, bg, bg)))
            .navigationBarsPadding()
            .padding(bottom = Space.s2),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(Modifier.contentColumn().padding(start = Size.gutter, end = Size.gutter, top = FadeHeight)) {
            AppNav(
                active,
                { item ->
                    when (item) {
                        NavItem.Wallet -> flow.openHome()
                        NavItem.Receive -> flow.receive.open()
                        NavItem.Settings -> flow.openSettings()
                    }
                },
            )
        }
    }
}

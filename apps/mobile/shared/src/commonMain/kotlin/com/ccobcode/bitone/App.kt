package com.ccobcode.bitone

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterExitState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import com.ccobcode.bitone.flow.Screen
import com.ccobcode.bitone.flow.WalletFlow
import com.ccobcode.bitone.platform.Brightness
import com.ccobcode.bitone.ui.components.ignoreTouches
import com.ccobcode.bitone.ui.components.pageTransition
import com.ccobcode.bitone.ui.screens.AddressScreen
import com.ccobcode.bitone.ui.screens.BackupScreen
import com.ccobcode.bitone.ui.screens.BootScreen
import com.ccobcode.bitone.ui.screens.CameraDeniedScreen
import com.ccobcode.bitone.ui.screens.ExportScreen
import com.ccobcode.bitone.ui.screens.FingerprintMismatchScreen
import com.ccobcode.bitone.ui.screens.ForbiddenScreen
import com.ccobcode.bitone.ui.screens.ImportOverwriteScreen
import com.ccobcode.bitone.ui.screens.ImportScreen
import com.ccobcode.bitone.ui.screens.MainTabs
import com.ccobcode.bitone.ui.screens.ManifestScreen
import com.ccobcode.bitone.ui.screens.NeedsRecoveryScreen
import com.ccobcode.bitone.ui.screens.ParseFailedScreen
import com.ccobcode.bitone.ui.screens.ReceiveAddressesScreen
import com.ccobcode.bitone.ui.screens.ReviewScreen
import com.ccobcode.bitone.ui.screens.ScanScreen
import com.ccobcode.bitone.ui.screens.SelfCheckFailedScreen
import com.ccobcode.bitone.ui.screens.SetPinScreen
import com.ccobcode.bitone.ui.screens.SignedScreen
import com.ccobcode.bitone.ui.screens.UnlockScreen
import com.ccobcode.bitone.ui.screens.VerifyScreen
import com.ccobcode.bitone.ui.screens.ViewWordsScreen
import com.ccobcode.bitone.ui.screens.WelcomeScreen
import com.ccobcode.bitone.ui.theme.BitOneFonts
import com.ccobcode.bitone.ui.theme.BitOneTheme

/** 新栈是旧栈的前缀才算返回.签名完成时栈也会变短,但那是前进. */
private fun isBack(from: List<Screen>, to: List<Screen>): Boolean =
    to.size < from.size && from.subList(0, to.size) == to

/** 底部导航的三个标签页共用一个外层,它们之间的切换在 [MainTabs] 里. */
private fun pageKey(screen: Screen): Screen =
    if (screen == Screen.Settings || screen == Screen.Receive) Screen.Home else screen

/** 共享界面的入口,各平台注入实现后调用. */
@Composable
fun App(flow: WalletFlow, brightness: Brightness, fonts: BitOneFonts = BitOneFonts()) {
    BitOneTheme(fonts) {
        val density = LocalDensity.current
        AnimatedContent(
            targetState = flow.stack,
            modifier = Modifier.fillMaxSize().background(BitOneTheme.colors.bg),
            transitionSpec = { pageTransition(isBack(initialState, targetState), density) },
            contentKey = { pageKey(it.last()) },
            label = "page",
        ) { stack ->
            Box(Modifier.ignoreTouches(transition.targetState != EnterExitState.Visible)) {
                when (stack.last()) {
                    Screen.Booting -> BootScreen(flow)
                    Screen.Welcome -> WelcomeScreen(flow)
                    Screen.ImportWords -> ImportScreen(flow)
                    Screen.Backup -> BackupScreen(flow)
                    Screen.Verify -> VerifyScreen(flow)
                    Screen.SetPin -> SetPinScreen(flow)
                    Screen.Home,
                    Screen.Receive,
                    Screen.Settings -> MainTabs(flow, brightness)
                    Screen.ReceiveAddresses -> ReceiveAddressesScreen(flow)
                    Screen.Unlock -> UnlockScreen(flow)
                    Screen.Export -> ExportScreen(flow, brightness)
                    Screen.Scan -> ScanScreen(flow, brightness)
                    Screen.Review -> ReviewScreen(flow)
                    Screen.Signed -> SignedScreen(flow, brightness)
                    Screen.CheckAddress -> AddressScreen(flow)
                    Screen.Manifest -> ManifestScreen(flow)
                    Screen.ViewWords -> ViewWordsScreen(flow)
                    Screen.SelfCheckFailed -> SelfCheckFailedScreen(flow)
                    Screen.FingerprintMismatch -> FingerprintMismatchScreen(flow)
                    Screen.ParseFailed -> ParseFailedScreen(flow)
                    Screen.Forbidden -> ForbiddenScreen(flow)
                    Screen.CameraDenied -> CameraDeniedScreen(flow)
                    Screen.NeedsRecovery -> NeedsRecoveryScreen(flow)
                    Screen.ImportOverwrite -> ImportOverwriteScreen(flow)
                }
            }
        }
    }
}

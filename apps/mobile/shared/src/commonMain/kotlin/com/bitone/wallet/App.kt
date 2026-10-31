package com.bitone.wallet

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterExitState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import com.bitone.wallet.flow.Screen
import com.bitone.wallet.flow.WalletFlow
import com.bitone.wallet.platform.Brightness
import com.bitone.wallet.ui.components.ignoreTouches
import com.bitone.wallet.ui.components.pageTransition
import com.bitone.wallet.ui.screens.AddressScreen
import com.bitone.wallet.ui.screens.BackupScreen
import com.bitone.wallet.ui.screens.BootScreen
import com.bitone.wallet.ui.screens.CameraDeniedScreen
import com.bitone.wallet.ui.screens.ExportScreen
import com.bitone.wallet.ui.screens.FingerprintMismatchScreen
import com.bitone.wallet.ui.screens.ForbiddenScreen
import com.bitone.wallet.ui.screens.ImportScreen
import com.bitone.wallet.ui.screens.MainTabs
import com.bitone.wallet.ui.screens.ManifestScreen
import com.bitone.wallet.ui.screens.NeedsRecoveryScreen
import com.bitone.wallet.ui.screens.ParseFailedScreen
import com.bitone.wallet.ui.screens.ReceiveAddressesScreen
import com.bitone.wallet.ui.screens.ReviewScreen
import com.bitone.wallet.ui.screens.ScanScreen
import com.bitone.wallet.ui.screens.SelfCheckFailedScreen
import com.bitone.wallet.ui.screens.SetPinScreen
import com.bitone.wallet.ui.screens.SignedScreen
import com.bitone.wallet.ui.screens.UnlockScreen
import com.bitone.wallet.ui.screens.VerifyScreen
import com.bitone.wallet.ui.screens.ViewWordsScreen
import com.bitone.wallet.ui.screens.WelcomeScreen
import com.bitone.wallet.ui.screens.WipedScreen
import com.bitone.wallet.ui.theme.BitOneFonts
import com.bitone.wallet.ui.theme.BitOneTheme

/** 新栈是旧栈的前缀才算返回。签名完成时栈也会变短，但那是前进。 */
private fun isBack(from: List<Screen>, to: List<Screen>): Boolean =
    to.size < from.size && from.subList(0, to.size) == to

/** 底部导航的三个标签页共用一个外层，它们之间的切换在 [MainTabs] 里。 */
private fun pageKey(screen: Screen): Screen =
    if (screen == Screen.Settings || screen == Screen.Receive) Screen.Home else screen

/** 共享界面的入口，各平台注入实现后调用。 */
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
                    Screen.Scan -> ScanScreen(flow)
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
                    Screen.Wiped -> WipedScreen(flow)
                }
            }
        }
    }
}

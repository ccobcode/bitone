package com.ccobcode.bitone.platform

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import android.util.Size
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.LifecycleOwner
import java.util.concurrent.Executors
import kotlin.coroutines.resume
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import zxingcpp.BarcodeReader

/** 动态二维码的码点很密，分析分辨率不能太低。 */
private val ANALYSIS_SIZE = Size(1920, 1080)

/** CameraX 取帧，zxing-cpp 解码。全程在本机，不用 Google Play 服务。 */
class AndroidQrScanner(private val context: Context, private val current: CurrentActivity) : QrScanner {

    override fun hasPermission(): Boolean =
        context.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    override suspend fun requestPermission(): Boolean =
        withContext(Dispatchers.Main) {
            val activity = current.activity as? ComponentActivity ?: return@withContext false
            suspendCancellableCoroutine { continuation ->
                lateinit var launcher: ActivityResultLauncher<String>
                launcher =
                    activity.activityResultRegistry.register(
                        "bitone.camera",
                        ActivityResultContracts.RequestPermission(),
                    ) { granted ->
                        launcher.unregister()
                        continuation.resume(granted)
                    }
                continuation.invokeOnCancellation { launcher.unregister() }
                launcher.launch(Manifest.permission.CAMERA)
            }
        }

    override fun openSettings() {
        val activity = current.activity ?: return
        activity.startActivity(
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
        )
    }

    @Composable
    override fun Viewfinder(modifier: Modifier, onCode: (String) -> Unit, onFailure: () -> Unit) {
        val latestCode by rememberUpdatedState(onCode)
        val latestFailure by rememberUpdatedState(onFailure)
        val owner = current.activity as? LifecycleOwner
        // TextureView 画在窗口内容里，受 FLAG_SECURE 保护。SurfaceView 是独立的层。
        val viewContext = LocalContext.current
        val preview = remember {
            PreviewView(viewContext).apply {
                implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                scaleType = PreviewView.ScaleType.FILL_CENTER
            }
        }
        var provider by remember { mutableStateOf<ProcessCameraProvider?>(null) }
        val analysisThread = remember { Executors.newSingleThreadExecutor() }

        DisposableEffect(Unit) {
            onDispose {
                provider?.unbindAll()
                analysisThread.shutdown()
            }
        }
        LaunchedEffect(owner) {
            try {
                val cameras = ProcessCameraProvider.awaitInstance(context)
                provider = cameras
                val analysis =
                    ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .setResolutionSelector(
                            ResolutionSelector.Builder()
                                .setResolutionStrategy(
                                    ResolutionStrategy(
                                        ANALYSIS_SIZE,
                                        ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER,
                                    )
                                )
                                .build()
                        )
                        .build()
                val reader =
                    BarcodeReader(
                        BarcodeReader.Options(formats = setOf(BarcodeReader.Format.QR_CODE), tryHarder = true)
                    )
                analysis.setAnalyzer(analysisThread) { image ->
                    val codes = image.use { reader.read(it) }
                    codes
                        .firstNotNullOfOrNull { it.text?.takeIf(String::isNotEmpty) }
                        ?.let { text ->
                            context.mainExecutor.execute { latestCode(text) }
                        }
                }
                val feed = Preview.Builder().build().also { it.surfaceProvider = preview.surfaceProvider }
                cameras.unbindAll()
                cameras.bindToLifecycle(requireNotNull(owner), CameraSelector.DEFAULT_BACK_CAMERA, feed, analysis)
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // 没有后置摄像头、被别的应用占用、CameraX 初始化失败都走这里。
                latestFailure()
            }
        }
        LaunchedEffect(Unit) { feedDevelopmentFrames(context) { latestCode(it) } }

        AndroidView(factory = { preview }, modifier = modifier)
    }
}

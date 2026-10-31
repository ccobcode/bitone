import com.android.build.api.variant.AndroidComponentsExtension
import javax.xml.parsers.DocumentBuilderFactory

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

// 发布时由 make 传入,和 tag 一致.versionCode 由它算出,只增不减,系统才允许覆盖安装.
val appVersion = providers.gradleProperty("appVersion").getOrElse("0.1.0")

android {
    namespace = "com.ccobcode.bitone"
    compileSdk = libs.versions.android.compileSdk.get().toInt()

    defaultConfig {
        applicationId = "com.ccobcode.bitone"
        minSdk = libs.versions.android.minSdk.get().toInt()
        targetSdk = libs.versions.android.targetSdk.get().toInt()
        versionCode = appVersion.split(".").fold(0) { code, part -> code * 100 + part.toInt() }
        versionName = appVersion
        // 不打包 32 位的 JNA,Rust 核心也只构建这两个架构.
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildTypes {
        release {
            vcsInfo.include = false
            // 只裁剪不混淆,规则见 proguard-rules.pro.发布包小一半,冷启动少加载一半代码.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    buildFeatures { buildConfig = false }
    packaging {
        // 保持 .so 原样,不随 NDK 版本的 strip 结果变化.
        jniLibs.keepDebugSymbols += "**/*.so"
    }
    // 为可复现构建做准备:不写入构建环境信息.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }
}

// 这个映射文件只用来还原混淆后的 Compose 堆栈.这里不混淆,也就不用多下载一个依赖.
composeCompiler { includeComposeMappingFile = false }

dependencies {
    implementation(project(":shared"))
    implementation(libs.androidx.activity.compose)
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.jna) { artifact { type = "aar" } }
    // 扫码:相机用 CameraX,解码用 zxing-cpp,都在本机完成.不用 Google Play 服务和 ML Kit.
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.zxing.cpp)
    testImplementation(libs.kotlin.test.junit)
}

// 关闭 baseline profile,见 docs/08 §5.profileinstaller 由 Compose 传递进来,运行时不需要.
// camera-view 带进来录像栈(camera-video,media3,Guava)和 ACCESS_NETWORK_STATE.这里只用 PreviewView,都不需要.
configurations.configureEach {
    exclude(group = "androidx.profileinstaller", module = "profileinstaller")
    exclude(group = "androidx.camera", module = "camera-video")
    exclude(group = "androidx.media3")
}

tasks.configureEach {
    if (name.contains("ArtProfile", ignoreCase = true) || name.contains("BaselineProfile", ignoreCase = true)) {
        enabled = false
    }
}

/** merged manifest 里不能出现这些权限.任何依赖带进来都会让构建失败. */
@CacheableTask
abstract class CheckManifestPermissions : DefaultTask() {
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val manifest: RegularFileProperty

    @get:Input abstract val forbidden: ListProperty<String>

    @get:OutputFile abstract val report: RegularFileProperty

    @TaskAction
    fun check() {
        val document =
            DocumentBuilderFactory.newInstance()
                .apply { isNamespaceAware = true }
                .newDocumentBuilder()
                .parse(manifest.get().asFile)
        val androidNs = "http://schemas.android.com/apk/res/android"
        val declared =
            listOf("uses-permission", "uses-permission-sdk-23").flatMap { tag ->
                val nodes = document.getElementsByTagName(tag)
                (0 until nodes.length).map { (nodes.item(it) as org.w3c.dom.Element).getAttributeNS(androidNs, "name") }
            }
        val hit = declared.filter { it in forbidden.get() }
        if (hit.isNotEmpty()) throw GradleException("merged manifest 含有禁止的权限:$hit")
        report.get().asFile.writeText(declared.sorted().joinToString("\n", postfix = "\n"))
    }
}

val forbiddenPermissions =
    listOf(
            "INTERNET",
            "ACCESS_NETWORK_STATE",
            "CHANGE_NETWORK_STATE",
            "CHANGE_WIFI_STATE",
            "BLUETOOTH",
            "BLUETOOTH_ADMIN",
            "BLUETOOTH_ADVERTISE",
            "BLUETOOTH_CONNECT",
            "BLUETOOTH_SCAN",
            "NFC",
        )
        .map { "android.permission.$it" }

extensions.configure<AndroidComponentsExtension<*, *, *>>("androidComponents") {
    onVariants { variant ->
        val suffix = variant.name.replaceFirstChar(Char::uppercase)
        val check =
            tasks.register<CheckManifestPermissions>("check${suffix}ManifestPermissions") {
                manifest.set(variant.artifacts.get(com.android.build.api.artifact.SingleArtifact.MERGED_MANIFEST))
                forbidden.set(forbiddenPermissions)
                report.set(layout.buildDirectory.file("reports/manifest-permissions/$suffix.txt"))
            }
        // assemble 时就检查,不靠人记得跑.
        tasks.matching { it.name == "assemble$suffix" }.configureEach { dependsOn(check) }
        tasks.matching { it.name == "check" }.configureEach { dependsOn(check) }
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.kmp.library) apply false
    alias(libs.plugins.kotlin.multiplatform) apply false
    alias(libs.plugins.compose.multiplatform) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.spotless)
}

// 与根目录的 Prettier 配置一致：4 空格，120 列。生成文件不格式化。
spotless {
    val ktfmtVersion = libs.versions.ktfmt.get()
    kotlin {
        target("**/*.kt")
        targetExclude("**/build/**", "**/uniffi/**", "**/Tokens.kt", "**/Translations.kt", "**/IconPaths.kt")
        ktfmt(ktfmtVersion).kotlinlangStyle().configure { it.setMaxWidth(120) }
    }
    kotlinGradle {
        target("**/*.gradle.kts")
        targetExclude("**/build/**")
        ktfmt(ktfmtVersion).kotlinlangStyle().configure { it.setMaxWidth(120) }
    }
}

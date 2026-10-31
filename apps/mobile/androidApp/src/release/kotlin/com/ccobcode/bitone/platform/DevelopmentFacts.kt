package com.ccobcode.bitone.platform

import android.content.Context

// release 构建不跳过任何检查,也不接受软件密钥库.

@Suppress("UNUSED_PARAMETER") internal fun softwareKeystoreAllowed(context: Context): Boolean = false

@Suppress("UNUSED_PARAMETER")
internal fun adjustFactsForDevelopment(context: Context, facts: DeviceFacts): DeviceFacts = facts

@Suppress("UNUSED_PARAMETER", "RedundantSuspendModifier")
internal suspend fun feedDevelopmentFrames(context: Context, onCode: (String) -> Unit) = Unit

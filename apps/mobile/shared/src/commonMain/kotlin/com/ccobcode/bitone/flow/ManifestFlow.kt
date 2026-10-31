package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.ManifestEntry
import kotlinx.coroutines.withContext

/** A12 查看内置清单.扫码更新属于扫码流程. */
class ManifestFlow internal constructor(private val host: FlowHost) {
    private val core = host.platform.core
    private val session = host.session
    private val work = host.work

    var version by mutableStateOf<UInt?>(null)
        private set

    var entries by mutableStateOf(emptyList<ManifestEntry>())
        private set

    /** A10 只进这一页,"更新清单"才打开相机. */
    fun open() {
        if (host.screen == Screen.Settings) host.push(Screen.Manifest)
    }

    suspend fun loadVersion() {
        version = withContext(work) { core.manifestVersion(session.manifestPackage()) }
    }

    /** 版本号和代币列表都来自同一份清单包. */
    suspend fun load() {
        val (loadedVersion, loadedEntries) =
            withContext(work) {
                val bytes = session.manifestPackage()
                core.manifestVersion(bytes) to core.manifestEntries(bytes)
            }
        version = loadedVersion
        entries = loadedEntries
    }

    internal fun clear() {
        entries = emptyList()
    }
}

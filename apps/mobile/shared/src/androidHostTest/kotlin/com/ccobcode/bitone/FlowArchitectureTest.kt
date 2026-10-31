package com.ccobcode.bitone

import java.io.File
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** 流程的分层约定.子流程只认识 FlowHost,互相不知道对方,牵涉几个子流程的事由 WalletFlow 协调. */
class FlowArchitectureTest {
    private val root = "WalletFlow"
    private val directory = File("src/commonMain/kotlin/com/ccobcode/bitone/flow")

    /** 去掉注释.注释可以提到别的子流程. */
    private fun code(file: File): String =
        file.readText().replace(Regex("/\\*.*?\\*/", RegexOption.DOT_MATCHES_ALL), "").replace(Regex("//.*"), "")

    private val files: List<File>
        get() = directory.listFiles().orEmpty().filter { it.extension == "kt" }

    private val subflows: List<String>
        get() = files.map { it.nameWithoutExtension }.filter { it.endsWith("Flow") && it != root }

    @Test
    fun subflowsAreFound() {
        assertTrue(subflows.size >= 7, subflows.toString())
    }

    @Test
    fun subflowsDoNotKnowEachOther() {
        for (file in files.filter { it.nameWithoutExtension != root }) {
            val text = code(file)
            for (name in subflows + root) {
                if (name == file.nameWithoutExtension) continue
                val found = Regex("\\b$name\\b").containsMatchIn(text)
                assertFalse(found, "${file.name} 不能引用 $name")
            }
        }
    }
}

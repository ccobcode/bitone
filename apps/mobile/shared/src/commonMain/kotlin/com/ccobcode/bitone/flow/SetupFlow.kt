package com.ccobcode.bitone.flow

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.ccobcode.bitone.platform.HardwareKeyException
import com.ccobcode.bitone.platform.QuizItem
import com.ccobcode.bitone.platform.WalletCoreException
import com.ccobcode.bitone.platform.WalletDraft
import com.ccobcode.bitone.platform.WalletMeta
import kotlinx.coroutines.withContext

private const val QUIZ_ROUNDS = 3
private const val SUGGESTION_LIMIT = 2

enum class VerifyState {
    Pending,
    Passed,
    Failed,
}

enum class ChecksumState {
    Incomplete,
    Valid,
    Invalid,
}

/** A1 到 A2c,新建或导入钱包,设好 PIN 后落盘. */
class SetupFlow internal constructor(private val host: FlowHost, private val pin: PinEntry) {
    private val core = host.platform.core
    private val store = host.platform.store
    private val hardwareKey = host.platform.hardwareKey
    private val work = host.work

    private var draft: WalletDraft? = null
    private var verified = false

    var wordCount by mutableStateOf(12)
        private set

    var backupWords by mutableStateOf(emptyList<String>())
        private set

    var passphraseEnabled by mutableStateOf(false)
        private set

    var quiz by mutableStateOf(emptyList<QuizItem>())
        private set

    var picks by mutableStateOf(emptyMap<Int, String>())
        private set

    var verifyState by mutableStateOf(VerifyState.Pending)
        private set

    // A1b

    var importedWords by mutableStateOf(emptyList<String>())
        private set

    var importBuffer by mutableStateOf("")
        private set

    var suggestions by mutableStateOf(emptyList<String>())
        private set

    var checksum by mutableStateOf(ChecksumState.Incomplete)
        private set

    val bufferInvalid: Boolean
        get() = importBuffer.isNotEmpty() && suggestions.isEmpty()

    val importComplete: Boolean
        get() = importedWords.size >= wordCount

    /** 正在改的那个词.点已填的词回去改,不用删掉后面的词. */
    var importEditing by mutableStateOf<Int?>(null)
        private set

    /** 正在输入的序号. */
    val importCurrent: Int
        get() = importEditing ?: importedWords.size

    /** 导入的钱包走 A1b 到 A2c,步骤标记只用于新建. */
    val isImportFlow: Boolean
        get() = draft?.imported == true

    // A1

    /** 新建钱包.自检不通过时停在 AS. */
    fun startCreate() = host.guarded {
        if (!host.recheckNewWallet()) return@guarded
        replaceDraft(withContext(work) { core.generateDraft(12) }, 12)
        passphraseEnabled = false
        loadBackupWords()
        host.push(Screen.Backup)
    }

    /** 导入是找回资金的路,自检不过时确认后可以继续.本机还有密文时先提醒:导入成功后它就没了,输错助记词也找不回来. */
    fun startImport() = host.operation {
        if (withContext(work) { store.readVault() != null }) {
            host.push(Screen.ImportOverwrite)
        } else {
            resetImportEntry()
            host.push(Screen.ImportWords)
        }
    }

    /** 提醒页的"继续导入". */
    fun confirmOverwrite() {
        if (host.busy) return
        resetImportEntry()
        host.showOnly(Screen.ImportWords)
    }

    private fun resetImportEntry() {
        resetImport()
        passphraseEnabled = false
    }

    private fun replaceDraft(next: WalletDraft?, count: Int) {
        draft?.close()
        draft = next
        wordCount = count
        verified = false
    }

    // A2

    private suspend fun loadBackupWords() {
        val current = draft ?: return
        backupWords = withContext(work) { current.words() }
    }

    /** 换长度就是换一组新助记词,已抄写的作废. */
    fun setBackupWordCount(count: Int) = host.guarded {
        if (count == wordCount || !host.recheckNewWallet()) return@guarded
        replaceDraft(withContext(work) { core.generateDraft(count) }, count)
        loadBackupWords()
    }

    fun togglePassphrase() {
        passphraseEnabled = !passphraseEnabled
    }

    /** 生成新钱包途中自检一直不过时放弃,回到 A1,那里还能导入. */
    fun abandonCreate() {
        if (host.busy) return
        quiz = emptyList()
        picks = emptyMap()
        verifyState = VerifyState.Pending
        backupWords = emptyList()
        pin.reset()
        replaceDraft(null, 12)
        host.stack = listOf(Screen.Welcome)
    }

    internal fun leaveCreate() {
        backupWords = emptyList()
        replaceDraft(null, 12)
        host.pop()
    }

    // A2b

    fun startVerify() = host.guarded {
        val current = draft ?: return@guarded
        quiz = withContext(work) { current.quiz(QUIZ_ROUNDS) }
        picks = emptyMap()
        verifyState = VerifyState.Pending
        verified = false
        // 进入验证后助记词不再留在界面状态里.
        backupWords = emptyList()
        host.push(Screen.Verify)
    }

    /** 全部答完才判定.判错后选项锁定,只能回 A2,防止逐个试出答案. */
    fun pick(index: Int, word: String) {
        if (verifyState == VerifyState.Failed || host.busy) return
        picks = picks + (index to word)
        if (picks.size < quiz.size) {
            verifyState = VerifyState.Pending
            return
        }
        val answers = picks.toMap()
        val current = draft ?: return
        host.guarded {
            val correct = withContext(work) { answers.all { (i, w) -> current.checkWord(i, w) } }
            verifyState = if (correct) VerifyState.Passed else VerifyState.Failed
        }
    }

    fun backToBackup() = host.guarded {
        if (!host.recheckNewWallet()) return@guarded
        quiz = emptyList()
        picks = emptyMap()
        verifyState = VerifyState.Pending
        verified = false
        loadBackupWords()
        host.pop()
    }

    fun continueAfterVerify() {
        if (verifyState != VerifyState.Passed) return
        verified = true
        pin.reset()
        host.push(Screen.SetPin)
    }

    // A1b

    fun setImportWordCount(count: Int) {
        if (host.busy || count == wordCount) return
        wordCount = count
        importedWords = importedWords.take(count)
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        refreshChecksum()
    }

    fun importLetter(letter: Char) {
        if (host.busy || (importComplete && importEditing == null)) return
        importBuffer += letter
        suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
    }

    fun importDelete() {
        if (host.busy) return
        when {
            importBuffer.isNotEmpty() -> {
                importBuffer = importBuffer.dropLast(1)
                suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
            }
            // 改词时删空了再按一次是放弃修改,原来的词还在.
            importEditing != null -> {
                importEditing = null
                refreshChecksum()
            }
            importedWords.isNotEmpty() -> {
                importedWords = importedWords.dropLast(1)
                refreshChecksum()
            }
        }
    }

    /** 回去改已经填好的词.这个词放回输入区,其余的词不动.再点一次放弃修改. */
    fun importEdit(index: Int) {
        if (host.busy || index !in importedWords.indices) return
        importBuffer = ""
        suggestions = emptyList()
        if (importEditing == index) {
            importEditing = null
            refreshChecksum()
            return
        }
        importEditing = index
        importBuffer = importedWords[index]
        suggestions = core.wordSuggestions(importBuffer, SUGGESTION_LIMIT)
        replaceDraft(null, wordCount)
        checksum = ChecksumState.Incomplete
    }

    fun importPick(word: String) {
        val editing = importEditing
        if (host.busy || (importComplete && editing == null) || !core.isValidWord(word)) return
        importedWords =
            if (editing != null) importedWords.toMutableList().also { it[editing] = word } else importedWords + word
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        refreshChecksum()
    }

    private fun refreshChecksum() {
        replaceDraft(null, wordCount)
        if (!importComplete) {
            checksum = ChecksumState.Incomplete
            return
        }
        val words = importedWords
        host.guarded {
            try {
                val next = withContext(work) { core.importDraft(words) }
                replaceDraft(next, wordCount)
                checksum = ChecksumState.Valid
            } catch (_: WalletCoreException) {
                checksum = ChecksumState.Invalid
            }
        }
    }

    /** 校验和错误时阻止,见 docs/02. */
    fun continueAfterImport() {
        if (checksum != ChecksumState.Valid || draft?.imported != true) return
        pin.reset()
        host.push(Screen.SetPin)
    }

    private fun resetImport() {
        replaceDraft(null, 12)
        importedWords = emptyList()
        importEditing = null
        importBuffer = ""
        suggestions = emptyList()
        checksum = ChecksumState.Incomplete
    }

    internal fun leaveImport() {
        resetImport()
        host.pop()
    }

    // A2c

    /** 新建的钱包必须验证过才能设 PIN,导入的钱包跳过验证. */
    private fun mayFinishSetup(): Boolean {
        val current = draft ?: return false
        return current.imported || verified
    }

    /** 建钱包的最后一步:硬件算出这个 PIN 的结果,Rust 用两者一起加密,落盘后才进首页. */
    fun finishSetup() {
        if (!pin.done || !mayFinishSetup()) return
        val current = draft ?: return
        val entered = pin.input
        val passphrase = passphraseEnabled
        host.guarded {
            pin.error = null
            // 抄写和设 PIN 可能停留很久,加密落盘之前再自检一次.
            if (!(if (current.imported) host.recheck() else host.recheckNewWallet())) return@guarded
            try {
                val saved =
                    withContext(work) {
                        // 恢复时旧密钥可能已失效,新钱包用新密钥.
                        hardwareKey.delete()
                        val tag = hardwareKey.enroll(entered)
                        val vault = current.seal(entered, tag)
                        // 启用 BIP39 密码时不算指纹和公钥:这里算出的不是用户真正的钱包,也不把密码派生的结果写盘.
                        var fingerprint: String? = null
                        var keys: ByteArray? = null
                        if (!passphrase) {
                            val wallet = core.open(vault, entered, tag, "")
                            try {
                                fingerprint = wallet.fingerprint()
                                keys = wallet.publicKeys(current.imported)
                            } finally {
                                wallet.close()
                            }
                        }
                        val next = WalletMeta(current.imported, passphrase, fingerprint)
                        store.saveWallet(vault, next, keys)
                        next
                    }
                host.walletSaved(saved)
                pin.reset()
                backupWords = emptyList()
                importedWords = emptyList()
                quiz = emptyList()
                picks = emptyMap()
                replaceDraft(null, 12)
                host.stack = listOf(Screen.Home)
            } catch (_: HardwareKeyException.AuthCancelled) {
                pin.error = "需要确认系统屏幕锁"
            } catch (_: HardwareKeyException) {
                host.recheck()
                pin.error = "硬件密钥不可用"
            } catch (_: WalletCoreException) {
                pin.error = "保存失败,请重试"
            }
        }
    }
}

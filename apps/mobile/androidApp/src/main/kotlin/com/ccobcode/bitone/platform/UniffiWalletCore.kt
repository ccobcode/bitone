package com.ccobcode.bitone.platform

import uniffi.bitone_ffi.WalletException
import uniffi.bitone_ffi.accountRows as ffiAccountRows
import uniffi.bitone_ffi.isBoundVault as ffiIsBoundVault
import uniffi.bitone_ffi.isValidWord as ffiIsValidWord
import uniffi.bitone_ffi.isWeakPin as ffiIsWeakPin
import uniffi.bitone_ffi.manifestEntries as ffiManifestEntries
import uniffi.bitone_ffi.manifestVersion as ffiManifestVersion
import uniffi.bitone_ffi.qrMatrix as ffiQrMatrix
import uniffi.bitone_ffi.qrText as ffiQrText
import uniffi.bitone_ffi.receiveAddress as ffiReceiveAddress
import uniffi.bitone_ffi.reviewAddress as ffiReviewAddress
import uniffi.bitone_ffi.reviewRequest as ffiReviewRequest
import uniffi.bitone_ffi.verifyManifest as ffiVerifyManifest
import uniffi.bitone_ffi.wordSuggestions as ffiWordSuggestions

/** 包住生成的 UniFFI 绑定,把 Rust 的错误换成共享层的异常. */
class UniffiWalletCore : WalletCore {
    override fun generateDraft(wordCount: Int): WalletDraft = mapped {
        Draft(uniffi.bitone_ffi.WalletDraft.generate(wordCount.toUInt()))
    }

    override fun importDraft(words: List<String>): WalletDraft = mapped {
        Draft(uniffi.bitone_ffi.WalletDraft.importWords(words))
    }

    override fun open(sealed: ByteArray, pin: String, tag: ByteArray, passphrase: String): UnlockedWallet = mapped {
        Unlocked(uniffi.bitone_ffi.UnlockedWallet.open(sealed, pin, tag, passphrase))
    }

    override fun openLegacy(sealed: ByteArray, pin: String, passphrase: String): UnlockedWallet = mapped {
        Unlocked(uniffi.bitone_ffi.UnlockedWallet.openLegacy(sealed, pin, passphrase))
    }

    override fun isBoundVault(vault: ByteArray): Boolean = ffiIsBoundVault(vault)

    override fun qrMatrix(part: String): QrMatrix = mapped {
        val matrix = ffiQrMatrix(part)
        QrMatrix(matrix.size.toInt(), matrix.modules.toBooleanArray())
    }

    override fun qrText(text: String): QrMatrix = mapped {
        val matrix = ffiQrText(text)
        QrMatrix(matrix.size.toInt(), matrix.modules.toBooleanArray())
    }

    override fun isWeakPin(pin: String): Boolean = ffiIsWeakPin(pin)

    override fun isValidWord(word: String): Boolean = ffiIsValidWord(word)

    override fun wordSuggestions(prefix: String, limit: Int): List<String> = ffiWordSuggestions(prefix, limit.toUInt())

    override fun newReceiver(): FrameReceiver = Receiver(uniffi.bitone_ffi.FrameReceiver())

    override fun reviewRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): ReviewResult = mapped { ffiReviewRequest(message, keys, highestChange.ffi(), manifest).shared() }

    override fun reviewAddress(message: ByteArray, keys: ByteArray): AddressReview = mapped {
        val review = ffiReviewAddress(message, keys)
        AddressReview(
            review.belongs,
            review.network,
            review.path,
            review.address,
            review.derived,
            review.kind,
            review.fingerprintMismatch,
            review.requestFingerprint,
        )
    }

    override fun accountRows(keys: ByteArray): List<AccountAddress> = mapped {
        ffiAccountRows(keys).map { AccountAddress(it.kind, it.path, it.address) }
    }

    override fun receiveAddress(keys: ByteArray, accountPath: String, index: UInt): String = mapped {
        ffiReceiveAddress(keys, accountPath, index)
    }

    override fun verifyManifest(bytes: ByteArray, currentVersion: UInt): UInt = mapped {
        ffiVerifyManifest(bytes, currentVersion)
    }

    override fun manifestVersion(bytes: ByteArray?): UInt = mapped { ffiManifestVersion(bytes) }

    override fun manifestEntries(bytes: ByteArray?): List<ManifestEntry> = mapped {
        ffiManifestEntries(bytes).map {
            ManifestEntry(it.network, it.networkLabel, it.symbol, it.contract, it.icon)
        }
    }
}

private fun ChangeIndexes.ffi() = uniffi.bitone_ffi.ChangeIndexes(mainnet, testnet)

private fun uniffi.bitone_ffi.ReviewResult.shared() =
    ReviewResult(
        risk = risk.shared(),
        confirm =
            when (confirm) {
                uniffi.bitone_ffi.ConfirmMode.TAP -> ConfirmMode.Tap
                uniffi.bitone_ffi.ConfirmMode.HOLD -> ConfirmMode.Hold
                uniffi.bitone_ffi.ConfirmMode.REFUSE -> ConfirmMode.Refuse
            },
        network = network,
        title = title,
        amount = amount?.let { ReviewAmount(it.text, it.symbol, it.asset, it.icon, it.unlimited) },
        counterpartyLabel = counterpartyLabel,
        counterparty = counterparty,
        alerts = alerts,
        actions = actions.map { ReviewAction(it.risk.shared(), it.title, it.rows.shared()) },
        rows = rows.shared(),
        raw = raw.map { RawLine(it.text, it.note) },
        fingerprintMismatch = fingerprintMismatch,
        requestFingerprint = requestFingerprint,
    )

private fun uniffi.bitone_ffi.RiskLevel.shared() =
    when (this) {
        uniffi.bitone_ffi.RiskLevel.TRANSFER -> Risk.Transfer
        uniffi.bitone_ffi.RiskLevel.RECOGNIZED -> Risk.Recognized
        uniffi.bitone_ffi.RiskLevel.UNKNOWN -> Risk.Unknown
        uniffi.bitone_ffi.RiskLevel.APPROVAL -> Risk.Approval
        uniffi.bitone_ffi.RiskLevel.PERMISSION_CHANGE -> Risk.PermissionChange
        uniffi.bitone_ffi.RiskLevel.FORBIDDEN -> Risk.Forbidden
    }

private fun List<uniffi.bitone_ffi.ReviewRow>.shared() = map { ReviewRow(it.label, it.value, it.mono) }

private class Draft(private val inner: uniffi.bitone_ffi.WalletDraft) : WalletDraft {
    override val imported: Boolean
        get() = inner.imported()

    override fun words(): List<String> = mapped { inner.words() }

    override fun quiz(rounds: Int): List<QuizItem> = mapped {
        inner.quiz(rounds.toUInt()).map { QuizItem(it.index.toInt(), it.options) }
    }

    override fun checkWord(index: Int, word: String): Boolean = mapped { inner.checkWord(index.toUInt(), word) }

    override fun seal(pin: String, tag: ByteArray): ByteArray = mapped { inner.seal(pin, tag) }

    override fun close() = inner.close()
}

private class Unlocked(private val inner: uniffi.bitone_ffi.UnlockedWallet) : UnlockedWallet {
    override fun fingerprint(): String = mapped { inner.fingerprint() }

    override fun exportAccounts(imported: Boolean): FrameStream = mapped {
        Stream(inner.exportAccounts(imported))
    }

    override fun words(): List<String> = mapped { inner.words() }

    override fun seal(pin: String, tag: ByteArray): ByteArray = mapped { inner.seal(pin, tag) }

    override fun publicKeys(imported: Boolean): ByteArray = mapped { inner.publicKeys(imported) }

    override fun signRequest(
        message: ByteArray,
        keys: ByteArray,
        highestChange: ChangeIndexes,
        manifest: ByteArray?,
    ): SignOutcome = mapped {
        val outcome = inner.signRequest(message, keys, highestChange.ffi(), manifest)
        SignOutcome(Stream(outcome.frames), outcome.changeIndex, outcome.changeIsTestnet)
    }

    override fun close() = inner.close()
}

private class Receiver(private val inner: uniffi.bitone_ffi.FrameReceiver) : FrameReceiver {
    override fun receive(part: String): Float = mapped { inner.receive(part) }

    override val complete: Boolean
        get() = inner.complete()

    override val urType: String?
        get() = inner.urType()

    override val fragmentCount: Int
        get() = inner.fragmentCount().toInt()

    override fun message(): ByteArray = mapped { inner.message() }

    override fun close() = inner.close()
}

private class Stream(private val inner: uniffi.bitone_ffi.FrameStream) : FrameStream {
    override val fragmentCount: Int
        get() = inner.fragmentCount().toInt()

    override fun nextPart(): String = mapped { inner.nextPart() }

    override fun close() = inner.close()
}

private inline fun <T> mapped(block: () -> T): T =
    try {
        block()
    } catch (e: WalletException) {
        throw when (e) {
            is WalletException.InvalidMnemonic -> WalletCoreException.InvalidMnemonic()
            is WalletException.InvalidPin -> WalletCoreException.InvalidPin()
            is WalletException.WrongPin -> WalletCoreException.WrongPin()
            // 这里用 reason,生成的 message 带字段名前缀,不能直接显示给用户.
            is WalletException.Other -> WalletCoreException.Other(e.reason)
        }
    }

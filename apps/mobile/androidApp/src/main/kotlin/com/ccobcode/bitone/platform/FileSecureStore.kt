package com.ccobcode.bitone.platform

import android.content.Context
import android.util.AtomicFile
import java.io.DataOutputStream
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException

/**
 * 应用私有目录.密文,PIN 计数,公钥和清单放 noBackupFilesDir,AtomicFile 在返回前 fsync, 断电时不会留下写了一半的文件.元数据和设置不是秘密,放 SharedPreferences,用
 * commit 同步写.
 */
class FileSecureStore(context: Context) : SecureStore {
    private val vault = AtomicFile(File(context.noBackupFilesDir, "vault.bin"))
    private val pin = AtomicFile(File(context.noBackupFilesDir, "pin.bin"))
    private val publicKeys = AtomicFile(File(context.noBackupFilesDir, "public_keys.bin"))
    private val manifest = AtomicFile(File(context.noBackupFilesDir, "manifest.bin"))
    private val prefs = context.getSharedPreferences("bitone", Context.MODE_PRIVATE)

    /** 读不出来不等于没有钱包.空内容按损坏的密文处理,解锁时进"需恢复",那里能导入. */
    override fun readVault(): ByteArray? =
        try {
            readBytes(vault)
        } catch (_: IOException) {
            ByteArray(0)
        }

    private fun readBytes(file: AtomicFile): ByteArray? =
        try {
            file.readFully()
        } catch (_: FileNotFoundException) {
            null
        }

    override fun readMeta(): WalletMeta? {
        if (!prefs.contains(KEY_PASSPHRASE)) return null
        return WalletMeta(
            imported = prefs.getBoolean(KEY_IMPORTED, false),
            passphrase = prefs.getBoolean(KEY_PASSPHRASE, false),
            fingerprint = prefs.getString(KEY_FINGERPRINT, null),
        )
    }

    /** 旧公钥最先删,中途断电时最多少一份缓存,不会留下别的钱包的公钥. */
    override fun saveWallet(vault: ByteArray, meta: WalletMeta, publicKeys: ByteArray?) {
        this.publicKeys.delete()
        write(this.vault) { it.write(vault) }
        if (publicKeys != null) writePublicKeys(publicKeys)
        writePinRecord(PinRecord.Clean)
        val edit = prefs.edit()
        prefs.all.keys.filter { it.startsWith(KEY_RECEIVE_INDEX) }.forEach(edit::remove)
        edit
            .remove(KEY_HIGHEST_CHANGE)
            .remove(KEY_HIGHEST_CHANGE_TESTNET)
            .putBoolean(KEY_IMPORTED, meta.imported)
            .putBoolean(KEY_PASSPHRASE, meta.passphrase)
            .putString(KEY_FINGERPRINT, meta.fingerprint)
            .commit()
    }

    override fun replaceVault(vault: ByteArray) = write(this.vault) { it.write(vault) }

    override fun readPublicKeys(): ByteArray? = readBytes(publicKeys)

    override fun writePublicKeys(keys: ByteArray) = write(publicKeys) { it.write(keys) }

    override fun readHighestChange() =
        ChangeIndexes(
            prefs.getLong(KEY_HIGHEST_CHANGE, 0).toUInt(),
            prefs.getLong(KEY_HIGHEST_CHANGE_TESTNET, 0).toUInt(),
        )

    override fun writeHighestChange(testnet: Boolean, index: UInt) {
        prefs.edit().putLong(if (testnet) KEY_HIGHEST_CHANGE_TESTNET else KEY_HIGHEST_CHANGE, index.toLong()).commit()
    }

    override fun readReceiveIndex(accountPath: String): UInt =
        prefs.getLong(KEY_RECEIVE_INDEX + accountPath, 0).toUInt()

    override fun writeReceiveIndex(accountPath: String, index: UInt) {
        prefs.edit().putLong(KEY_RECEIVE_INDEX + accountPath, index.toLong()).commit()
    }

    override fun readManifest(): ByteArray? = readBytes(manifest)

    override fun writeManifest(bytes: ByteArray) = write(manifest) { it.write(bytes) }

    override fun readPinRecord(): PinRecord = PinRecord.decode(readBytes(pin))

    override fun writePinRecord(record: PinRecord) {
        write(pin) { data ->
            data.writeInt(record.failures)
            data.writeLong(record.bootId)
            data.writeLong(record.elapsedMs)
        }
    }

    override fun readLanguage(): String? = prefs.getString("language", null)

    override fun writeLanguage(code: String?) {
        prefs.edit().putString("language", code).commit()
    }

    override fun readAutoBrightness(): Boolean = prefs.getBoolean("auto_brightness", true)

    override fun writeAutoBrightness(on: Boolean) {
        prefs.edit().putBoolean("auto_brightness", on).commit()
    }

    override fun wipe() {
        vault.delete()
        pin.delete()
        publicKeys.delete()
        manifest.delete()
        prefs.edit().clear().commit()
    }

    private fun write(file: AtomicFile, block: (DataOutputStream) -> Unit) {
        val stream = file.startWrite()
        try {
            val data = DataOutputStream(stream)
            block(data)
            data.flush()
            file.finishWrite(stream)
        } catch (e: Exception) {
            file.failWrite(stream)
            throw e
        }
    }

    private companion object {
        const val KEY_IMPORTED = "imported"
        const val KEY_PASSPHRASE = "passphrase"
        const val KEY_FINGERPRINT = "fingerprint"
        const val KEY_HIGHEST_CHANGE = "highest_change"
        const val KEY_HIGHEST_CHANGE_TESTNET = "highest_change_testnet"
        const val KEY_RECEIVE_INDEX = "receive_index:"
    }
}

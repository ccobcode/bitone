package com.ccobcode.bitone.platform

/**
 * 解析 Android 密钥证明扩展,取出 RootOfTrust 里的 deviceLocked.结构见
 * https://source.android.com/docs/security/features/keystore/attestation 只读这一个字段,所以只写了够用的 DER 读取,不引入 ASN.1 库.
 */
internal object KeyAttestation {
    const val OID = "1.3.6.1.4.1.11129.2.1.17"

    private const val TAG_ROOT_OF_TRUST = 704
    private const val CLASS_CONTEXT = 2
    private const val SECURITY_LEVEL_SOFTWARE = 0

    /** [extension] 是 X509Certificate.getExtensionValue 的返回值.软件证明或缺字段时返回 null. */
    fun deviceLocked(extension: ByteArray): Boolean? =
        try {
            val description = Der(extension).next().let { Der(extension, it.start, it.end).next() }
            val fields = Der(extension, description.start, description.end)
            fields.next() // attestationVersion
            val securityLevel = fields.next().integer(extension)
            fields.next() // keymasterVersion
            fields.next() // keymasterSecurityLevel
            fields.next() // attestationChallenge
            fields.next() // uniqueId
            fields.next() // softwareEnforced
            val hardwareEnforced = fields.next()
            if (securityLevel == SECURITY_LEVEL_SOFTWARE) {
                null
            } else {
                rootOfTrust(extension, hardwareEnforced)
            }
        } catch (_: IndexOutOfBoundsException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }

    private fun rootOfTrust(bytes: ByteArray, list: Der.Item): Boolean? {
        val entries = Der(bytes, list.start, list.end)
        while (entries.hasNext()) {
            val entry = entries.next()
            if (entry.tagClass == CLASS_CONTEXT && entry.tag == TAG_ROOT_OF_TRUST) {
                val sequence = Der(bytes, entry.start, entry.end).next()
                val root = Der(bytes, sequence.start, sequence.end)
                root.next() // verifiedBootKey
                return root.next().integer(bytes) != 0
            }
        }
        return null
    }

    private class Der(
        private val bytes: ByteArray,
        private var position: Int = 0,
        private val limit: Int = bytes.size,
    ) {
        class Item(val tagClass: Int, val tag: Int, val start: Int, val end: Int) {
            fun integer(bytes: ByteArray): Int {
                var value = 0
                for (i in start until end) value = (value shl 8) or (bytes[i].toInt() and 0xFF)
                return value
            }
        }

        fun hasNext(): Boolean = position < limit

        fun next(): Item {
            require(position < limit)
            val identifier = byte()
            var tag = identifier and 0x1F
            if (tag == 0x1F) {
                tag = 0
                do {
                    val part = byte()
                    tag = (tag shl 7) or (part and 0x7F)
                } while (part and 0x80 != 0)
            }
            var length = byte()
            if (length and 0x80 != 0) {
                val count = length and 0x7F
                require(count in 1..4)
                length = 0
                repeat(count) { length = (length shl 8) or byte() }
            }
            require(length >= 0 && position + length <= limit)
            val item = Item(identifier shr 6, tag, position, position + length)
            position += length
            return item
        }

        private fun byte(): Int = bytes[position++].toInt() and 0xFF
    }
}

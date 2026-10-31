package com.ccobcode.bitone

import com.ccobcode.bitone.platform.KeyAttestation
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class KeyAttestationTest {
    private fun tlv(tag: IntArray, value: ByteArray): ByteArray {
        val length =
            if (value.size < 0x80) byteArrayOf(value.size.toByte())
            else byteArrayOf(0x82.toByte(), (value.size shr 8).toByte(), value.size.toByte())
        return ByteArray(tag.size) { tag[it].toByte() } + length + value
    }

    private fun sequence(vararg items: ByteArray) = tlv(intArrayOf(0x30), items.reduce(ByteArray::plus))

    private fun int(value: Int) = tlv(intArrayOf(0x02), byteArrayOf(value.toByte()))

    private fun enum(value: Int) = tlv(intArrayOf(0x0A), byteArrayOf(value.toByte()))

    private fun octets(size: Int) = tlv(intArrayOf(0x04), ByteArray(size))

    private fun bool(value: Boolean) = tlv(intArrayOf(0x01), byteArrayOf(if (value) 0xFF.toByte() else 0))

    /** [locked] 为 null 时不放 RootOfTrust. */
    private fun extension(level: Int, locked: Boolean?): ByteArray {
        // purpose [1] 放在前面,确认解析会跳过其他字段.
        val other = tlv(intArrayOf(0xA1), tlv(intArrayOf(0x31), int(2)))
        val hardware =
            if (locked == null) {
                sequence(other)
            } else {
                val root = sequence(octets(32), bool(locked), enum(0), octets(32))
                sequence(other, tlv(intArrayOf(0xBF, 0x85, 0x40), root))
            }
        val description =
            sequence(int(4), enum(level), int(4), enum(level), octets(16), octets(0), sequence(int(1)), hardware)
        return tlv(intArrayOf(0x04), description)
    }

    @Test
    fun readsDeviceLocked() {
        assertEquals(true, KeyAttestation.deviceLocked(extension(level = 1, locked = true)))
        assertEquals(false, KeyAttestation.deviceLocked(extension(level = 1, locked = false)))
        assertEquals(true, KeyAttestation.deviceLocked(extension(level = 2, locked = true)))
    }

    @Test
    fun softwareAttestationIsNotTrusted() {
        assertNull(KeyAttestation.deviceLocked(extension(level = 0, locked = true)))
    }

    @Test
    fun missingOrBrokenDataIsUnknown() {
        assertNull(KeyAttestation.deviceLocked(extension(level = 1, locked = null)))
        assertNull(KeyAttestation.deviceLocked(byteArrayOf(0x04, 0x7F, 0x30)))
        assertNull(KeyAttestation.deviceLocked(ByteArray(0)))
    }
}

package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Input
import org.junit.Assert.*
import org.junit.Test

class NativeWireTest {
    private fun encoded(input: Input) = NoiseSessionFactory.hex(NativeWire.event(input))
    @Test fun existingBincodeMessagesKeepTheirOriginalTagsAndZigzagIntegers() {
        assertEquals("0a001e01", encoded(Input.Key(30, true)))
        assertEquals("0a01fb100100", encoded(Input.Button(272, false)))
        assertEquals("0a021d8c", encoded(Input.Move(-15, 70)))
        assertEquals("0a0300ef", encoded(Input.Scroll(0, -120)))
        assertArrayEquals(byteArrayOf(2, 1, 0, 0), NativeWire.prepare())
        assertArrayEquals(byteArrayOf(8, 1), NativeWire.activate())
        assertEquals(65536L, NativeWire.pongId(byteArrayOf(1, 252.toByte(), 0, 0, 1, 0)))
    }
    @Test fun protocolCompatibilityDoesNotDependOnTheApplicationVersion() {
        val app = "9.8.7"
        NativeWire.requireVersion("DESKVER2".toByteArray() + byteArrayOf(0, 9, app.length.toByte()) + app.toByteArray())
        val old = "DESKVER2".toByteArray() + byteArrayOf(0, 8, app.length.toByte()) + app.toByteArray()
        assertThrows(IllegalArgumentException::class.java) { NativeWire.requireVersion(old) }
        assertThrows(IllegalArgumentException::class.java) { NativeWire.event(Input.Key(300, true)) }
        assertThrows(IllegalArgumentException::class.java) { NativeWire.event(Input.Move(16385, 0)) }
    }
}

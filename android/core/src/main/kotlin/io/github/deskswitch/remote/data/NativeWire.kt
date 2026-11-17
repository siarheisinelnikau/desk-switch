package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Input
import java.io.ByteArrayOutputStream

/** The existing bincode DefaultOptions varint encoding; no new device messages. */
object NativeWire {
    const val PROTOCOL = 9
    const val WORKSPACE_PROTOCOL = 3
    const val VERSION = "0.2.0-experimental"
    fun version(): ByteArray = "DESKVER2".toByteArray(Charsets.US_ASCII) + byteArrayOf(0, PROTOCOL.toByte(), VERSION.length.toByte()) + VERSION.toByteArray(Charsets.US_ASCII)
    fun requireVersion(bytes: ByteArray) {
        require(bytes.size >= 12 && bytes.copyOfRange(0, 8).contentEquals("DESKVER2".toByteArray(Charsets.US_ASCII))) { "The computer does not report a device protocol" }
        require(bytes[8].toInt() == 0 && bytes[9].toInt() == PROTOCOL && bytes.size == 11 + (bytes[10].toInt() and 255)) { "The computer uses a different device protocol" }
    }
    private fun ByteArrayOutputStream.uint(value: Long) {
        require(value >= 0)
        when {
            value < 251 -> write(value.toInt())
            value <= 65535 -> { write(251); repeat(2) { write((value ushr (it * 8)).toInt() and 255) } }
            value <= 0xffffffffL -> { write(252); repeat(4) { write((value ushr (it * 8)).toInt() and 255) } }
            else -> { write(253); repeat(8) { write((value ushr (it * 8)).toInt() and 255) } }
        }
    }
    private fun ByteArrayOutputStream.signed(value: Int) = uint(((value.toLong() shl 1) xor (value.toLong() shr 31)) and 0xffffffffL)
    fun ping(id: Long): ByteArray = ByteArrayOutputStream().apply { uint(0); uint(id) }.toByteArray()
    fun prepare(): ByteArray = byteArrayOf(2, 1, 0, 0)
    fun activate(): ByteArray = byteArrayOf(8, 1)
    fun pongId(bytes: ByteArray): Long {
        require(bytes.size >= 2 && bytes[0].toInt() == 1) { "The computer ended input control" }
        val marker = bytes[1].toInt() and 255
        val count = when (marker) { in 0..250 -> 0; 251 -> 2; 252 -> 4; 253 -> 8; else -> error("Invalid heartbeat") }
        require(bytes.size == 2 + count)
        if (count == 0) return marker.toLong()
        var value = 0L
        repeat(count) { value = value or ((bytes[it + 2].toLong() and 255) shl (it * 8)) }
        require(value > 0)
        return value
    }
    fun event(event: Input): ByteArray = ByteArrayOutputStream().apply {
        uint(10)
        when (event) {
            is Input.Key -> { require(event.code in 1..248); uint(0); uint(event.code.toLong()); write(if (event.down) 1 else 0) }
            is Input.Button -> { require(event.code in 272..276); uint(1); uint(event.code.toLong()); write(if (event.down) 1 else 0) }
            is Input.Move -> { require(event.dx in -16384..16384 && event.dy in -16384..16384); uint(2); signed(event.dx); signed(event.dy) }
            is Input.Scroll -> { require(event.dx in -4096..4096 && event.dy in -4096..4096); uint(3); signed(event.dx); signed(event.dy) }
        }
    }.toByteArray()
}

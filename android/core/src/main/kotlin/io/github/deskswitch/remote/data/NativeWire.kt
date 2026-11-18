package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Input
import java.io.ByteArrayOutputStream

enum class ProtocolKind { DEVICE, WORKSPACE }

class ProtocolVersionMismatch(val kind: ProtocolKind, val expected: Int, val received: Int) :
    IllegalArgumentException("Expected protocol $expected, received $received")

/** The existing bincode DefaultOptions varint encoding; no new device messages. */
object NativeWire {
    const val PROTOCOL = 9
    const val WORKSPACE_PROTOCOL = 3
    internal const val MAX_MOVE = 16384
    internal const val MAX_SCROLL = 4096
    private const val MAX_VERSION_LENGTH = 48
    private val versionHeader = "DESKVER2".toByteArray(Charsets.US_ASCII)

    fun version(appVersion: String): ByteArray {
        require(appVersion.length in 1..MAX_VERSION_LENGTH && appVersion.all { it.code <= 127 })
        return versionHeader + byteArrayOf((PROTOCOL ushr 8).toByte(), PROTOCOL.toByte(), appVersion.length.toByte()) +
            appVersion.toByteArray(Charsets.US_ASCII)
    }

    fun requireVersion(bytes: ByteArray) {
        require(bytes.size >= 12 && versionHeader.indices.all { bytes[it] == versionHeader[it] }) {
            "The computer does not report a device protocol"
        }
        val length = bytes[10].toInt() and 255
        require(length in 1..MAX_VERSION_LENGTH && bytes.size == 11 + length && (11 until bytes.size).all { bytes[it] >= 0 }) {
            "The computer sent an invalid device version"
        }
        val received = ((bytes[8].toInt() and 255) shl 8) or (bytes[9].toInt() and 255)
        requireProtocol(ProtocolKind.DEVICE, PROTOCOL, received)
    }

    fun requireProtocols(device: Int, workspace: Int) {
        requireProtocol(ProtocolKind.DEVICE, PROTOCOL, device)
        requireProtocol(ProtocolKind.WORKSPACE, WORKSPACE_PROTOCOL, workspace)
    }

    private fun requireProtocol(kind: ProtocolKind, expected: Int, received: Int) {
        if (received != expected) throw ProtocolVersionMismatch(kind, expected, received)
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
    internal fun validateEvent(event: Input) {
        when (event) {
            is Input.Key -> require(event.code in 1..248)
            is Input.Button -> require(event.code in 272..276)
            is Input.Move -> require(event.dx in -MAX_MOVE..MAX_MOVE && event.dy in -MAX_MOVE..MAX_MOVE)
            is Input.Scroll -> require(event.dx in -MAX_SCROLL..MAX_SCROLL && event.dy in -MAX_SCROLL..MAX_SCROLL)
        }
    }

    fun event(event: Input): ByteArray {
        validateEvent(event)
        return ByteArrayOutputStream().apply {
            uint(10)
            when (event) {
                is Input.Key -> { uint(0); uint(event.code.toLong()); write(if (event.down) 1 else 0) }
                is Input.Button -> { uint(1); uint(event.code.toLong()); write(if (event.down) 1 else 0) }
                is Input.Move -> { uint(2); signed(event.dx); signed(event.dy) }
                is Input.Scroll -> { uint(3); signed(event.dx); signed(event.dy) }
            }
        }.toByteArray()
    }
}

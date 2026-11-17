package io.github.deskswitch.remote.data

import com.southernstorm.noise.protocol.CipherStatePair
import com.southernstorm.noise.protocol.HandshakeState
import io.github.deskswitch.remote.domain.Identity
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest

internal class NoiseSocket(val socket: Socket, private val width: Int) : AutoCloseable {
    data class Handshake(val key: ByteArray, val hash: ByteArray, val payload: ByteArray)
    private val input = DataInputStream(socket.getInputStream())
    private val output = DataOutputStream(socket.getOutputStream())
    private var cipher: CipherStatePair? = null
    val limit = if (width == 1) 128 else 60 * 1024
    fun handshake(identity: Identity, pattern: String, prologue: String, payload: ByteArray, expected: ByteArray? = null): Handshake {
        val state = HandshakeState("Noise_${pattern}_25519_ChaChaPoly_BLAKE2s", HandshakeState.INITIATOR)
        try {
            val intro = prologue.toByteArray(Charsets.UTF_8)
            state.setPrologue(intro, 0, intro.size)
            state.localKeyPair.setPrivateKey(identity.privateKey, 0)
            if (expected != null && pattern == "KK") state.remotePublicKey.setPublicKey(expected, 0)
            state.start()
            val buffer = ByteArray(limit)
            var remote = ByteArray(0)
            while (state.action != HandshakeState.SPLIT) {
                if (state.action == HandshakeState.WRITE_MESSAGE) {
                    val length = state.writeMessage(buffer, 0, payload, 0, payload.size)
                    writeRecord(buffer.copyOf(length))
                } else {
                    val bytes = readRecord()
                    val length = state.readMessage(bytes, 0, bytes.size, buffer, 0)
                    remote = buffer.copyOf(length)
                }
            }
            val key = ByteArray(32).also { state.remotePublicKey.getPublicKey(it, 0) }
            if (expected != null) require(MessageDigest.isEqual(expected, key)) { "The computer identity has changed. Pair again on both devices." }
            val hash = state.handshakeHash.clone()
            cipher = state.split()
            return Handshake(key, hash, remote)
        } finally { state.destroy() }
    }
    @Synchronized fun send(plain: ByteArray) {
        require(plain.size + 16 <= limit)
        val result = ByteArray(plain.size + 16)
        val length = checkNotNull(cipher).sender.encryptWithAd(null, plain, 0, result, 0, plain.size)
        writeRecord(result.copyOf(length))
    }
    fun receive(): ByteArray {
        val bytes = readRecord()
        val result = ByteArray(bytes.size)
        val length = checkNotNull(cipher).receiver.decryptWithAd(null, bytes, 0, result, 0, bytes.size)
        return result.copyOf(length)
    }
    private fun readRecord(): ByteArray {
        val length = if (width == 1) input.readUnsignedByte() else input.readUnsignedShort()
        require(length in 1..limit) { "Invalid encrypted record size" }
        return ByteArray(length).also { input.readFully(it) }
    }
    private fun writeRecord(bytes: ByteArray) {
        require(bytes.size in 1..limit)
        val prefix = if (width == 1) byteArrayOf(bytes.size.toByte()) else byteArrayOf((bytes.size ushr 8).toByte(), bytes.size.toByte())
        output.write(prefix + bytes)
        output.flush()
    }
    override fun close() { socket.close(); cipher?.destroy(); cipher = null }
    companion object {
        fun open(host: String, port: Int, width: Int, register: (Socket) -> Unit): NoiseSocket {
            val socket = Socket()
            register(socket)
            try {
                socket.tcpNoDelay = true
                socket.connect(InetSocketAddress(host, port), 4000)
                socket.soTimeout = 5000
                return NoiseSocket(socket, width)
            } catch (error: Exception) { socket.close(); throw error }
        }
    }
}

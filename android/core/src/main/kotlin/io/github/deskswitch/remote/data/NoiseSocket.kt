package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Identity
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest

internal class NoiseSocket private constructor(val socket: Socket, private val width: Int) : AutoCloseable {
    data class Handshake(val key: ByteArray, val hash: ByteArray, val payload: ByteArray)
    private lateinit var input: DataInputStream
    private lateinit var output: DataOutputStream
    private val stateLock = Any()
    @Volatile private var handle = 0L
    private val limit = if (width == 1) 128 else 60 * 1024

    fun handshake(identity: Identity, pattern: String, prologue: String, payload: ByteArray, expected: ByteArray? = null): Handshake {
        require(pattern == "XX" || pattern == "KK")
        val kk = pattern == "KK"
        val session = synchronized(stateLock) {
            check(!socket.isClosed && handle == 0L)
            NativeNoise.start(kk, identity.privateKey, prologue.toByteArray(Charsets.UTF_8), expected, limit)
                .also { handle = it }
        }
        try {
            writeRecord(NativeNoise.message(session, true, payload))
            val remote = NativeNoise.message(session, false, readRecord())
            if (!kk) writeRecord(NativeNoise.message(session, true, payload))
            val info = NativeNoise.finish(session)
            val key = info.copyOfRange(0, 32)
            if (expected != null) require(MessageDigest.isEqual(expected, key)) {
                "The computer identity has changed. Pair again on both devices."
            }
            return Handshake(key, info.copyOfRange(32, 64), remote)
        } catch (error: Exception) {
            runCatching { close() }
            throw error
        }
    }

    @Synchronized fun send(plain: ByteArray) {
        require(plain.size <= limit - 16)
        writeRecord(NativeNoise.message(handle, true, plain))
    }

    fun receive(): ByteArray {
        val bytes = readRecord()
        return NativeNoise.message(handle, false, bytes)
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

    override fun close() {
        synchronized(stateLock) {
            try {
                socket.close()
            } finally {
                val session = handle
                handle = 0
                if (session != 0L) NativeNoise.release(session)
            }
        }
    }

    companion object {
        fun open(host: String, port: Int, width: Int, register: (NoiseSocket) -> Unit): NoiseSocket {
            val connection = NoiseSocket(Socket(), width)
            val socket = connection.socket
            try {
                register(connection)
                socket.tcpNoDelay = true
                socket.connect(InetSocketAddress(host, port), 4000)
                socket.soTimeout = 5000
                connection.input = DataInputStream(socket.getInputStream())
                connection.output = DataOutputStream(socket.getOutputStream())
                return connection
            } catch (error: Exception) {
                runCatching { connection.close() }
                throw error
            }
        }
    }
}

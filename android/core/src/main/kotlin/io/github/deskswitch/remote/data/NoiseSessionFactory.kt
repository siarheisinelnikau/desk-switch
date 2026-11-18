package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.*
import org.json.JSONObject
import java.security.MessageDigest
import java.util.Collections
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class NoiseSessionFactory(
    private val appVersion: String,
    private val phoneName: String = "Android remote",
    private val onTrusted: (Computer) -> Unit = {},
) : SessionFactory, AutoCloseable {
    @Volatile private var previewListener: (ByteArray?) -> Unit = {}
    @Volatile private var previewErrorListener: (String?) -> Unit = {}
    @Volatile private var inputErrorListener: (String) -> Unit = {}
    override fun setPreviewListener(listener: (ByteArray?) -> Unit) { previewListener = listener }
    override fun setPreviewErrorListener(listener: (String?) -> Unit) { previewErrorListener = listener }
    override fun setInputErrorListener(listener: (String) -> Unit) { inputErrorListener = listener }

    private val sockets = Collections.synchronizedSet(mutableSetOf<NoiseSocket>())
    private val cancelled = AtomicBoolean(false)
    private fun open(computer: Computer, port: Int, width: Int) = NoiseSocket.open(computer.host, port, width) {
        synchronized(sockets) { check(!cancelled.get()) { "Connection cancelled" }; sockets.add(it) }
    }
    override fun connect(computer: Computer, identity: Identity, confirm: (Pairing) -> Boolean, onClosed: (String?) -> Unit): RemoteSession {
        require(computer.host.isNotBlank() && computer.host.length <= 253 && computer.port in 1..65535 && computer.inputPort in 1..65535)
        try {
            val control = open(computer, computer.port, 2)
            val hello = JSONObject().put("name", phoneName.filterNot(Char::isISOControl).trim().take(128).ifEmpty { "Android phone" }).put("app", appVersion)
                .put("protocol", NativeWire.PROTOCOL).put("port", computer.port)
                .put("phone_preview", true).put("control_protocol", 1).put("workspace_protocol", NativeWire.WORKSPACE_PROTOCOL)
            val expected = computer.key?.let(::unhex)
            val auth = control.handshake(identity, "XX", "Desk Switch workspace 1", hello.toString().toByteArray(), expected)
            computer.advertisedKey?.let {
                require(MessageDigest.isEqual(unhex(it), auth.key)) { "The computer identity changed during discovery. Search again." }
            }
            val peer = JSONObject(String(auth.payload, Charsets.UTF_8))
            NativeWire.requireProtocols(peer.getInt("protocol"), peer.getInt("workspace_protocol"))
            val name = peer.getString("name")
            require(name.isNotBlank() && name.length <= 256 && name.none(Char::isISOControl))
            val known = computer.copy(name = name, key = hex(auth.key), previewSupported = peer.optBoolean("phone_preview", false))
            val hash = MessageDigest.getInstance("SHA-256").digest(auth.hash)
            val code = "%03d %03d".format(java.util.Locale.ROOT, (((hash[0].toInt() and 255) shl 8) or (hash[1].toInt() and 255)) % 1000,
                (((hash[2].toInt() and 255) shl 8) or (hash[3].toInt() and 255)) % 1000)
            check(expected != null || confirm(Pairing(known, code, known.key!!))) { "Pairing cancelled" }
            control.socket.soTimeout = 95_000
            control.send("{\"Decision\":true}".toByteArray())
            require(JSONObject(String(control.receive(), Charsets.UTF_8)).getBoolean("Decision")) { "The computer declined pairing" }
            control.send("\"Pinned\"".toByteArray())
            require(String(control.receive(), Charsets.UTF_8) == "\"Pinned\"") { "Pairing did not complete" }
            check(!cancelled.get()) { "Connection cancelled" }
            val data = open(known, known.inputPort, 1)
            val inputAuth = data.handshake(identity, "KK", "Desk Switch protocol 1", NativeWire.version(appVersion), auth.key)
            NativeWire.requireVersion(inputAuth.payload)
            onTrusted(known)
            data.socket.soTimeout = 30_000
            data.send(NativeWire.prepare())
            require(data.receive().contentEquals(byteArrayOf(3, 1))) { "The computer could not prepare input. Check its permission dialog." }
            data.send(NativeWire.activate())
            require(data.receive().contentEquals(byteArrayOf(9, 1))) { "The computer did not activate input" }
            data.socket.soTimeout = 1800
            control.socket.soTimeout = 0
            return Connection(control, data, hex(identity.publicKey), hex(auth.key), known.previewSupported, onClosed).also { it.start() }
        } catch (error: Exception) { close(); throw error }
    }
    override fun close() {
        cancelled.set(true)
        synchronized(sockets) { sockets.forEach { runCatching { it.close() } }; sockets.clear() }
    }
    private object PreviewRequest

    private inner class Connection(
        private val control: NoiseSocket,
        private val data: NoiseSocket,
        private val source: String,
        private val peer: String,
        private val previewSupported: Boolean,
        private val onClosed: (String?) -> Unit,
    ) : RemoteSession {
        private val stopped = AtomicBoolean(false)
        private val commands = LinkedBlockingQueue<Any>(128)
        private val previewLock = Any()
        private val previewOptions = AtomicReference<ScreenOptions?>()
        private var showPreview = false
        private val acknowledgements = ArrayBlockingQueue<String>(1)
        private val controlPending = AtomicBoolean()
        private val ping = java.util.concurrent.atomic.AtomicLong()
        private val barriers = java.util.concurrent.ConcurrentHashMap<Long, java.util.concurrent.CompletableFuture<Unit>>()
        private val workers = java.util.concurrent.CopyOnWriteArrayList<Thread>()
        fun start() {
            workers += thread(name = "Desk Switch input", isDaemon = true) {
                guard {
                    while (!stopped.get()) {
                        when (val command = commands.take()) {
                            is Input -> data.send(NativeWire.event(command))
                            is String -> {
                                // A native Ping/Pong drains earlier input before text on
                                // the control channel; its ack orders all later input.
                                val id = ping.incrementAndGet()
                                val barrier = java.util.concurrent.CompletableFuture<Unit>()
                                barriers[id] = barrier
                                data.send(NativeWire.ping(id))
                                try { barrier.get(2, java.util.concurrent.TimeUnit.SECONDS) } finally { barriers.remove(id) }
                                val response = exchange(ScreenProtocol.clipboard(source, "text/plain;charset=utf-8", command.toByteArray(Charsets.UTF_8)))
                                if (response != ACKNOWLEDGED) inputErrorListener(response.removePrefix(TEXT_ERROR).take(200))
                            }
                            PreviewRequest -> {
                                val options = previewOptions.getAndSet(null) ?: continue
                                check(exchange(ScreenProtocol.options(options, source)) == ACKNOWLEDGED) { "Unexpected preview response" }
                            }
                        }
                    }
                }
            }
            workers += thread(name = "Desk Switch control", isDaemon = true) {
                guard {
                    while (!stopped.get()) {
                        val record = String(control.receive(), Charsets.UTF_8)
                        if (record == ACKNOWLEDGED) {
                            check(controlPending.get() && acknowledgements.offer(record)) { "Unexpected control acknowledgement" }
                            continue
                        }
                        val packet = JSONObject(record)
                        val error = packet.optString("Error")
                        when {
                            error.startsWith(TEXT_ERROR) -> check(controlPending.get() && acknowledgements.offer(error)) {
                                "Unexpected text response"
                            }
                            ScreenProtocol.isFrame(packet) && previewSupported -> {
                                val frame = ScreenProtocol.frame(packet, peer)
                                synchronized(previewLock) {
                                    if (showPreview && !stopped.get()) {
                                        previewErrorListener(null)
                                        previewListener(frame)
                                    }
                                }
                            }
                            previewSupported && error.startsWith(ScreenProtocol.ERROR_PREFIX) -> synchronized(previewLock) {
                                if (showPreview && !stopped.get()) {
                                    previewListener(null)
                                    previewErrorListener(error.removePrefix(ScreenProtocol.ERROR_PREFIX))
                                }
                            }
                            else -> error("Unexpected computer control response")
                        }
                    }
                }
            }
            workers += thread(name = "Desk Switch heartbeat", isDaemon = true) {
                guard {
                    while (!stopped.get()) {
                        val id = NativeWire.pongId(data.receive())
                        barriers.remove(id)?.complete(Unit)
                    }
                }
            }
            workers += thread(name = "Desk Switch lease", isDaemon = true) {
                guard {
                    while (!stopped.get()) {
                        data.send(NativeWire.ping(ping.incrementAndGet()))
                        Thread.sleep(250)
                    }
                }
            }
        }
        private fun exchange(packet: ByteArray): String {
            check(controlPending.compareAndSet(false, true))
            try {
                control.send(packet)
                return checkNotNull(acknowledgements.poll(5, TimeUnit.SECONDS)) { "The computer did not acknowledge the command" }
            } finally {
                controlPending.set(false)
            }
        }

        override fun preview(options: ScreenOptions) {
            if (!previewSupported || stopped.get()) return
            synchronized(previewLock) {
                showPreview = options.enabled
                previewErrorListener(null)
                if (!options.enabled) previewListener(null)
            }
            if (previewOptions.getAndSet(options) == null &&
                !commands.offer(PreviewRequest)) stop("The input queue is full. Reconnect to continue.")
        }
        private fun guard(block: () -> Unit) {
            try { block() } catch (_: Exception) { stop("Connection lost. Reconnect to continue. Pending input was discarded.") }
        }
        override fun input(event: Input) {
            NativeWire.event(event)
            if (!stopped.get() && !commands.offer(event)) stop("The input queue is full. Reconnect to continue.")
        }
        override fun text(value: String) {
            require(value.isNotEmpty() && value.toByteArray(Charsets.UTF_8).size <= 8192) { "Send up to 8 KiB of text at a time" }
            if (!stopped.get() && !commands.offer(value)) stop("Text is arriving too quickly. Reconnect to continue.")
        }
        private fun stop(error: String?) {
            if (!stopped.compareAndSet(false, true)) return
            this@NoiseSessionFactory.close()
            workers.forEach { it.interrupt() }
            commands.clear()
            barriers.values.forEach { it.cancel(true) }; barriers.clear()
            synchronized(previewLock) {
                showPreview = false
                previewListener(null)
                previewErrorListener(null)
            }
            onClosed(error)
        }
        override fun close() = stop(null)
    }
    companion object {
        private const val TEXT_ERROR = "Text: "
        private const val ACKNOWLEDGED = "\"Ping\""
        fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it.toInt() and 255) }
        fun unhex(value: String): ByteArray {
            require(value.length == 64 && value.all { it in '0'..'9' || it in 'a'..'f' }) { "Invalid computer identity" }
            return value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        }
    }
}

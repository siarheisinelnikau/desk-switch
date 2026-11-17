package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.ScreenOptions
import org.json.JSONObject
import java.nio.ByteBuffer
import java.security.MessageDigest
import java.util.Base64

/** Optional MIME payloads inside the existing authenticated Clipboard record. */
internal object ScreenProtocol {
    const val OPTIONS_MIME = "application/x-desk-switch-screen-options+json"
    const val ERROR_PREFIX = "Cursor preview: "
    const val MAX_PNG = 40 * 1024
    private val signature = byteArrayOf(137.toByte(), 80, 78, 71, 13, 10, 26, 10)

    fun options(options: ScreenOptions, source: String): ByteArray {
        val value = JSONObject().put("enabled", options.enabled).put("fps", options.fps)
            .put("width", options.width).put("height", options.height)
        return clipboard(source, OPTIONS_MIME, value.toString().toByteArray(Charsets.UTF_8))
    }

    fun clipboard(source: String, mime: String, bytes: ByteArray): ByteArray {
        NoiseSessionFactory.unhex(source)
        return JSONObject().put("Clipboard", JSONObject().put("source", source).put("mime", mime)
            .put("bytes", Base64.getEncoder().encodeToString(bytes))
            .put("digest", NoiseSessionFactory.hex(MessageDigest.getInstance("SHA-256").digest(bytes))))
            .toString().toByteArray(Charsets.UTF_8)
    }

    fun isFrame(value: JSONObject): Boolean = value.optJSONObject("Clipboard")?.optString("mime") == "image/png"

    fun frame(value: JSONObject, peer: String): ByteArray {
        val packet = value.getJSONObject("Clipboard")
        require(packet.getString("source") == peer && packet.getString("mime") == "image/png") { "Invalid cursor preview source" }
        val encoded = packet.getString("bytes")
        require(encoded.length <= ((MAX_PNG + 2) / 3) * 4) { "Cursor preview frame is too large" }
        val bytes = Base64.getDecoder().decode(encoded)
        require(bytes.size in 33..MAX_PNG) { "Cursor preview frame is too large" }
        require(packet.getString("digest") == NoiseSessionFactory.hex(MessageDigest.getInstance("SHA-256").digest(bytes))) { "Cursor preview checksum mismatch" }
        require(bytes.copyOfRange(0, 8).contentEquals(signature) && bytes.copyOfRange(12, 16).contentEquals("IHDR".toByteArray(Charsets.US_ASCII))) { "Cursor preview is not a PNG image" }
        val header = ByteBuffer.wrap(bytes, 8, 16)
        require(header.int == 13) { "Invalid cursor preview PNG header" }
        header.int
        val width = header.int
        val height = header.int
        require(width in 1..640 && height in 1..480) { "Cursor preview dimensions exceed their limit" }
        return bytes
    }
}

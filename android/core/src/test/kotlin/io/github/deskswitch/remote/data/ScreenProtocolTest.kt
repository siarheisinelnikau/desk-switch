package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.ScreenOptions
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.ByteBuffer
import java.util.Base64

class ScreenProtocolTest {
    private val peer = "01".repeat(32)

    @Test fun previewOptionsKeepTheAuthenticatedClipboardEnvelope() {
        val options = ScreenOptions(enabled = true, fps = 24)
        val packet = JSONObject(String(ScreenProtocol.options(options, peer))).getJSONObject("Clipboard")
        assertEquals(peer, packet.getString("source"))
        assertEquals(ScreenProtocol.OPTIONS_MIME, packet.getString("mime"))
        val decoded = JSONObject(String(Base64.getDecoder().decode(packet.getString("bytes"))))
        assertTrue(decoded.getBoolean("enabled"))
        assertEquals(24, decoded.getInt("fps"))
    }

    @Test fun boundsAndAuthenticatesThePngBeforeDeliveringItToTheRenderer() {
        fun frame(width: Int): ByteArray = ByteBuffer.allocate(33)
            .put(byteArrayOf(137.toByte(), 80, 78, 71, 13, 10, 26, 10))
            .putInt(13).put("IHDR".toByteArray()).putInt(width).putInt(256).array()
        fun packet(bytes: ByteArray) = JSONObject(String(ScreenProtocol.clipboard(peer, "image/png", bytes)))
        assertArrayEquals(frame(384), ScreenProtocol.frame(packet(frame(384)), peer))
        assertThrows(IllegalArgumentException::class.java) { ScreenProtocol.frame(packet(frame(641)), peer) }
        assertThrows(IllegalArgumentException::class.java) { ScreenProtocol.frame(packet(frame(384)), "02".repeat(32)) }
        val changed = packet(frame(384))
        changed.getJSONObject("Clipboard").put("digest", "00".repeat(32))
        assertThrows(IllegalArgumentException::class.java) { ScreenProtocol.frame(changed, peer) }
    }
}

package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class DesktopInteropTest {
    @Test fun encryptedAndroidClientUsesTheUnmodifiedDesktopReceiver() {
        val directory = System.getenv("DESK_SWITCH_MOBILE_PROOF")
        assumeTrue("Run with the isolated desktop fixture", directory != null)
        val info = JSONObject(File(directory!!, "ready.json").readText())
        val computer = Computer("TV", info.getString("host"), info.getInt("port"), info.getInt("inputPort"))
        val identity = noiseIdentity()
        val closed = CountDownLatch(1)
        val preview = CountDownLatch(1)
        var trusted: Computer? = null
        val factory = NoiseSessionFactory("9.8.7-fixture") { trusted = it }
        factory.setPreviewListener { frame ->
            if (frame != null && frame.size >= 24 && frame[19] == 1.toByte() && frame[23] == 1.toByte()) {
                preview.countDown()
            }
        }
        val session = factory.connect(computer, identity, { pairing ->
            assertTrue(pairing.code.matches(Regex("[0-9]{3} [0-9]{3}")))
            File(directory, "phone-code.txt").writeText(pairing.code)
            true
        }, { closed.countDown() })
        assertNotNull(trusted?.key)
        session.input(Input.Move(-15, 70))
        session.input(Input.Scroll(0, -120))
        session.input(Input.Button(272, true))
        session.input(Input.Button(272, false))
        session.preview(ScreenOptions(enabled = true, fps = 15))
        session.text("Привет, TV 👋")
        assertTrue("The synthetic cursor frame arrived alongside input", preview.await(3, TimeUnit.SECONDS))
        session.preview(ScreenOptions())
        session.input(Input.Key(14, true))
        session.input(Input.Key(14, false))
        // The desktop proof closes only after ordered text and the following
        // key arrive; a held modifier additionally exercises fail-open release.
        session.input(Input.Key(42, true))
        Thread.sleep(600)
        session.close()
        assertTrue(closed.await(2, TimeUnit.SECONDS))
        factory.close()
    }
}

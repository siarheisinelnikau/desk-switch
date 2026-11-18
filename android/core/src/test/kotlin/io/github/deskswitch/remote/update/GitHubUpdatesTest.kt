package io.github.deskswitch.remote.update

import java.io.ByteArrayInputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URI
import java.security.MessageDigest
import java.util.concurrent.CancellationException
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class GitHubUpdatesTest {
    @get:Rule val folder = TemporaryFolder()

    @Test
    fun selectsHighestNumericAndroidPrereleaseAcrossPages() {
        val visited = mutableListOf<URI>()
        val source = GitHubUpdates { url ->
            visited += url
            when (url.query) {
                "per_page=30&page=1" -> Response(
                    url,
                    releases(
                        release("0.4.1"),
                        release("9.8.7").put("tag_name", "v9.8.7"),
                        release("9.8.6").put("draft", true),
                        release("9.8.5").put("prerelease", false),
                        release("9.8.4").put("tag_name", "android-remote-v9.8.4-extra"),
                    ),
                    headers = mapOf("Link" to """<https://untrusted.example>; rel="next", <https://untrusted.example>; rel="last""""),
                )
                else -> Response(url, releases(release("0.10.0"), release("0.9.15")))
            }
        }
        assertEquals(AndroidUpdate("0.10.0", assetUrl("0.10.0"), 3, checksum), source.check("0.3.1-experimental"))
        assertEquals(listOf("api.github.com", "api.github.com"), visited.map { it.host })
    }

    @Test
    fun legacyDesktopReleasesAndAlreadyInstalledAndroidVersionReturnNoUpdate() {
        val desktop = release("14.0.1").put("tag_name", "v14.0.1")
        assertNull(source(releases(desktop)).check("0.3.1-experimental"))
        assertNull(source(releases(desktop, release("0.3.1"))).check("0.3.1-experimental"))
        assertNull(source(releases()).check("0.3.1-experimental"))
    }

    @Test
    fun rejectsUnverifiableCurrentReleaseRecords() {
        assertFailure(UpdateFailure.RELEASE_DATA) { source("not json".toByteArray()).check("0.3.1") }
        assertFailure(UpdateFailure.RELEASE_DATA) { source(releases()).check("local") }
        val missingChannel = release("0.4.0").apply { remove("prerelease") }
        assertFailure(UpdateFailure.RELEASE_DATA) { source(releases(missingChannel)).check("0.3.1") }
        val invalidAssets = listOf(
            asset("0.4.0").put("name", "replacement.apk"),
            asset("0.4.0").put("size", 0),
            asset("0.4.0").put("size", 3.5),
            asset("0.4.0").put("digest", JSONObject.NULL),
            asset("0.4.0").put("state", "starter"),
            asset("0.4.0").put("browser_download_url", "https://evil.example/app.apk"),
        )
        invalidAssets.forEach { invalid ->
            val metadata = releases(release("0.4.0").put("assets", JSONArray().put(invalid)))
            assertFailure(UpdateFailure.RELEASE_DATA) { source(metadata).check("0.3.1") }
        }
    }

    @Test
    fun distinguishesRateLimitsAndConnectivityFailures() {
        assertFailure(UpdateFailure.RATE_LIMIT) { source(releases(), status = 429).check("0.3.1") }
        assertFailure(UpdateFailure.RATE_LIMIT) {
            source(releases(), status = 403, headers = mapOf("X-RateLimit-Remaining" to "0")).check("0.3.1")
        }
        assertFailure(UpdateFailure.NETWORK) { source(releases(), status = 503).check("0.3.1") }
        assertFailure(UpdateFailure.NETWORK) {
            GitHubUpdates { throw SocketTimeoutException() }.check("0.3.1")
        }
    }

    @Test
    fun downloadFollowsGitHubCdnAndPublishesVerifiedBytes() {
        val visited = mutableListOf<URI>()
        val source = GitHubUpdates { url ->
            visited += url
            if (url.host == "github.com") {
                Response(url, status = 302, headers = mapOf("Location" to "https://release-assets.githubusercontent.com/a?key=v"))
            } else {
                Response(url, apk, headers = mapOf("Content-Length" to "3"))
            }
        }
        val final = folder.newFile("verified.apk")
        val percentages = mutableListOf<Int>()
        source.download(update, final, percentages::add) {}
        assertEquals(listOf("github.com", "release-assets.githubusercontent.com"), visited.map { it.host })
        assertEquals(listOf(0, 99, 100), percentages)
        assertArrayEquals(apk, final.readBytes())
        assertEquals(listOf(final.name), folder.root.list()?.toList())
    }

    @Test
    fun rejectsUntrustedRedirectsWithoutContactingTheirHosts() {
        for (location in listOf(
            "http://release-assets.githubusercontent.com/a",
            "https://release-assets.githubusercontent.com.evil.example/a",
            "https://evil.example@release-assets.githubusercontent.com/a",
            "https://release-assets.githubusercontent.com:8443/a",
        )) {
            val source = GitHubUpdates { url ->
                assertEquals("github.com", url.host)
                Response(url, status = 302, headers = mapOf("Location" to location))
            }
            assertFailure(UpdateFailure.NETWORK) { source.download(update, folder.root.resolve("update.apk"), {}) {} }
            assertEquals(emptyList<String>(), folder.root.list()?.toList())
        }
    }

    @Test
    fun deletesStagedBytesWhenLengthOrDigestDoesNotMatch() {
        for (bytes in listOf(byteArrayOf(1, 2), byteArrayOf(1, 2, 3, 4), byteArrayOf(3, 2, 1))) {
            val final = folder.root.resolve("update.apk")
            assertFailure(UpdateFailure.CORRUPT_FILE) { source(bytes).download(update, final, {}) {} }
            assertFalse(final.exists())
            assertEquals(emptyList<String>(), folder.root.list()?.toList())
        }
    }

    @Test
    fun cancellationCleansStagingAndExistingDestinationIsPreserved() {
        var canceled = false
        val final = folder.root.resolve("update.apk")
        assertThrows(CancellationException::class.java) {
            source(apk).download(update, final, { if (it > 0) canceled = true }) {
                if (canceled) throw CancellationException()
            }
        }
        assertEquals(emptyList<String>(), folder.root.list()?.toList())

        final.writeText("existing")
        assertFailure(UpdateFailure.CORRUPT_FILE) { source(byteArrayOf(3, 2, 1)).download(update, final, {}) {} }
        assertEquals("existing", final.readText())
        assertEquals(listOf(final.name), folder.root.list()?.toList())
    }

    private fun source(body: ByteArray, status: Int = 200, headers: Map<String, String> = emptyMap()) =
        GitHubUpdates { Response(it, body, status, headers) }

    private fun assertFailure(expected: UpdateFailure, work: () -> Unit) {
        assertEquals(expected, assertThrows(UpdateException::class.java) { work() }.failure)
    }

    private class Response(
        url: URI,
        private val body: ByteArray = byteArrayOf(),
        private val status: Int = 200,
        private val headers: Map<String, String> = emptyMap(),
    ) : HttpURLConnection(url.toURL()) {
        override fun getResponseCode() = status
        override fun getHeaderField(name: String) = headers[name]
        override fun getInputStream(): InputStream = ByteArrayInputStream(body)
        override fun usingProxy() = false
        override fun connect() = Unit
        override fun disconnect() = Unit
    }

    companion object {
        private val apk = byteArrayOf(1, 2, 3)
        private val checksum = MessageDigest.getInstance("SHA-256").digest(apk).joinToString("") { "%02x".format(it) }
        private val update = AndroidUpdate("0.4.0", assetUrl("0.4.0"), 3, checksum)

        private fun assetUrl(version: String) =
            "https://github.com/siarheisinelnikau/desk-switch/releases/download/android-remote-v$version/DeskSwitchRemote-v$version-experimental.apk"

        private fun asset(version: String) = JSONObject()
            .put("name", "DeskSwitchRemote-v$version-experimental.apk")
            .put("size", apk.size)
            .put("state", "uploaded")
            .put("digest", "sha256:${checksum.uppercase()}")
            .put("browser_download_url", assetUrl(version))

        private fun release(version: String) = JSONObject()
            .put("tag_name", "android-remote-v$version")
            .put("draft", false)
            .put("prerelease", true)
            .put("assets", JSONArray().put(asset(version)))

        private fun releases(vararg records: JSONObject) = JSONArray(records.toList()).toString().toByteArray()
    }
}

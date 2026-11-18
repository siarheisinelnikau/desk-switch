package io.github.deskswitch.remote.update

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URI
import java.net.URISyntaxException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.util.Locale
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

private const val repository = "siarheisinelnikau/desk-switch"
private const val tagPrefix = "android-remote-v"
private const val maxReleasePages = 20
private const val maxMetadataBytes = 8 * 1024 * 1024
private const val maxApkBytes = 2L * 1024 * 1024 * 1024
private val releaseVersion = Regex("(0|[1-9][0-9]{0,9})\\.(0|[1-9][0-9]{0,9})\\.(0|[1-9][0-9]{0,9})")
private val digestHex = Regex("[a-fA-F0-9]{64}")
private val hasNextPage = Regex("""(?:^|,)\s*<[^>]+>\s*;\s*rel="next"(?:\s*,|\s*$)""")
private val redirectCodes = setOf(301, 302, 303, 307, 308)
private val assetHosts = setOf(
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "github-releases.githubusercontent.com",
)

class GitHubUpdates internal constructor(
    private val connect: (URI) -> HttpURLConnection,
) : UpdateSource {
    constructor() : this({ it.toURL().openConnection() as HttpURLConnection })

    override fun check(installedVersion: String): AndroidUpdate? {
        var newest = Version.parse(installedVersion.removeSuffix("-experimental"))
            ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
        var selected: JSONObject? = null
        for (page in 1..maxReleasePages) {
            val (releases, morePages) = network {
                request(
                    URI("https://api.github.com/repos/$repository/releases?per_page=30&page=$page"),
                    accept = "application/vnd.github+json",
                ) { response ->
                    val bytes = response.inputStream.use { it.readBounded(maxMetadataBytes + 1) }
                    if (bytes.size > maxMetadataBytes) throw UpdateException(UpdateFailure.RELEASE_DATA)
                    val releases = try {
                        JSONArray(bytes.toString(Charsets.UTF_8))
                    } catch (error: JSONException) {
                        throw UpdateException(UpdateFailure.RELEASE_DATA, error)
                    }
                    releases to hasNextPage.containsMatchIn(response.getHeaderField("Link").orEmpty())
                }
            }
            for (index in 0 until releases.length()) {
                val release = releases.optJSONObject(index) ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
                val tag = release.opt("tag_name") as? String ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
                if (!tag.startsWith(tagPrefix)) continue
                val version = Version.parse(tag.removePrefix(tagPrefix)) ?: continue
                if (version <= newest) continue
                val draft = release.opt("draft") as? Boolean ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
                if (draft) continue
                val prerelease = release.opt("prerelease") as? Boolean ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
                if (prerelease) {
                    newest = version
                    selected = release
                }
            }
            if (!morePages) return selected?.let { readUpdate(it, newest.toString()) }
        }
        throw UpdateException(UpdateFailure.RELEASE_DATA)
    }

    override fun download(update: AndroidUpdate, destination: File, progress: (Int) -> Unit, checkActive: () -> Unit) {
        if (Version.parse(update.version) == null || update.downloadUrl != downloadUrl(update.version) ||
            update.size !in 1..maxApkBytes || !digestHex.matches(update.sha256)
        ) {
            throw UpdateException(UpdateFailure.RELEASE_DATA)
        }
        checkActive()
        val target = destination.absoluteFile.toPath()
        val partial = storage { Files.createTempFile(target.parent, "remote-update-", ".part") }
        try {
            network {
                request(URI(update.downloadUrl), "application/octet-stream", checkActive) { response ->
                    val length = response.getHeaderField("Content-Length")
                    if (length != null && length.toLongOrNull() != update.size) {
                        throw UpdateException(UpdateFailure.CORRUPT_FILE)
                    }
                    response.inputStream.use { input ->
                        val output = storage { Files.newOutputStream(partial).buffered() }
                        storage { output.use { copyAndVerify(input, it, update, progress, checkActive) } }
                    }
                }
            }
            checkActive()
            storage { Files.move(partial, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING) }
            progress(100)
        } finally {
            // The destination is published only after validation. Clean up this attempt's staging file.
            runCatching { Files.deleteIfExists(partial) }
        }
    }

    private fun readUpdate(release: JSONObject, version: String): AndroidUpdate {
        val assets = release.optJSONArray("assets") ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
        val matches = (0 until assets.length()).mapNotNull(assets::optJSONObject)
            .filter { it.opt("name") == assetName(version) }
        val asset = matches.singleOrNull() ?: throw UpdateException(UpdateFailure.RELEASE_DATA)
        val url = downloadUrl(version)
        val size = when (val value = asset.opt("size")) {
            is Int -> value.toLong()
            is Long -> value
            else -> throw UpdateException(UpdateFailure.RELEASE_DATA)
        }
        val digest = (asset.opt("digest") as? String).orEmpty()
        val sha256 = digest.removePrefix("sha256:")
        if (asset.opt("state") != "uploaded" || asset.opt("browser_download_url") != url ||
            size !in 1..maxApkBytes || !digest.startsWith("sha256:") || !digestHex.matches(sha256)
        ) {
            throw UpdateException(UpdateFailure.RELEASE_DATA)
        }
        return AndroidUpdate(version, url, size, sha256.lowercase(Locale.ROOT))
    }

    private fun <T> request(
        initial: URI,
        accept: String,
        checkActive: () -> Unit = {},
        read: (HttpURLConnection) -> T,
    ): T {
        var url = initial
        repeat(6) { hop ->
            checkActive()
            val response = connect(url)
            try {
                response.instanceFollowRedirects = false
                response.connectTimeout = 10_000
                response.readTimeout = 15_000
                response.setRequestProperty("User-Agent", "DeskSwitchRemote")
                response.setRequestProperty("Accept", accept)
                response.setRequestProperty("Accept-Encoding", "identity")
                val status = response.responseCode
                checkActive()
                if (status == 200) return read(response)
                if (status == 429 || status == 403 && response.isRateLimited()) {
                    throw UpdateException(UpdateFailure.RATE_LIMIT)
                }
                if (status in redirectCodes && initial.host == "github.com" && hop < 5) {
                    url = redirect(url, response.getHeaderField("Location"))
                } else {
                    throw UpdateException(UpdateFailure.NETWORK)
                }
            } finally {
                response.disconnect()
            }
        }
        throw UpdateException(UpdateFailure.NETWORK)
    }

    private fun HttpURLConnection.isRateLimited(): Boolean =
        getHeaderField("X-RateLimit-Remaining") == "0" || getHeaderField("Retry-After") != null ||
            errorStream?.use { "rate limit" in it.readBounded(4096).toString(Charsets.UTF_8).lowercase(Locale.ROOT) } == true

    private fun redirect(from: URI, location: String?): URI {
        val target = try {
            from.resolve(URI(location ?: throw UpdateException(UpdateFailure.NETWORK)))
        } catch (error: URISyntaxException) {
            throw UpdateException(UpdateFailure.NETWORK, error)
        }
        if (target.scheme != "https" || target.host !in assetHosts || target.userInfo != null ||
            target.port !in setOf(-1, 443) || target.fragment != null
        ) {
            throw UpdateException(UpdateFailure.NETWORK)
        }
        return target
    }
}

private fun assetName(version: String) = "DeskSwitchRemote-v$version-experimental.apk"
private fun downloadUrl(version: String) =
    "https://github.com/$repository/releases/download/$tagPrefix$version/${assetName(version)}"

private data class Version(val major: Long, val minor: Long, val patch: Long) : Comparable<Version> {
    override fun compareTo(other: Version) = compareValuesBy(this, other, Version::major, Version::minor, Version::patch)

    override fun toString() = "$major.$minor.$patch"

    companion object {
        fun parse(value: String): Version? {
            val (_, major, minor, patch) = releaseVersion.matchEntire(value)?.groupValues ?: return null
            return Version(major.toLong(), minor.toLong(), patch.toLong())
        }
    }
}

private inline fun <T> network(read: () -> T): T = try {
    read()
} catch (error: IOException) {
    throw UpdateException(UpdateFailure.NETWORK, error)
}

private inline fun <T> storage(write: () -> T): T = try {
    write()
} catch (error: IOException) {
    throw UpdateException(UpdateFailure.STORAGE, error)
}

private fun InputStream.readBounded(limit: Int): ByteArray {
    val output = ByteArrayOutputStream()
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    while (output.size() < limit) {
        val count = read(buffer, 0, minOf(buffer.size, limit - output.size()))
        if (count < 0) break
        output.write(buffer, 0, count)
    }
    return output.toByteArray()
}

private fun copyAndVerify(
    input: InputStream,
    output: OutputStream,
    update: AndroidUpdate,
    progress: (Int) -> Unit,
    checkActive: () -> Unit,
) {
    val digest = MessageDigest.getInstance("SHA-256")
    val buffer = ByteArray(64 * 1024)
    var total = 0L
    var reported = 0
    progress(reported)
    while (true) {
        checkActive()
        val count = network { input.read(buffer) }
        if (count < 0) break
        total += count
        if (total > update.size) throw UpdateException(UpdateFailure.CORRUPT_FILE)
        digest.update(buffer, 0, count)
        output.write(buffer, 0, count)
        val percent = (total * 100 / update.size).toInt().coerceAtMost(99)
        if (percent > reported) {
            reported = percent
            progress(percent)
        }
    }
    val expected = ByteArray(32) { index -> update.sha256.substring(index * 2, index * 2 + 2).toInt(16).toByte() }
    if (total != update.size || !MessageDigest.isEqual(digest.digest(), expected)) {
        throw UpdateException(UpdateFailure.CORRUPT_FILE)
    }
}

package io.github.deskswitch.remote.update

import java.io.File

data class AndroidUpdate(val version: String, val downloadUrl: String, val size: Long, val sha256: String)

enum class UpdateFailure {
    NETWORK, RATE_LIMIT, RELEASE_DATA, CORRUPT_FILE, INCOMPATIBLE_APK, SIGNER_MISMATCH, STORAGE
}

class UpdateException(val failure: UpdateFailure, cause: Throwable? = null) :
    Exception(failure.name, cause)

interface UpdateSource {
    fun check(installedVersion: String): AndroidUpdate?

    /** Publishes the verified download at a caller-owned staging destination. */
    fun download(
        update: AndroidUpdate,
        destination: File,
        progress: (Int) -> Unit,
        checkActive: () -> Unit,
    )
}

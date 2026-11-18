package io.github.deskswitch.remote.data

import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import io.github.deskswitch.remote.update.AndroidUpdate
import io.github.deskswitch.remote.update.UpdateException
import io.github.deskswitch.remote.update.UpdateFailure
import java.io.File
import java.io.IOException

/** Validates the replacement against the installed package before Android opens its installer. */
@Suppress("DEPRECATION")
internal class ApkInstaller(private val context: Context) {
    val directory = File(context.cacheDir, "desk-switch-updates")

    fun verify(update: AndroidUpdate, file: File) {
        if (!owns(file) || file.length() != update.size) throw UpdateException(UpdateFailure.CORRUPT_FILE)
        val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val packages = context.packageManager
        val installed = packages.getPackageInfo(context.packageName, flags)
        val replacement = try {
            packages.getPackageArchiveInfo(file.absolutePath, flags)
        } catch (failure: IllegalArgumentException) {
            throw UpdateException(UpdateFailure.INCOMPATIBLE_APK, failure)
        } ?: throw UpdateException(UpdateFailure.INCOMPATIBLE_APK)
        if (replacement.packageName != installed.packageName || replacement.versionName != "${update.version}-experimental" ||
            versionCode(replacement) <= versionCode(installed) ||
            replacement.applicationInfo?.let { it.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0 } != false) {
            throw UpdateException(UpdateFailure.INCOMPATIBLE_APK)
        }
        val currentSigners = signers(installed)
        if (currentSigners.isEmpty() || signers(replacement) != currentSigners) {
            throw UpdateException(UpdateFailure.SIGNER_MISMATCH)
        }
    }

    fun allowed(): Boolean = context.packageManager.canRequestPackageInstalls()

    fun permissionIntent(): Intent = Intent(
        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.fromParts("package", context.packageName, null),
    )

    fun installIntent(file: File): Intent {
        if (!owns(file)) throw UpdateException(UpdateFailure.CORRUPT_FILE)
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.updates", file)
        return Intent(Intent.ACTION_INSTALL_PACKAGE, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            .putExtra(Intent.EXTRA_RETURN_RESULT, true)
    }

    private fun owns(file: File): Boolean = try {
        file.isFile && file.extension == "apk" && file.canonicalFile.parentFile == directory.canonicalFile
    } catch (failure: IOException) {
        throw UpdateException(UpdateFailure.STORAGE, failure)
    }

    private fun versionCode(info: PackageInfo): Long =
        if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()

    private fun signers(info: PackageInfo): Set<Signature> =
        if (Build.VERSION.SDK_INT >= 28) info.signingInfo?.apkContentsSigners.orEmpty().toSet()
        else info.signatures.orEmpty().toSet()
}

class UpdateFileProvider : FileProvider()

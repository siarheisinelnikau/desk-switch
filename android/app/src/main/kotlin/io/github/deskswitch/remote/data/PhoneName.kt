package io.github.deskswitch.remote.data

import android.content.Context
import android.os.Build
import android.provider.Settings

/** A familiar local device label, independent of the cryptographic identity. */
fun phoneName(context: Context): String {
    val friendly = runCatching { Settings.Global.getString(context.contentResolver, "device_name") }.getOrNull()
    val fallback = if (Build.MODEL.startsWith(Build.MANUFACTURER, ignoreCase = true)) Build.MODEL
        else "${Build.MANUFACTURER.replaceFirstChar(Char::uppercase)} ${Build.MODEL}"
    return (friendly?.takeIf(String::isNotBlank) ?: fallback).filterNot(Char::isISOControl).trim().take(64)
        .ifEmpty { "Android phone" }
}

package io.github.deskswitch.remote.data

import android.content.Context

internal class UpdatePreferences(context: Context) {
    private val preferences = context.getSharedPreferences("updates", Context.MODE_PRIVATE)

    var startupCheck: Boolean
        get() = preferences.getBoolean("startup_check", true)
        set(enabled) {
            preferences.edit().putBoolean("startup_check", enabled).apply()
        }
}

package io.github.deskswitch.remote.data

import android.content.Context
import io.github.deskswitch.remote.domain.AppLanguage
import io.github.deskswitch.remote.domain.AppTheme
import io.github.deskswitch.remote.domain.AppearanceSettings
import io.github.deskswitch.remote.domain.AppearanceSettingsStore

internal class AppearancePreferences(context: Context) : AppearanceSettingsStore {
    private val prefs = context.getSharedPreferences("appearance", Context.MODE_PRIVATE)

    override fun load(): AppearanceSettings {
        val default = AppearanceSettings()
        val theme = prefs.getString("theme", null)
        val language = prefs.getString("language", null)
        return AppearanceSettings(
            theme = AppTheme.entries.firstOrNull { it.name == theme } ?: default.theme,
            language = AppLanguage.entries.firstOrNull { it.tag == language } ?: default.language,
        )
    }

    override fun save(settings: AppearanceSettings) {
        prefs.edit().putString("theme", settings.theme.name)
            .putString("language", settings.language.tag)
            .apply()
    }
}

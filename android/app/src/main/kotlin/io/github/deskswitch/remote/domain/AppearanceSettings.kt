package io.github.deskswitch.remote.domain

internal enum class AppTheme { SYSTEM, LIGHT, DARK }

internal enum class AppLanguage(val tag: String?, val displayName: String) {
    SYSTEM(null, ""),
    ENGLISH("en", "English"),
    RUSSIAN("ru", "Русский"),
    GERMAN("de", "Deutsch"),
    FRENCH("fr", "Français"),
    SPANISH("es", "Español"),
    ITALIAN("it", "Italiano"),
}

internal data class AppearanceSettings(
    val theme: AppTheme = AppTheme.DARK,
    val language: AppLanguage = AppLanguage.SYSTEM,
)

internal interface AppearanceSettingsStore {
    fun load(): AppearanceSettings
    fun save(settings: AppearanceSettings)
}

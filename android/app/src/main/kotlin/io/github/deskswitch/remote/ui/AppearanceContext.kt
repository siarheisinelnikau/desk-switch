package io.github.deskswitch.remote.ui

import android.content.Context
import android.content.res.Configuration
import android.os.LocaleList
import io.github.deskswitch.remote.domain.AppTheme
import io.github.deskswitch.remote.domain.AppearanceSettings
import java.util.Locale

internal fun Context.withAppearance(appearance: AppearanceSettings): Context {
    if (appearance.theme == AppTheme.SYSTEM && appearance.language.tag == null) return this
    val config = Configuration()
    appearance.language.tag?.let { config.setLocales(LocaleList(Locale.forLanguageTag(it))) }
    config.uiMode = when (appearance.theme) {
        AppTheme.SYSTEM -> 0
        AppTheme.LIGHT -> Configuration.UI_MODE_NIGHT_NO
        AppTheme.DARK -> Configuration.UI_MODE_NIGHT_YES
    }
    return createConfigurationContext(config)
}

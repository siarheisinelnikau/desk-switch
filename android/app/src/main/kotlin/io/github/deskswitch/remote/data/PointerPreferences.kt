package io.github.deskswitch.remote.data

import android.content.Context
import io.github.deskswitch.remote.domain.PointerMode
import io.github.deskswitch.remote.domain.PointerSettings
import io.github.deskswitch.remote.domain.PointerSettingsStore

class PointerPreferences(context: Context) : PointerSettingsStore {
    private val prefs = context.getSharedPreferences("pointer", Context.MODE_PRIVATE)

    override fun load(): PointerSettings = runCatching {
        PointerSettings(
            mode = runCatching { PointerMode.valueOf(prefs.getString("mode", null).orEmpty()) }.getOrDefault(PointerMode.TOUCHPAD),
            touchpadSpeed = prefs.getFloat("touchpad_speed", 1f),
            joystickSpeed = prefs.getFloat("joystick_speed", 1000f),
            previewEnabled = prefs.getBoolean("preview_enabled", false),
            previewFps = prefs.getInt("preview_fps", 10),
            doubleTapScroll = prefs.getBoolean("double_tap_scroll", true),
            scrollGestureSpeed = prefs.getFloat("scroll_gesture_speed", 1f),
            scrollStickSpeed = prefs.getFloat("scroll_stick_speed", 1f),
            invertScroll = prefs.getBoolean("invert_scroll", false),
        ).normalized()
    }.getOrDefault(PointerSettings())

    override fun save(settings: PointerSettings) {
        val value = settings.normalized()
        prefs.edit().putString("mode", value.mode.name)
            .putFloat("touchpad_speed", value.touchpadSpeed)
            .putFloat("joystick_speed", value.joystickSpeed)
            .putBoolean("preview_enabled", value.previewEnabled)
            .putInt("preview_fps", value.previewFps)
            .putBoolean("double_tap_scroll", value.doubleTapScroll)
            .putFloat("scroll_gesture_speed", value.scrollGestureSpeed)
            .putFloat("scroll_stick_speed", value.scrollStickSpeed)
            .putBoolean("invert_scroll", value.invertScroll).apply()
    }
}

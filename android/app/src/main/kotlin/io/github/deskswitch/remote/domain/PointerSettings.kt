package io.github.deskswitch.remote.domain

enum class PointerMode { TOUCHPAD, JOYSTICK }

data class PointerSettings(
    val mode: PointerMode = PointerMode.TOUCHPAD,
    val touchpadSpeed: Float = 1f,
    val joystickSpeed: Float = 1000f,
    val previewEnabled: Boolean = false,
    val previewFps: Int = 10,
) {
    fun normalized() = copy(
        touchpadSpeed = touchpadSpeed.takeIf(Float::isFinite)?.coerceIn(0.35f, 3f) ?: 1f,
        joystickSpeed = joystickSpeed.takeIf(Float::isFinite)?.coerceIn(300f, 2400f) ?: 1000f,
        previewFps = previewFps.coerceIn(1, 30),
    )
}

interface PointerSettingsStore {
    fun load(): PointerSettings
    fun save(settings: PointerSettings)
}

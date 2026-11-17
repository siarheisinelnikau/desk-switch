package io.github.deskswitch.remote.domain

import kotlin.math.abs
import kotlin.math.sign

internal class ScrollMotion {
    private var remainderX = 0.0
    private var remainderY = 0.0

    fun gesture(dxDp: Float, dyDp: Float, settings: PointerSettings): Input.Scroll? {
        val gain = 3.5 * settings.scrollGestureSpeed * direction(settings)
        return accumulate(dxDp * gain, dyDp * gain)
    }

    fun stick(displacement: Float, seconds: Double, settings: PointerSettings): Input.Scroll? {
        if (!displacement.isFinite() || !seconds.isFinite() || abs(displacement) <= STICK_DEAD_ZONE || seconds <= 0) {
            return null
        }
        val strength = (abs(displacement).coerceAtMost(1f) - STICK_DEAD_ZONE) / (1f - STICK_DEAD_ZONE)
        val speed = (strength * strength * 1440 + strength * 240) * settings.scrollStickSpeed
        return accumulate(0.0, sign(displacement) * speed * seconds.coerceAtMost(0.05) * direction(settings))
    }

    fun reset() {
        remainderX = 0.0
        remainderY = 0.0
    }

    private fun accumulate(dx: Double, dy: Double): Input.Scroll? {
        val x = (remainderX + dx).coerceIn(-4096.0, 4096.0)
        val y = (remainderY + dy).coerceIn(-4096.0, 4096.0)
        val stepX = x.toInt()
        val stepY = y.toInt()
        remainderX = x - stepX
        remainderY = y - stepY
        return if (stepX == 0 && stepY == 0) null else Input.Scroll(stepX, stepY)
    }

    companion object {
        const val STICK_DEAD_ZONE = 0.14f
        fun direction(settings: PointerSettings) = if (settings.invertScroll) -1 else 1
    }
}

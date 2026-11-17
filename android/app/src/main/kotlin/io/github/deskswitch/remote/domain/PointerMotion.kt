package io.github.deskswitch.remote.domain

import kotlin.math.hypot
import kotlin.math.pow

data class PointerStep(val dx: Int, val dy: Int)

/** Fractional movement belongs to one gesture and is discarded on cancellation. */
class PointerMotion {
    private var remainderX = 0.0
    private var remainderY = 0.0

    fun reset() { remainderX = 0.0; remainderY = 0.0 }

    fun touchpad(dx: Float, dy: Float, density: Float, sensitivity: Float): PointerStep {
        if (!density.isFinite() || density <= 0f) return PointerStep(0, 0)
        val speed = PointerSettings(touchpadSpeed = sensitivity).normalized().touchpadSpeed
        return take(dx.toDouble() / density * 1.8 * speed, dy.toDouble() / density * 1.8 * speed)
    }

    /** x/y describe displacement relative to the outer ring's radius. */
    fun joystick(x: Float, y: Float, seconds: Double, maximumSpeed: Float): PointerStep {
        if (!x.isFinite() || !y.isFinite() || !seconds.isFinite()) return PointerStep(0, 0)
        val length = hypot(x.toDouble(), y.toDouble())
        if (length <= DEAD_ZONE || seconds <= 0.0) return PointerStep(0, 0)
        val strength = ((length.coerceAtMost(1.0) - DEAD_ZONE) / (1.0 - DEAD_ZONE)).pow(1.65)
        val speed = PointerSettings(joystickSpeed = maximumSpeed).normalized().joystickSpeed
        // Never catch up after a paused frame or send an accumulated jump.
        val distance = strength * speed * seconds.coerceAtMost(0.05)
        return take(x / length * distance, y / length * distance)
    }

    private fun take(dx: Double, dy: Double): PointerStep {
        if (!dx.isFinite() || !dy.isFinite()) { reset(); return PointerStep(0, 0) }
        val x = remainderX + dx
        val y = remainderY + dy
        val wholeX = x.toInt()
        val wholeY = y.toInt()
        remainderX = x % 1.0
        remainderY = y % 1.0
        return PointerStep(wholeX.coerceIn(-16384, 16384), wholeY.coerceIn(-16384, 16384))
    }

    companion object { const val DEAD_ZONE = 0.065 }
}

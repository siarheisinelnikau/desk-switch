package io.github.deskswitch.remote.domain

import org.junit.Assert.*
import org.junit.Test
import kotlin.math.hypot

class PointerMotionTest {
    @Test fun joystickDistanceIsIndependentOfFrameRate() {
        fun movement(frames: Int): Int {
            val motion = PointerMotion()
            return (0 until frames).sumOf { motion.joystick(1f, 0f, 1.0 / frames, 1000f).dx }
        }
        assertTrue(kotlin.math.abs(movement(60) - movement(120)) <= 1)
        assertTrue(movement(60) in 999..1000)
    }

    @Test fun fartherDisplacementIsFasterAndDiagonalSpeedIsBounded() {
        val near = PointerMotion().joystick(0.3f, 0f, 0.04, 1000f)
        val far = PointerMotion().joystick(0.9f, 0f, 0.04, 1000f)
        val diagonal = PointerMotion().joystick(2f, 2f, 0.04, 1000f)
        assertTrue(near.dx in 1 until far.dx)
        assertTrue(hypot(diagonal.dx.toDouble(), diagonal.dy.toDouble()) <= 40.0)
    }

    @Test fun deadZoneAndCancelledFractionalMovementNeverDrift() {
        val motion = PointerMotion()
        repeat(20) { assertEquals(PointerStep(0, 0), motion.joystick(0.03f, -0.02f, 0.016, 1000f)) }
        motion.touchpad(0.1f, 0f, 1f, 1f)
        motion.reset()
        assertEquals(PointerStep(0, 0), motion.touchpad(0.1f, 0f, 1f, 1f))
    }

    @Test fun pausesCannotProduceAnUnboundedCatchUpJump() {
        assertEquals(PointerStep(50, 0), PointerMotion().joystick(1f, 0f, 30.0, 1000f))
        assertEquals(PointerStep(0, 0), PointerMotion().joystick(Float.NaN, 1f, 0.016, 1000f))
        assertEquals(PointerStep(0, 0), PointerMotion().joystick(1f, 0f, -1.0, 1000f))
    }

    @Test fun touchpadSensitivityAndDensityAreAppliedWithoutLosingSmallMovements() {
        val motion = PointerMotion()
        assertEquals(PointerStep(18, -9), motion.touchpad(20f, -10f, 2f, 1f))
        assertEquals(PointerStep(36, 0), PointerMotion().touchpad(20f, 0f, 2f, 2f))
        val sum = (0 until 10).sumOf { motion.touchpad(0.2f, 0f, 1f, 1f).dx }
        assertTrue(sum in 3..4)
    }

    @Test fun corruptedSettingsUseFiniteSafeDefaults() {
        val settings = PointerSettings(touchpadSpeed = Float.NaN, joystickSpeed = Float.POSITIVE_INFINITY).normalized()
        assertEquals(PointerSettings(), settings)
        assertEquals(0.35f, PointerSettings(touchpadSpeed = -1f).normalized().touchpadSpeed)
        assertEquals(2400f, PointerSettings(joystickSpeed = 100000f).normalized().joystickSpeed)
        assertEquals(1, PointerSettings(previewFps = -1).normalized().previewFps)
        assertEquals(30, PointerSettings(previewFps = Int.MAX_VALUE).normalized().previewFps)
        assertFalse(PointerSettings().previewEnabled)
    }

    @Test fun outlierTouchCoordinatesRespectNativeBoundsWithoutAReplayTail() {
        val motion = PointerMotion()
        assertEquals(PointerStep(16384, -16384), motion.touchpad(Float.MAX_VALUE, -Float.MAX_VALUE, 1f, 3f))
        assertEquals(PointerStep(0, 0), motion.touchpad(0f, 0f, 1f, 3f))
    }
}

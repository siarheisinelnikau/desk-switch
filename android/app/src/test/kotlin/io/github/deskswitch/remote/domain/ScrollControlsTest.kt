package io.github.deskswitch.remote.domain

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ScrollControlsTest {
    @Test fun aSecondTapStaysReservedUntilMovementCrossesSlop() {
        val gesture = DoubleTapScroll(300, 48, 8)
        gesture.begin(10, 40f, 40f, true)
        assertFalse(gesture.move(0f, 20f))
        gesture.finish(60, 40f, 40f, clicked = true)
        gesture.begin(120, 43f, 42f, true)
        assertTrue(gesture.reserved)
        assertFalse(gesture.move(0f, 8f))
        assertTrue(gesture.move(0f, 9f))
        assertTrue(gesture.move(0f, 1f))
        gesture.finish(170, 43f, 51f, clicked = false)

        gesture.begin(200, 43f, 51f, true)
        assertFalse(gesture.reserved)
        assertFalse(gesture.scrolling)
    }

    @Test fun ordinaryDoubleClickDoesNotReserveTheThirdTap() {
        val gesture = DoubleTapScroll(300, 48, 8)
        gesture.finish(20, 12f, 12f, clicked = true)
        gesture.begin(100, 12f, 12f, true)
        assertTrue(gesture.reserved)
        assertFalse(gesture.move(1f, 1f))
        gesture.finish(120, 13f, 13f, clicked = true)
        gesture.begin(140, 13f, 13f, true)
        assertFalse(gesture.reserved)
    }

    @Test fun disabledExpiredDistantAndCancelledTapsCannotReserveScrolling() {
        val gesture = DoubleTapScroll(300, 48, 8)
        gesture.finish(100, 30f, 30f, clicked = true)
        gesture.begin(130, 30f, 30f, false)
        assertFalse(gesture.reserved)
        gesture.finish(100, 30f, 30f, clicked = true)
        gesture.begin(401, 30f, 30f, true)
        assertFalse(gesture.reserved)
        gesture.finish(100, 30f, 30f, clicked = true)
        gesture.begin(140, 80f, 30f, true)
        assertFalse(gesture.reserved)
        gesture.finish(160, 80f, 30f, clicked = true)
        gesture.reset()
        gesture.begin(180, 80f, 30f, true)
        assertFalse(gesture.reserved)
    }

    @Test fun gesturesApplySpeedReversalBoundsAndFractionalReset() {
        val motion = ScrollMotion()
        val settings = PointerSettings(scrollGestureSpeed = 2f, invertScroll = true)
        assertEquals(Input.Scroll(-14, 21), motion.gesture(2f, -3f, settings))
        assertNull(motion.gesture(0f, 0.1f, settings))
        motion.reset()
        assertNull(motion.gesture(0f, 0.1f, settings))
        assertEquals(Input.Scroll(-4096, 4096), motion.gesture(Float.MAX_VALUE, -Float.MAX_VALUE, settings))
        assertNull(motion.gesture(0f, 0f, settings))
    }

    @Test fun joystickIsVerticalProportionalAndBoundedAfterLongFrames() {
        val settings = PointerSettings()
        assertNull(ScrollMotion().stick(0.1f, 0.03, settings))
        val near = ScrollMotion().stick(0.4f, 0.05, settings)!!
        val far = ScrollMotion().stick(1f, 0.05, settings)!!
        assertEquals(0, near.dx)
        assertTrue(near.dy in 1 until far.dy)
        assertEquals(far, ScrollMotion().stick(8f, 5.0, settings))
        assertEquals(Input.Scroll(0, -far.dy * 2),
            ScrollMotion().stick(1f, 0.05, settings.copy(scrollStickSpeed = 2f, invertScroll = true)))

        fun distance(frames: Int): Int {
            val motion = ScrollMotion()
            return (0 until frames).sumOf { motion.stick(-1f, 1.0 / frames, settings)?.dy ?: 0 }
        }
        assertTrue(kotlin.math.abs(distance(60) - distance(120)) <= 1)
        assertTrue(distance(60) in -1680..-1679)
    }

    @Test fun olderSettingsAndInvalidScrollSpeedsUseSafeDefaults() {
        assertTrue(PointerSettings().doubleTapScroll)
        assertFalse(PointerSettings().invertScroll)
        assertEquals(PointerSettings(),
            PointerSettings(scrollGestureSpeed = Float.NaN, scrollStickSpeed = Float.POSITIVE_INFINITY).normalized())
        assertEquals(0.5f, PointerSettings(scrollGestureSpeed = 0f).normalized().scrollGestureSpeed)
        assertEquals(3f, PointerSettings(scrollStickSpeed = 4f).normalized().scrollStickSpeed)
    }
}

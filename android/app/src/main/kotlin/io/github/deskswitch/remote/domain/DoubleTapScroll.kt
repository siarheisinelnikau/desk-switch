package io.github.deskswitch.remote.domain

import kotlin.math.hypot

/** Keeps only one completed tap, so a third tap starts a fresh pair. */
internal class DoubleTapScroll(
    private val timeout: Long,
    private val doubleTapSlop: Int,
    private val touchSlop: Int,
) {
    private data class Tap(val time: Long, val x: Float, val y: Float)
    private var firstTap: Tap? = null
    var reserved = false
        private set
    var scrolling = false
        private set

    fun begin(time: Long, x: Float, y: Float, enabled: Boolean) {
        val tap = firstTap
        reset()
        reserved = enabled && tap != null && time - tap.time in 0..timeout &&
            hypot(x - tap.x, y - tap.y) <= doubleTapSlop
    }

    fun move(dx: Float, dy: Float): Boolean {
        if (reserved && !scrolling && hypot(dx, dy) > touchSlop) scrolling = true
        return scrolling
    }

    fun finish(time: Long, x: Float, y: Float, clicked: Boolean) {
        firstTap = if (clicked && !reserved) Tap(time, x, y) else null
        reserved = false
        scrolling = false
    }

    fun reset() {
        firstTap = null
        reserved = false
        scrolling = false
    }
}

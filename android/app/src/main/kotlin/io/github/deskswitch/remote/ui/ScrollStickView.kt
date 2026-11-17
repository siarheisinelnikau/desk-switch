package io.github.deskswitch.remote.ui

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.os.Bundle
import android.util.TypedValue
import android.view.Choreographer
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.accessibility.AccessibilityNodeInfo
import io.github.deskswitch.remote.R
import io.github.deskswitch.remote.domain.Input
import io.github.deskswitch.remote.domain.PointerSettings
import io.github.deskswitch.remote.domain.ScrollMotion
import kotlin.math.abs

internal class ScrollStickView(context: Context, private val send: (Input) -> Unit) : View(context) {
    private val colors = RemoteStyle.palette(context)
    private val density = resources.displayMetrics.density
    private val label = context.getString(R.string.scroll_label)
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        textSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 12f, resources.displayMetrics)
        strokeCap = Paint.Cap.ROUND
    }
    private var settings = PointerSettings()
    private val motion = ScrollMotion()
    private val choreographer = Choreographer.getInstance()
    private val frame = Choreographer.FrameCallback(::advance)
    private var scheduled = false
    private var lastFrame = 0L
    private var pointerId = -1
    private var startY = 0f
    private var displacement = 0f

    init {
        background = RemoteStyle.interactive(context)
        contentDescription = context.getString(R.string.scroll_joystick_accessibility)
        tooltipText = context.getString(R.string.scroll_joystick_tooltip)
        isFocusable = true
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
    }

    fun preferences(value: PointerSettings) {
        if (settings.scrollStickSpeed != value.scrollStickSpeed || settings.invertScroll != value.invertScroll) {
            cancelInteraction()
        }
        settings = value
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val centerY = height / 2f
        val fitsLabel = paint.measureText(label) + 33 * density <= width
        val centerX = if (fitsLabel) 18 * density else width / 2f
        paint.color = colors.muted
        paint.strokeWidth = 1.4f * density
        for (direction in -1..1 step 2) {
            val tip = centerY + direction * 16 * density
            val edge = tip - direction * 4 * density
            canvas.drawLine(centerX, tip, centerX - 4 * density, edge, paint)
            canvas.drawLine(centerX, tip, centerX + 4 * density, edge, paint)
        }
        if (fitsLabel) canvas.drawText(label, 30 * density, centerY - (paint.ascent() + paint.descent()) / 2, paint)
        paint.color = colors.accent
        canvas.drawCircle(centerX, centerY + displacement * 8 * density, 4.5f * density, paint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                cancelInteraction()
                pointerId = event.getPointerId(0)
                startY = event.y
                isPressed = true
                parent.requestDisallowInterceptTouchEvent(true)
            }
            MotionEvent.ACTION_MOVE -> {
                val index = event.findPointerIndex(pointerId)
                if (index < 0) cancelInteraction()
                else {
                    displacement = ((event.getY(index) - startY) / (38 * density)).coerceIn(-1f, 1f)
                    if (abs(displacement) <= ScrollMotion.STICK_DEAD_ZONE) stopFrames()
                    else if (!scheduled) scheduleFrame()
                    invalidate()
                }
            }
            MotionEvent.ACTION_POINTER_UP -> if (event.getPointerId(event.actionIndex) == pointerId) cancelInteraction()
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> cancelInteraction()
        }
        return true
    }

    private fun scheduleFrame() {
        scheduled = true
        choreographer.postFrameCallback(frame)
    }

    private fun advance(time: Long) {
        scheduled = false
        if (pointerId < 0 || abs(displacement) <= ScrollMotion.STICK_DEAD_ZONE) return
        if (lastFrame == 0L) lastFrame = time
        else if (time - lastFrame >= 16_000_000L) {
            motion.stick(displacement, (time - lastFrame) / 1_000_000_000.0, settings)?.let(send)
            lastFrame = time
        }
        scheduleFrame()
    }

    private fun stopFrames() {
        if (scheduled) choreographer.removeFrameCallback(frame)
        scheduled = false
        lastFrame = 0
        motion.reset()
    }

    fun cancelInteraction() {
        stopFrames()
        pointerId = -1
        displacement = 0f
        isPressed = false
        invalidate()
    }

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        if (!hasWindowFocus) cancelInteraction()
    }

    override fun onDetachedFromWindow() {
        cancelInteraction()
        super.onDetachedFromWindow()
    }

    override fun onInitializeAccessibilityNodeInfo(info: AccessibilityNodeInfo) {
        super.onInitializeAccessibilityNodeInfo(info)
        info.isScrollable = true
        info.addAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_FORWARD)
        info.addAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_BACKWARD)
    }

    override fun performAccessibilityAction(action: Int, arguments: Bundle?): Boolean = when (action) {
        AccessibilityNodeInfo.ACTION_SCROLL_FORWARD -> step(1)
        AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD -> step(-1)
        else -> super.performAccessibilityAction(action, arguments)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = when (keyCode) {
        KeyEvent.KEYCODE_DPAD_DOWN -> step(1)
        KeyEvent.KEYCODE_DPAD_UP -> step(-1)
        else -> super.onKeyDown(keyCode, event)
    }

    private fun step(direction: Int): Boolean {
        send(Input.Scroll(0, direction * 120 * ScrollMotion.direction(settings)))
        return true
    }
}

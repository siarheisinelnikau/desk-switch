package io.github.deskswitch.remote.ui

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.RectF
import android.graphics.Shader
import android.os.SystemClock
import android.view.Choreographer
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.animation.DecelerateInterpolator
import io.github.deskswitch.remote.R
import io.github.deskswitch.remote.domain.DoubleTapScroll
import io.github.deskswitch.remote.domain.Input
import io.github.deskswitch.remote.domain.PointerMode
import io.github.deskswitch.remote.domain.PointerMotion
import io.github.deskswitch.remote.domain.PointerSettings
import io.github.deskswitch.remote.domain.ScrollMotion
import kotlin.math.hypot
import kotlin.math.min

/** One gesture surface. Joystick frames exist only while the finger requests movement. */
class TouchpadView(context: Context, private val send: (Input) -> Unit) : View(context) {
    private val colors = RemoteStyle.palette(context)
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
    private val density = resources.displayMetrics.density
    private val configuration = ViewConfiguration.get(context)
    private val slop = configuration.scaledTouchSlop
    private val doubleTap = DoubleTapScroll(ViewConfiguration.getDoubleTapTimeout().toLong(), configuration.scaledDoubleTapSlop, slop)
    private val motion = PointerMotion()
    private val scroll = ScrollMotion()
    private var preferences = PointerSettings()
    private var surface: Shader? = null
    private var preview: Bitmap? = null
    private val previewBounds = RectF()
    private val previewClip = Path()
    private val idleTypeface = android.graphics.Typeface.create("sans-serif-medium", android.graphics.Typeface.NORMAL)
    private var hintTitle = ""
    private var hintDetails = ""
    private val touchpadGlyph = GlyphDrawable(context, Glyph.TOUCHPAD, colors.touchpadGlyph)
    private val joystickGlyph = GlyphDrawable(context, Glyph.JOYSTICK, colors.touchpadGlyph)
    private var hintAlpha = 1f
    private var hintTarget = 1f
    private var hintAnimation: ValueAnimator? = null
    private var hintForeground = false
    private var remoteTouchHeld = false
    private var hintRevealAt = 0L
    private val revealHint = Runnable { updateHint() }
    private val halo = RadialGradient(0f, 0f, 58 * density,
        intArrayOf(colors.touchpadGlow, Color.TRANSPARENT), null, Shader.TileMode.CLAMP)
    private val haloTransform = Matrix()
    private var lastX = 0f; private var lastY = 0f
    private var startX = 0f; private var startY = 0f
    private var anchorX = 0f; private var anchorY = 0f
    private var stickX = 0f; private var stickY = 0f
    private var began = 0L
    private var down = false; private var two = false; private var moved = false; private var dragging = false
    private var feedback = 0f
    private var visualStick = false
    private var feedbackAnimation: ValueAnimator? = null
    private var scheduled = false
    private var lastFrameNanos = 0L
    private var nextInputNanos = 0L
    private val inputPeriodNanos = 16_666_667L
    private val choreographer = Choreographer.getInstance()
    private val frame = Choreographer.FrameCallback { time -> advance(time) }
    private val radius: Float get() = min(88f * density, min(width, height) * 0.31f).coerceAtLeast(1f)
    private val hold = Runnable {
        if (down && !two && !moved) {
            dragging = true
            send(Input.Button(272, true))
            performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
            invalidate()
        }
    }

    init {
        isClickable = true
        isFocusable = true
        minimumHeight = (120 * density).toInt()
        updateDescription()
    }

    fun preferences(value: PointerSettings) {
        val next = value.normalized()
        if (next == preferences) return
        if (next.mode != preferences.mode || next.doubleTapScroll != preferences.doubleTapScroll) cancelInteractions(false)
        if (next.scrollGestureSpeed != preferences.scrollGestureSpeed || next.invertScroll != preferences.invertScroll) scroll.reset()
        preferences = next
        updateDescription()
        invalidate()
    }

    /** Hold one visible image only; replaced images are left to Android's renderer/GC. */
    fun preview(bitmap: Bitmap?) {
        if (preview === bitmap) return
        val changedVisibility = (preview == null) != (bitmap == null)
        preview = bitmap
        if (changedVisibility) updateHint()
        invalidate()
    }

    fun setHintForeground(active: Boolean) {
        hintForeground = active
        if (!active && remoteTouchHeld) hintRevealAt = SystemClock.uptimeMillis() + HINT_IDLE_MS
        if (!active) remoteTouchHeld = false
        updateHint()
    }

    fun remoteTouch(held: Boolean) {
        if (remoteTouchHeld == held) return
        remoteTouchHeld = held
        if (!held) hintRevealAt = SystemClock.uptimeMillis() + HINT_IDLE_MS
        updateHint()
    }

    fun noteRemoteActivity() {
        if (remoteTouchHeld) return
        hintRevealAt = SystemClock.uptimeMillis() + HINT_IDLE_MS
        updateHint()
    }

    private fun updateHint() {
        removeCallbacks(revealHint)
        if (!hintForeground || !isAttachedToWindow || !isShown || windowVisibility != VISIBLE ||
            !hasWindowFocus() || preview != null) {
            hintAnimation?.cancel()
            hintAnimation = null
            return
        }

        val wait = hintRevealAt - SystemClock.uptimeMillis()
        if (remoteTouchHeld || wait > 0) {
            animateHint(0f)
            if (!remoteTouchHeld) postDelayed(revealHint, wait)
        } else animateHint(1f)
    }

    private fun animateHint(target: Float) {
        if (hintTarget == target && hintAnimation?.isStarted == true) return
        hintAnimation?.cancel()
        hintAnimation = null
        hintTarget = target
        if (hintAlpha == target) return
        if (!ValueAnimator.areAnimatorsEnabled()) {
            hintAlpha = target
            invalidate()
            return
        }
        hintAnimation = ValueAnimator.ofFloat(hintAlpha, target).apply {
            duration = if (target == 0f) 140L else 300L
            interpolator = DecelerateInterpolator()
            addUpdateListener {
                hintAlpha = it.animatedValue as Float
                invalidate()
            }
            start()
        }
    }

    private fun updateDescription() {
        val joystick = preferences.mode == PointerMode.JOYSTICK
        val description = context.getString(if (joystick) R.string.joystick_accessibility else R.string.touchpad_accessibility)
        contentDescription = if (preferences.doubleTapScroll) {
            context.getString(R.string.double_tap_accessibility, description)
        } else description
        hintTitle = context.getString(if (joystick) R.string.joystick_hint else R.string.touchpad_hint)
        hintDetails = context.getString(when {
            preferences.doubleTapScroll -> R.string.double_tap_hint
            joystick -> R.string.joystick_faster_hint
            else -> R.string.touchpad_gestures_hint
        })
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        surface = LinearGradient(0f, 0f, w.toFloat(), h.toFloat(),
            intArrayOf(colors.touchpadTop, colors.touchpadMiddle, colors.touchpadBottom), null, Shader.TileMode.CLAMP)
        previewClip.reset()
        previewClip.addRoundRect(density, density, w - density, h - density, 26 * density, 26 * density, Path.Direction.CW)
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val inset = density
        val corner = 26 * density
        paint.style = Paint.Style.FILL
        paint.shader = surface
        canvas.drawRoundRect(inset, inset, width - inset, height - inset, corner, corner, paint)
        paint.shader = null
        val image = preview
        if (image != null && !image.isRecycled) {
            val scale = maxOf((width - inset * 2) / image.width, (height - inset * 2) / image.height)
            val w = image.width * scale
            val h = image.height * scale
            previewBounds.set((width - w) / 2, (height - h) / 2, (width + w) / 2, (height + h) / 2)
            val saved = canvas.save()
            canvas.clipPath(previewClip)
            canvas.drawBitmap(image, null, previewBounds, paint)
            canvas.restoreToCount(saved)
        }
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = density
        paint.color = colors.touchpadBorder
        canvas.drawRoundRect(inset, inset, width - inset, height - inset, corner, corner, paint)
        paint.style = Paint.Style.FILL
        val roomy = height > 190 * density
        if (image == null && hintAlpha > 0f) {
            val size = (if (roomy) 64 else 35) * density
            val centerY = height / 2f - if (roomy) 24 * density else 8 * density
            val layer = if (hintAlpha < 1f) {
                canvas.saveLayerAlpha(0f, (centerY - size / 2).coerceAtLeast(0f), width.toFloat(),
                    height.toFloat(), (hintAlpha * 255).toInt())
            } else null
            val glyph = if (preferences.mode == PointerMode.JOYSTICK) joystickGlyph else touchpadGlyph
            glyph.setBounds((width / 2f - size / 2).toInt(), (centerY - size / 2).toInt(), (width / 2f + size / 2).toInt(), (centerY + size / 2).toInt())
            glyph.draw(canvas)
            paint.color = colors.ink
            paint.typeface = idleTypeface
            paint.textAlign = Paint.Align.CENTER
            paint.textSize = android.util.TypedValue.applyDimension(android.util.TypedValue.COMPLEX_UNIT_SP,
                if (roomy) 18f else 14f, resources.displayMetrics)
            drawHintLine(canvas, hintTitle, centerY + size / 2 + (if (roomy) 30 else 20) * density)
            if (roomy) {
                paint.color = colors.muted
                paint.textSize = android.util.TypedValue.applyDimension(android.util.TypedValue.COMPLEX_UNIT_SP, 12f, resources.displayMetrics)
                drawHintLine(canvas, hintDetails, centerY + size / 2 + 54 * density)
            }
            if (layer != null) canvas.restoreToCount(layer)
        }
        if (feedback > 0.01f) {
            if (visualStick) drawStick(canvas)
            else {
                haloTransform.setTranslate(lastX, lastY)
                halo.setLocalMatrix(haloTransform)
                paint.shader = halo
                paint.alpha = (255 * feedback).toInt()
                canvas.drawCircle(lastX, lastY, 58 * density, paint)
                paint.shader = null
                paint.color = if (dragging) colors.success else colors.accent
                paint.alpha = (220 * feedback).toInt()
                canvas.drawCircle(lastX, lastY, 6 * density, paint)
                paint.style = Paint.Style.STROKE; paint.strokeWidth = 1.5f * density
                paint.alpha = (120 * feedback).toInt()
                canvas.drawCircle(lastX, lastY, 22 * density, paint)
                paint.style = Paint.Style.FILL; paint.alpha = 255
            }
        }
    }

    private fun drawHintLine(canvas: Canvas, label: String, baseline: Float) {
        val availableWidth = (width - 16 * density).coerceAtLeast(1f)
        val labelWidth = paint.measureText(label)
        if (labelWidth > availableWidth) paint.textSize *= availableWidth / labelWidth
        canvas.drawText(label, width / 2f, baseline, paint)
    }

    private fun drawStick(canvas: Canvas) {
        val r = radius
        val x = anchorX.coerceIn(r + 10 * density, (width - r - 10 * density).coerceAtLeast(r + 10 * density))
        val y = anchorY.coerceIn(r + 10 * density, (height - r - 10 * density).coerceAtLeast(r + 10 * density))
        paint.color = colors.touchpadFeedbackFill
        paint.alpha = (100 * feedback).toInt()
        canvas.drawCircle(x, y, r, paint)
        paint.style = Paint.Style.STROKE; paint.strokeWidth = 1.5f * density
        paint.color = colors.touchpadFeedbackStroke
        paint.alpha = (160 * feedback).toInt()
        canvas.drawCircle(x, y, r, paint)
        paint.alpha = (50 * feedback).toInt()
        canvas.drawCircle(x, y, r * 0.6f, paint)
        val length = hypot(stickX, stickY).coerceAtLeast(1f)
        val xx = x + stickX / length * r * 0.66f
        val yy = y + stickY / length * r * 0.66f
        paint.strokeWidth = 3 * density
        canvas.drawLine(x, y, xx, yy, paint)
        paint.style = Paint.Style.FILL
        paint.color = if (dragging) colors.success else colors.accent
        paint.alpha = (240 * feedback).toInt()
        canvas.drawCircle(xx, yy, r * 0.29f, paint)
        paint.color = colors.ink
        canvas.drawCircle(xx, yy, r * 0.1f, paint)
        paint.alpha = 255
    }

    private fun animateFeedback(target: Float) {
        feedbackAnimation?.cancel()
        feedbackAnimation = ValueAnimator.ofFloat(feedback, target).apply {
            duration = if (target > feedback) 130 else 220
            interpolator = DecelerateInterpolator()
            addUpdateListener { feedback = it.animatedValue as Float; invalidate() }
            start()
        }
    }

    private fun requestFrame() {
        if (!scheduled && down && !two && hypot(stickX, stickY) > PointerMotion.DEAD_ZONE) {
            scheduled = true
            lastFrameNanos = System.nanoTime()
            nextInputNanos = lastFrameNanos + inputPeriodNanos
            choreographer.postFrameCallback(frame)
        }
    }

    private fun advance(time: Long) {
        scheduled = false
        if (!down || two || preferences.mode != PointerMode.JOYSTICK) return
        val seconds = (time - lastFrameNanos).coerceAtLeast(0) / 1_000_000_000.0
        if (time >= nextInputNanos) {
            val step = motion.joystick(stickX, stickY, seconds, preferences.joystickSpeed)
            if (step.dx != 0 || step.dy != 0) send(Input.Move(step.dx, step.dy))
            lastFrameNanos = time
            nextInputNanos += inputPeriodNanos
            if (nextInputNanos < time - inputPeriodNanos) nextInputNanos = time + inputPeriodNanos
        }
        if (hypot(stickX, stickY) > PointerMotion.DEAD_ZONE) {
            scheduled = true
            choreographer.postFrameCallback(frame)
        }
    }

    private fun stopFrames() {
        choreographer.removeFrameCallback(frame)
        scheduled = false
        lastFrameNanos = 0
        nextInputNanos = 0
        motion.reset()
    }

    private fun click(code: Int) { send(Input.Button(code, true)); send(Input.Button(code, false)) }
    override fun performClick(): Boolean { super.performClick(); noteRemoteActivity(); click(272); return true }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                val previousFeedback = feedback
                finishInteraction(false)
                feedback = previousFeedback
                down = true; startX = event.x; startY = event.y; lastX = event.x; lastY = event.y
                doubleTap.begin(event.eventTime, event.x, event.y, preferences.doubleTapScroll)
                visualStick = !doubleTap.reserved && preferences.mode == PointerMode.JOYSTICK
                anchorX = event.x; anchorY = event.y; began = event.eventTime
                parent.requestDisallowInterceptTouchEvent(true)
                if (!doubleTap.reserved) postDelayed(hold, ViewConfiguration.getLongPressTimeout().toLong())
                animateFeedback(1f)
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                if (!down) return true
                removeCallbacks(hold); stopFrames()
                if (doubleTap.reserved) moved = true
                doubleTap.reset()
                if (dragging) { send(Input.Button(272, false)); dragging = false }
                two = true
                visualStick = false
                lastX = (event.getX(0) + event.getX(1)) / 2
                lastY = (event.getY(0) + event.getY(1)) / 2
                startX = lastX; startY = lastY
                scroll.reset()
            }
            MotionEvent.ACTION_POINTER_UP -> {
                // Rebase if one of the two fingers used for the centroid changes.
                if (two && event.pointerCount > 2) {
                    val first = if (event.actionIndex == 0) 1 else 0
                    val second = if (event.actionIndex <= 1) 2 else 1
                    lastX = (event.getX(first) + event.getX(second)) / 2
                    lastY = (event.getY(first) + event.getY(second)) / 2
                }
            }
            MotionEvent.ACTION_MOVE -> {
                if (!down) return true
                val x = if (event.pointerCount >= 2) (event.getX(0) + event.getX(1)) / 2 else event.x
                val y = if (event.pointerCount >= 2) (event.getY(0) + event.getY(1)) / 2 else event.y
                if (hypot(x - startX, y - startY) > slop) { moved = true; removeCallbacks(hold) }
                if ((two && event.pointerCount >= 2) || doubleTap.move(x - startX, y - startY)) {
                    scroll.gesture((x - lastX) / density, (y - lastY) / density, preferences)?.let(send)
                } else if (doubleTap.reserved) {
                    // Keep the second touch quiet until it becomes a click or a scroll.
                } else if (!two && preferences.mode == PointerMode.JOYSTICK) {
                    stickX = (x - anchorX) / radius
                    stickY = (y - anchorY) / radius
                    if (hypot(stickX, stickY) <= PointerMotion.DEAD_ZONE) stopFrames() else requestFrame()
                } else if (!two) {
                    val step = motion.touchpad(x - lastX, y - lastY, density, preferences.touchpadSpeed)
                    if (step.dx != 0 || step.dy != 0) send(Input.Move(step.dx, step.dy))
                }
                lastX = x; lastY = y
            }
            MotionEvent.ACTION_UP -> {
                val tap = down && !moved && !dragging && event.eventTime - began < ViewConfiguration.getLongPressTimeout() &&
                    (two || hypot(event.x - startX, event.y - startY) <= slop)
                if (tap) {
                    if (two) click(273) else performClick()
                }
                doubleTap.finish(event.eventTime, event.x, event.y, tap && !two)
                finishInteraction(true)
            }
            MotionEvent.ACTION_CANCEL -> cancelInteractions()
        }
        invalidate()
        return true
    }

    fun cancelInteractions(animate: Boolean = true) {
        doubleTap.reset()
        finishInteraction(animate)
        invalidate()
    }

    private fun finishInteraction(animate: Boolean) {
        removeCallbacks(hold); stopFrames()
        if (dragging) send(Input.Button(272, false))
        down = false; two = false; moved = false; dragging = false
        if (!animate) { stickX = 0f; stickY = 0f }
        scroll.reset()
        if (animate) animateFeedback(0f) else { feedbackAnimation?.cancel(); feedback = 0f }
    }

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        if (!hasWindowFocus) {
            remoteTouch(false)
            cancelInteractions(false)
        }
        updateHint()
    }

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        if (!isVisible) remoteTouch(false)
        updateHint()
    }

    override fun onDetachedFromWindow() {
        removeCallbacks(revealHint)
        hintAnimation?.cancel()
        hintAnimation = null
        cancelInteractions(false)
        preview = null
        feedbackAnimation?.cancel()
        feedbackAnimation = null
        super.onDetachedFromWindow()
    }

    private companion object {
        const val HINT_IDLE_MS = 15_000L
    }
}

package io.github.deskswitch.remote.ui

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.view.MotionEvent
import android.view.View
import android.view.animation.DecelerateInterpolator

internal object RemoteStyle {
    val background = Color.rgb(13, 18, 25)
    val surface = Color.rgb(22, 32, 45)
    val raised = Color.rgb(29, 43, 59)
    val border = Color.rgb(45, 62, 81)
    val ink = Color.rgb(233, 241, 249)
    val muted = Color.rgb(158, 179, 199)
    val accent = Color.rgb(75, 196, 244)
    val accentInk = Color.rgb(8, 29, 41)
    val success = Color.rgb(96, 221, 180)
    val danger = Color.rgb(255, 160, 151)

    fun dp(context: Context, value: Float) = value * context.resources.displayMetrics.density

    fun shape(context: Context, color: Int = surface, radius: Float = 18f, stroke: Int? = border): GradientDrawable =
        GradientDrawable().apply {
            setColor(color)
            cornerRadius = dp(context, radius)
            stroke?.let { setStroke(dp(context, 1f).toInt().coerceAtLeast(1), it) }
        }

    fun interactive(context: Context, primary: Boolean = false, selected: Boolean = false): Drawable {
        val color = when { primary -> accent; selected -> Color.rgb(30, 65, 83); else -> surface }
        return RippleDrawable(ColorStateList.valueOf(Color.argb(45, 150, 222, 255)),
            shape(context, color, if (selected) 13f else 17f, if (selected) accent else if (primary) null else border),
            shape(context, Color.WHITE, 17f, null))
    }

    /** Finite press feedback; no animation work remains after release/cancel. */
    fun press(view: View) {
        view.setOnTouchListener { target, event ->
            val scale = when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> 0.965f
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> 1f
                else -> return@setOnTouchListener false
            }
            target.animate().cancel()
            target.animate().scaleX(scale).scaleY(scale).setDuration(if (scale < 1f) 90 else 160)
                .setInterpolator(DecelerateInterpolator()).start()
            false
        }
    }
}

internal enum class Glyph { BRAND, SETTINGS, DISCONNECT, KEYBOARD, TOUCHPAD, JOYSTICK, LEFT_CLICK, RIGHT_CLICK, BACK, ENTER, SPACE, ADD, COMPUTER, CHEVRON, CLOSE, CHECK, WIFI }

/** Small code-native icons share one optical size and stroke weight. */
internal class GlyphDrawable(private val context: Context, private val glyph: Glyph, private var color: Int) : Drawable() {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1.65f
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }
    override fun getIntrinsicWidth() = RemoteStyle.dp(context, 22f).toInt()
    override fun getIntrinsicHeight() = intrinsicWidth
    override fun draw(canvas: Canvas) {
        val saved = canvas.save()
        canvas.translate(bounds.left.toFloat(), bounds.top.toFloat())
        canvas.scale(bounds.width() / 24f, bounds.height() / 24f)
        paint.color = color
        paint.style = Paint.Style.STROKE
        fun line(x: Float, y: Float, xx: Float, yy: Float) = canvas.drawLine(x, y, xx, yy, paint)
        fun path(vararg points: Float) {
            val value = Path().apply { moveTo(points[0], points[1]); for (i in 2 until points.size step 2) lineTo(points[i], points[i + 1]) }
            canvas.drawPath(value, paint)
        }
        when (glyph) {
            Glyph.BRAND -> {
                canvas.drawRoundRect(3f, 3f, 15f, 15f, 2f, 2f, paint)
                paint.color = RemoteStyle.background; paint.style = Paint.Style.FILL
                canvas.drawRoundRect(8f, 8f, 22f, 22f, 2f, 2f, paint)
                paint.color = color; paint.style = Paint.Style.STROKE
                canvas.drawRoundRect(9f, 9f, 21f, 21f, 2f, 2f, paint)
                line(9f, 12.5f, 21f, 12.5f)
            }
            Glyph.SETTINGS -> {
                line(4f, 7f, 20f, 7f); line(4f, 17f, 20f, 17f)
                paint.style = Paint.Style.FILL; paint.color = RemoteStyle.surface
                canvas.drawCircle(9f, 7f, 3f, paint); canvas.drawCircle(15f, 17f, 3f, paint)
                paint.style = Paint.Style.STROKE; paint.color = color
                canvas.drawCircle(9f, 7f, 3f, paint); canvas.drawCircle(15f, 17f, 3f, paint)
            }
            Glyph.DISCONNECT -> {
                canvas.drawArc(4.5f, 4.5f, 19.5f, 19.5f, -50f, 280f, false, paint)
                line(12f, 2.5f, 12f, 11f)
            }
            Glyph.KEYBOARD -> {
                canvas.drawRoundRect(2f, 5f, 22f, 19f, 3f, 3f, paint)
                paint.strokeWidth = 1.9f
                for (x in listOf(6f, 10f, 14f, 18f)) { line(x, 9f, x, 9.1f); line(x, 12f, x, 12.1f) }
                line(8f, 15.5f, 16f, 15.5f)
            }
            Glyph.TOUCHPAD -> {
                canvas.drawRoundRect(3f, 3f, 21f, 21f, 4f, 4f, paint)
                line(3f, 16f, 21f, 16f); line(12f, 16f, 12f, 21f)
                line(10f, 9f, 14f, 9f); line(12f, 7f, 12f, 11f)
            }
            Glyph.JOYSTICK -> {
                canvas.drawCircle(12f, 12f, 9f, paint)
                canvas.drawCircle(14f, 10f, 3.5f, paint)
                line(4f, 12f, 6f, 12f); line(12f, 18f, 12f, 20f)
            }
            Glyph.LEFT_CLICK, Glyph.RIGHT_CLICK -> {
                canvas.drawRoundRect(5f, 2.5f, 19f, 21.5f, 7f, 7f, paint)
                line(5f, 10f, 19f, 10f); line(12f, 2.5f, 12f, 10f)
                paint.style = Paint.Style.FILL
                canvas.drawRoundRect(if (glyph == Glyph.LEFT_CLICK) 7f else 14f, 5f,
                    if (glyph == Glyph.LEFT_CLICK) 10f else 17f, 8f, 1f, 1f, paint)
            }
            Glyph.BACK -> path(10f, 5f, 3f, 12f, 10f, 19f).also { line(3f, 12f, 21f, 12f) }
            Glyph.ENTER -> path(20f, 4f, 20f, 14f, 4f, 14f).also { path(9f, 9f, 4f, 14f, 9f, 19f) }
            Glyph.SPACE -> path(3f, 9f, 3f, 15f, 21f, 15f, 21f, 9f)
            Glyph.ADD -> { line(12f, 4f, 12f, 20f); line(4f, 12f, 20f, 12f) }
            Glyph.COMPUTER -> {
                canvas.drawRoundRect(2f, 3f, 22f, 16.5f, 2.5f, 2.5f, paint)
                line(12f, 17f, 12f, 21f); line(7f, 21f, 17f, 21f)
            }
            Glyph.CHEVRON -> path(9f, 5f, 16f, 12f, 9f, 19f)
            Glyph.CLOSE -> { line(6f, 6f, 18f, 18f); line(6f, 18f, 18f, 6f) }
            Glyph.CHECK -> path(4f, 12f, 9f, 17f, 20f, 6f)
            Glyph.WIFI -> {
                canvas.drawArc(2f, 5f, 22f, 25f, 225f, 90f, false, paint)
                canvas.drawArc(6f, 10f, 18f, 22f, 225f, 90f, false, paint)
                paint.style = Paint.Style.FILL; canvas.drawCircle(12f, 19f, 1.3f, paint)
            }
        }
        paint.strokeWidth = 1.65f
        canvas.restoreToCount(saved)
    }
    override fun setAlpha(alpha: Int) { paint.alpha = alpha }
    override fun setColorFilter(colorFilter: ColorFilter?) { paint.colorFilter = colorFilter }
    @Deprecated("Deprecated in Java") override fun getOpacity() = PixelFormat.TRANSLUCENT
}

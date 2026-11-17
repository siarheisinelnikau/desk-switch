package io.github.deskswitch.remote.ui

import android.content.Context
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection

/** IME composition stays on the phone. Only committed text is sent to the PC. */
class RemoteKeyboard(context: Context, private val text: (String) -> Unit, private val tap: (Int) -> Unit) : View(context) {
    private var composing = ""
    init { isFocusable = true; isFocusableInTouchMode = true; contentDescription = "Remote keyboard" }
    override fun onCheckIsTextEditor() = true
    override fun onCreateInputConnection(info: EditorInfo): InputConnection {
        info.inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
        info.imeOptions = EditorInfo.IME_ACTION_GO or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        return object : BaseInputConnection(this, false) {
            override fun setComposingText(value: CharSequence?, position: Int): Boolean { composing = value?.toString().orEmpty(); return true }
            override fun commitText(value: CharSequence?, position: Int): Boolean {
                composing = ""; value?.toString()?.takeIf { it.isNotEmpty() }?.let(text); return true
            }
            override fun finishComposingText(): Boolean {
                val value = composing; composing = ""; if (value.isNotEmpty()) text(value); return true
            }
            override fun deleteSurroundingText(before: Int, after: Int): Boolean {
                if (composing.isNotEmpty()) {
                    val count = composing.codePointCount(0, composing.length)
                    composing = composing.substring(0, composing.offsetByCodePoints(0, (count - before.coerceAtLeast(0)).coerceAtLeast(0)))
                } else repeat(before.coerceIn(0, 64)) { tap(14) }
                repeat(after.coerceIn(0, 64)) { tap(111) }
                return true
            }
            override fun deleteSurroundingTextInCodePoints(before: Int, after: Int) = deleteSurroundingText(before, after)
            override fun performEditorAction(action: Int): Boolean { finishComposingText(); tap(28); return true }
            override fun sendKeyEvent(event: KeyEvent): Boolean {
                if (event.action != KeyEvent.ACTION_DOWN) return true
                val code = when (event.keyCode) { KeyEvent.KEYCODE_DEL -> 14; KeyEvent.KEYCODE_ENTER -> 28; KeyEvent.KEYCODE_TAB -> 15; KeyEvent.KEYCODE_DPAD_UP -> 103; KeyEvent.KEYCODE_DPAD_DOWN -> 108; KeyEvent.KEYCODE_DPAD_LEFT -> 105; KeyEvent.KEYCODE_DPAD_RIGHT -> 106; else -> null }
                if (code != null) { if (code != 14) finishComposingText(); tap(code) }
                else event.unicodeChar.takeIf { it != 0 }?.let { text(String(Character.toChars(it))) }
                return true
            }
        }
    }
    fun discardComposition() { composing = ""; clearFocus() }
}

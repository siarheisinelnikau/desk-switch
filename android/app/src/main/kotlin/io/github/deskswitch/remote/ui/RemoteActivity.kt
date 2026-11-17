package io.github.deskswitch.remote.ui

import android.app.AlertDialog
import android.app.Dialog
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.text.InputType
import android.text.TextUtils
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.view.WindowManager
import android.view.animation.DecelerateInterpolator
import android.view.inputmethod.InputMethodManager
import android.widget.*
import androidx.activity.ComponentActivity
import androidx.activity.viewModels
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import io.github.deskswitch.remote.BuildConfig
import io.github.deskswitch.remote.R
import io.github.deskswitch.remote.data.ComputerDiscovery
import io.github.deskswitch.remote.data.NoiseSessionFactory
import io.github.deskswitch.remote.data.PhoneStore
import io.github.deskswitch.remote.data.PointerPreferences
import io.github.deskswitch.remote.data.phoneName
import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.Pairing
import io.github.deskswitch.remote.domain.PointerMode
import io.github.deskswitch.remote.domain.PointerSettings
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.withContext
import java.util.Locale

class RemoteActivity : ComponentActivity() {
    private val model: RemoteViewModel by viewModels {
        object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = RemoteViewModel(
                PhoneStore(applicationContext),
                { onTrusted -> NoiseSessionFactory(phoneName(applicationContext), onTrusted) },
                PointerPreferences(applicationContext),
            ) as T
        }
    }
    private var discovery: ComputerDiscovery? = null
    private lateinit var content: LinearLayout
    private var last: ScreenState? = null
    private var keyboard: RemoteKeyboard? = null
    private var keyboardButton: Button? = null
    private var touchpad: TouchpadView? = null
    private val largeControls = mutableListOf<View>()
    private val modeButtons = mutableMapOf<PointerMode, Button>()
    private var settingsDialog: Dialog? = null
    private var previewMessage: TextView? = null
    private var remotePreviewError: TextView? = null
    private var previewVisible = false
    private var typing = false
    private val landscape get() = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
    private val captureCase by lazy {
        if (BuildConfig.DEBUG) intent.getStringExtra("preview")?.takeIf {
            it in setOf("remote", "joystick", "nearby", "settings", "pairing")
        } else null
    }
    private val previewComputer = Computer("Living room PC", "192.168.1.20")
    private fun dp(value: Int) = RemoteStyle.dp(this, value.toFloat()).toInt()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            @Suppress("DEPRECATION")
            window.setDecorFitsSystemWindows(false)
            window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING)
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
        }
        content = column().apply { setBackgroundColor(RemoteStyle.background) }
        content.setOnApplyWindowInsetsListener { view, insets ->
            val padding: IntArray
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                val bars = insets.getInsets(android.view.WindowInsets.Type.systemBars() or android.view.WindowInsets.Type.ime())
                padding = intArrayOf(bars.left, bars.top, bars.right, bars.bottom)
                typing = insets.isVisible(android.view.WindowInsets.Type.ime())
            } else {
                @Suppress("DEPRECATION")
                padding = intArrayOf(insets.systemWindowInsetLeft, insets.systemWindowInsetTop,
                    insets.systemWindowInsetRight, insets.systemWindowInsetBottom.coerceAtMost(dp(48)))
                @Suppress("DEPRECATION")
                typing = insets.systemWindowInsetBottom > dp(150)
            }
            applyTypingLayout()
            view.setPadding(dp(20) + padding[0], dp(8) + padding[1], dp(20) + padding[2], dp(12) + padding[3])
            insets
        }
        setContentView(content)
        if (captureCase != null) {
            val controls = PointerSettings(mode = if (captureCase == "joystick") PointerMode.JOYSTICK else PointerMode.TOUCHPAD)
            val state = when (captureCase) {
                "remote", "joystick" -> ScreenState(connected = previewComputer, controls = controls)
                "pairing" -> ScreenState(pairing = Pairing(previewComputer, "284 761", "01".repeat(32)), controls = controls)
                else -> ScreenState(nearby = listOf(previewComputer), controls = controls)
            }
            render(state)
            if (captureCase == "settings") content.post { settings() } else capture(content)
        } else lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                launch { model.state.collect(::render) }
                launch {
                    model.inputErrors.collect { error ->
                        if (previewVisible && settingsDialog == null) {
                            Toast.makeText(this@RemoteActivity, error, Toast.LENGTH_SHORT).show()
                        }
                    }
                }
                launch {
                    model.previewFrames.collectLatest { bytes ->
                        if (bytes == null) { touchpad?.preview(null); return@collectLatest }
                        val destination = touchpad
                        var decoded: android.graphics.Bitmap? = null
                        try {
                            withContext(Dispatchers.Default) { decoded = PreviewImage.decode(bytes) }
                            currentCoroutineContext().ensureActive()
                            if (destination != null && destination === touchpad && model.state.value.controls.previewEnabled &&
                                model.previewFrames.value === bytes) {
                                destination.preview(decoded)
                                decoded = null
                            }
                        } finally { decoded?.recycle() }
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        if (captureCase == null) model.discovered(emptyList())
        if (captureCase == null) discovery = runCatching {
            ComputerDiscovery(this, { model.discovered(it) }, { model.discoveryFailed(it) })
        }.onFailure {
            model.discoveryFailed("Wi-Fi discovery is unavailable. Reopen the app to retry, or add a computer by address.")
        }.getOrNull()
    }

    override fun onResume() {
        super.onResume()
        previewVisible = true
        if (captureCase == null) model.foreground(settingsDialog == null)
    }

    override fun onPause() {
        previewVisible = false
        if (captureCase == null) model.foreground(false)
        touchpad?.preview(null)
        touchpad?.cancelInteractions(false)
        super.onPause()
    }

    override fun onStop() {
        content.animate().cancel()
        discovery?.close(); discovery = null
        keyboard?.discardComposition()
        settingsDialog?.dismiss(); settingsDialog = null
        if (captureCase == null && !isChangingConfigurations) model.disconnect()
        super.onStop()
    }

    private fun column() = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
    private fun text(value: String, size: Float = 16f, color: Int = RemoteStyle.ink, medium: Boolean = false) = TextView(this).apply {
        text = value; textSize = size; setTextColor(color)
        typeface = Typeface.create(if (medium) "sans-serif-medium" else "sans-serif", Typeface.NORMAL)
        setLineSpacing(dp(3).toFloat(), 1f)
    }
    private fun add(parent: LinearLayout, view: View, gap: Int = 12, weight: Float = 0f) {
        parent.addView(view, LinearLayout.LayoutParams(-1, if (weight > 0) 0 else -2, weight).apply { bottomMargin = dp(gap) })
    }
    private fun button(value: String, primary: Boolean = false, glyph: Glyph? = null, action: () -> Unit) = Button(this).apply {
        text = value; isAllCaps = false; textSize = 15f; setTextColor(if (primary) RemoteStyle.accentInk else RemoteStyle.ink)
        background = RemoteStyle.interactive(this@RemoteActivity, primary)
        stateListAnimator = null
        minHeight = dp(52); minimumWidth = 0; setPadding(dp(12), dp(8), dp(12), dp(8))
        isSingleLine = true; ellipsize = TextUtils.TruncateAt.END
        if (glyph != null) {
            setCompoundDrawablesWithIntrinsicBounds(GlyphDrawable(this@RemoteActivity, glyph,
                if (primary) RemoteStyle.accentInk else RemoteStyle.muted), null, null, null)
            compoundDrawablePadding = dp(9)
        }
        contentDescription = value
        RemoteStyle.press(this)
        setOnClickListener { action() }
    }
    private fun icon(glyph: Glyph, description: String, color: Int = RemoteStyle.muted, action: () -> Unit) = ImageButton(this).apply {
        setImageDrawable(GlyphDrawable(this@RemoteActivity, glyph, color))
        background = RemoteStyle.interactive(this@RemoteActivity)
        setPadding(dp(13), dp(13), dp(13), dp(13))
        contentDescription = description; tooltipText = description
        scaleType = ImageView.ScaleType.CENTER_INSIDE
        minimumWidth = dp(48); minimumHeight = dp(48)
        RemoteStyle.press(this)
        setOnClickListener { action() }
    }
    private fun row(vararg views: View) = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL
        views.forEachIndexed { index, view -> addView(view, LinearLayout.LayoutParams(0, -2, 1f).apply {
            if (index > 0) leftMargin = dp(8)
        }) }
    }
    private fun appBar(state: ScreenState): View = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
        val logo = ImageView(this@RemoteActivity).apply {
            setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.BRAND, RemoteStyle.accent))
            setPadding(dp(6), dp(6), dp(6), dp(6))
            importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
        }
        addView(logo, LinearLayout.LayoutParams(dp(42), dp(42)).apply { rightMargin = dp(10) })
        val titles = column()
        add(titles, text("Desk Switch", 20f, medium = true), 2)
        val connected = state.connected
        add(titles, text(if (connected == null) "PHONE REMOTE" else "Connected · ${connected.name}", 12f,
            if (connected == null) RemoteStyle.muted else RemoteStyle.success).apply { isSingleLine = true; ellipsize = TextUtils.TruncateAt.END }, 0)
        addView(titles, LinearLayout.LayoutParams(0, -2, 1f))
        addView(icon(Glyph.SETTINGS, "Control settings") { settings() }.apply { id = R.id.settings },
            LinearLayout.LayoutParams(dp(48), dp(48)).apply { leftMargin = dp(10) })
        if (connected != null) addView(icon(Glyph.DISCONNECT, "Disconnect from workspace", RemoteStyle.danger) {
            touchpad?.cancelInteractions(false); hideKeyboard(); model.disconnect()
        }.apply { id = R.id.disconnect }, LinearLayout.LayoutParams(dp(48), dp(48)).apply { leftMargin = dp(8) })
    }

    private fun render(state: ScreenState) {
        updatePreviewMessage(state)
        if (state.connected != null && state.connected == last?.connected && state.error == last?.error) {
            updateControls(state.controls)
            last = state
            return
        }
        if (state.pairing != null && state.pairing == last?.pairing) { last = state; return }
        val previous = last
        last = state
        touchpad?.cancelInteractions(false)
        keyboard?.discardComposition(); keyboard = null; keyboardButton = null
        touchpad = null; remotePreviewError = null; largeControls.clear(); modeButtons.clear()
        content.removeAllViews()
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        add(content, appBar(state), if (landscape) 10 else 22)
        when {
            state.pairing != null -> pairing(state.pairing)
            state.connected != null -> remote(state.connected, state.controls)
            state.connecting != null -> connecting(state.connecting)
            else -> computers(state)
        }
        applyTypingLayout()
        if (captureCase == null && previous != null && previous.connected != state.connected) {
            content.animate().cancel()
            content.alpha = 0f; content.translationY = dp(9).toFloat()
            content.animate().alpha(1f).translationY(0f).setDuration(240).setInterpolator(DecelerateInterpolator()).start()
        }
    }

    private fun computers(state: ScreenState) {
        add(content, text(if (landscape) "Your PC. In your hand." else "Your PC.\nIn your hand.", if (landscape) 26f else 34f, medium = true), 10)
        add(content, text("Choose a computer on your Wi-Fi.", 15f, RemoteStyle.muted), 22)
        (state.error ?: state.discoveryError)?.let {
            add(content, text(it, 14f, RemoteStyle.danger).apply {
                setPadding(dp(15), dp(12), dp(15), dp(12))
                background = RemoteStyle.shape(this@RemoteActivity, Color.rgb(51, 32, 37), 16f, Color.rgb(99, 58, 63))
            }, 14)
        }
        val list = column()
        val known = state.saved
        val nearby = state.nearby.filterNot { found -> known.any { it.host == found.host } }
        if (known.isNotEmpty()) {
            add(list, text("YOUR COMPUTERS", 11f, RemoteStyle.muted, true), 10)
            known.forEach { add(list, computerCard(it, "Saved computer"), 10) }
        }
        if (nearby.isNotEmpty()) {
            add(list, text("NEARBY", 11f, RemoteStyle.muted, true), 10)
            nearby.forEach { add(list, computerCard(it, "Ready to connect"), 10) }
        }
        if (known.isEmpty() && nearby.isEmpty()) {
            val empty = column().apply {
                gravity = Gravity.CENTER; setPadding(dp(20), dp(28), dp(20), dp(28))
                background = RemoteStyle.shape(this@RemoteActivity)
            }
            empty.addView(ImageView(this).apply { setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.WIFI, RemoteStyle.accent)) }, LinearLayout.LayoutParams(dp(42), dp(42)))
            add(empty, text("Looking for nearby computers…", 16f, medium = true).apply { gravity = Gravity.CENTER; setPadding(0, dp(18), 0, 0) }, 8)
            add(empty, text("Open Desk Switch on your computer\nto make it available here.", 14f, RemoteStyle.muted).apply { gravity = Gravity.CENTER }, 0)
            add(list, empty, 0)
        }
        add(content, ScrollView(this).apply { isFillViewport = false; overScrollMode = View.OVER_SCROLL_NEVER; addView(list) }, 16, 1f)
        add(content, button("Add a computer", glyph = Glyph.ADD) { address() }.apply { id = R.id.connect }, 0)
    }

    private fun computerCard(computer: Computer, status: String): View = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(16), dp(16), dp(16), dp(16))
        background = RemoteStyle.interactive(this@RemoteActivity)
        minimumHeight = dp(92); isClickable = true; isFocusable = true
        contentDescription = "Connect to ${computer.name}, ${computer.host}"
        val art = ImageView(this@RemoteActivity).apply {
            setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.COMPUTER, RemoteStyle.accent))
            setPadding(dp(12), dp(12), dp(12), dp(12))
            background = RemoteStyle.shape(this@RemoteActivity, Color.rgb(26, 56, 75), 15f, null)
        }
        addView(art, LinearLayout.LayoutParams(dp(54), dp(54)).apply { rightMargin = dp(14) })
        val titles = column()
        add(titles, text(computer.name, 17f, medium = true).apply { isSingleLine = true; ellipsize = TextUtils.TruncateAt.END }, 4)
        add(titles, text(status, 12f, RemoteStyle.muted), 3)
        add(titles, text(computer.host, 11f, RemoteStyle.muted), 0)
        addView(titles, LinearLayout.LayoutParams(0, -2, 1f))
        addView(ImageView(this@RemoteActivity).apply { setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.CHEVRON, RemoteStyle.muted)) }, LinearLayout.LayoutParams(dp(22), dp(22)))
        RemoteStyle.press(this)
        setOnClickListener { model.connect(computer) }
    }

    private fun pairing(pairing: Pairing) {
        val card = FrameLayout(this).apply {
            background = RemoteStyle.shape(this@RemoteActivity, RemoteStyle.surface, 26f)
            addView(column().apply {
                gravity = Gravity.CENTER
                addView(text("PAIRING CODE", 11f, RemoteStyle.muted, true).apply { gravity = Gravity.CENTER })
                addView(text(pairing.code, 44f, RemoteStyle.accent).apply {
                    typeface = Typeface.MONOSPACE; gravity = Gravity.CENTER
                    isSingleLine = true
                    setAutoSizeTextTypeUniformWithConfiguration(20, 44, 1, android.util.TypedValue.COMPLEX_UNIT_SP)
                    setPadding(dp(8), dp(20), dp(8), dp(12))
                })
                addView(text("Confirm it on both screens", 13f, RemoteStyle.muted).apply { gravity = Gravity.CENTER })
            }, FrameLayout.LayoutParams(-1, -2, Gravity.CENTER))
        }
        val approve = button("Codes match", true, Glyph.CHECK) { model.approve(true) }.apply { id = R.id.pair }
        val cancel = button("Cancel") { model.approve(false) }
        if (landscape) {
            val body = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
            body.addView(card, LinearLayout.LayoutParams(0, -1, 1f).apply { rightMargin = dp(18) })
            val actions = column()
            add(actions, text("Check the code on\n${pairing.computer.name}.", 15f, RemoteStyle.muted), 16)
            add(actions, approve, 10); add(actions, cancel, 0)
            body.addView(actions, LinearLayout.LayoutParams(dp(210), -2))
            add(content, body, 0, 1f)
        } else {
            add(content, text("Confirm pairing", 30f, medium = true), 12)
            add(content, text("Check that this code appears in Desk Switch on ${pairing.computer.name}.", 16f, RemoteStyle.muted), 20)
            add(content, card, 20, 1f)
            add(content, approve, 10); add(content, cancel, 0)
        }
    }

    private fun connecting(computer: Computer) {
        add(content, text("Getting connected", 29f, medium = true), 12)
        add(content, text("Confirm the connection in Desk Switch on ${computer.name}.", 16f, RemoteStyle.muted), 20)
        add(content, FrameLayout(this).apply {
            addView(ProgressBar(this@RemoteActivity).apply { indeterminateTintList = android.content.res.ColorStateList.valueOf(RemoteStyle.accent) },
                FrameLayout.LayoutParams(dp(42), dp(42), Gravity.CENTER))
        }, 20, 1f)
        add(content, button("Cancel") { model.disconnect() }, 0)
    }

    private fun modeControl(settings: PointerSettings, register: Boolean = true): View {
        val bar = row()
        val buttons = mutableMapOf<PointerMode, Button>()
        bar.background = RemoteStyle.shape(this, Color.rgb(17, 26, 37), 18f, null)
        bar.setPadding(dp(4), dp(4), dp(4), dp(4))
        for (mode in PointerMode.entries) {
            val selected = mode == settings.mode
            val control = button(if (mode == PointerMode.TOUCHPAD) "Touchpad" else "Joystick", glyph = if (mode == PointerMode.TOUCHPAD) Glyph.TOUCHPAD else Glyph.JOYSTICK) {
                setControls(currentControls().copy(mode = mode))
                for ((candidate, button) in buttons) selectMode(button, candidate == mode)
            }.apply {
                minHeight = dp(44); textSize = 14f
                background = RemoteStyle.interactive(this@RemoteActivity, selected = selected)
                setTextColor(if (selected) RemoteStyle.accent else RemoteStyle.muted)
                isSelected = selected
            }
            buttons[mode] = control
            if (register) modeButtons[mode] = control
            bar.addView(control, LinearLayout.LayoutParams(0, -2, 1f).apply { if (mode == PointerMode.JOYSTICK) leftMargin = dp(4) })
        }
        return bar
    }

    private fun remote(@Suppress("UNUSED_PARAMETER") computer: Computer, settings: PointerSettings) {
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val root = LinearLayout(this).apply { orientation = if (landscape) LinearLayout.HORIZONTAL else LinearLayout.VERTICAL }
        val surface = column()
        add(surface, modeControl(settings).also { largeControls.add(it) }, 12)
        add(surface, text(last?.previewError.orEmpty(), 13f, RemoteStyle.danger).apply {
            maxLines = 3
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
            visibility = if (last?.previewError == null || typing) View.GONE else View.VISIBLE
            remotePreviewError = this
        }, 8)
        val pad = TouchpadView(this, model::input).apply { id = R.id.touchpad; preferences(settings); touchpad = this }
        add(surface, pad, if (landscape) 0 else 12, 1f)
        val controls = column()
        val left = button("Left click", glyph = Glyph.LEFT_CLICK) { model.click(272) }
        val right = button("Right click", glyph = Glyph.RIGHT_CLICK) { model.click(273) }
        if (landscape) { add(controls, left, 8); add(controls, right, 10) }
        else add(controls, row(left, right), 10)
        val shortcutButtons = arrayOf(
            button("Back") { model.chord(56, 105) },
            button("Space") { model.tap(57) },
            button("Enter") { model.tap(28) },
        )
        if (landscape) shortcutButtons.forEach { key ->
            key.textSize = 13f
            key.setPaddingRelative(dp(4), key.paddingTop, dp(4), key.paddingBottom)
        }
        val shortcuts = row(*shortcutButtons)
        add(controls, shortcuts.also { largeControls.add(it) }, 10)
        add(controls, button("Keyboard", true, Glyph.KEYBOARD) {
            if (typing) hideKeyboard(false) else showKeyboard()
        }.apply { id = R.id.keyboard; keyboardButton = this }, 0)
        if (landscape) {
            root.addView(surface, LinearLayout.LayoutParams(0, -1, 1f).apply { rightMargin = dp(14) })
            root.addView(controls, LinearLayout.LayoutParams(dp(190), -2))
        } else {
            root.addView(surface, LinearLayout.LayoutParams(-1, 0, 1f))
            root.addView(controls, LinearLayout.LayoutParams(-1, -2))
        }
        add(content, root, 0, 1f)
    }

    private fun updateControls(settings: PointerSettings) {
        touchpad?.preferences(settings)
        for ((mode, control) in modeButtons) {
            selectMode(control, mode == settings.mode)
        }
    }
    private fun selectMode(button: Button, selected: Boolean) {
        if (button.isSelected != selected) {
            button.isSelected = selected
            button.background = RemoteStyle.interactive(this, selected = selected)
            button.setTextColor(if (selected) RemoteStyle.accent else RemoteStyle.muted)
        }
    }
    private fun currentControls() = if (captureCase == null) model.state.value.controls else last?.controls ?: PointerSettings()
    private fun updatePreviewMessage(state: ScreenState) {
        remotePreviewError?.apply {
            text = state.previewError.orEmpty()
            visibility = if (state.previewError == null || typing) View.GONE else View.VISIBLE
        }
        previewMessage?.apply {
            text = state.previewError ?: "Shows the area around the PC pointer. Allow cursor preview in Desk Switch on the computer."
            setTextColor(if (state.previewError == null) RemoteStyle.muted else RemoteStyle.danger)
        }
    }
    private fun setControls(settings: PointerSettings) {
        if (captureCase == null) model.controls(settings)
        else { last = last?.copy(controls = settings); updateControls(settings) }
    }
    private fun applyTypingLayout() {
        largeControls.forEach { it.visibility = if (typing) View.GONE else View.VISIBLE }
        remotePreviewError?.visibility = if (typing || last?.previewError == null) View.GONE else View.VISIBLE
        keyboardButton?.apply {
            text = if (typing) "Hide keyboard" else "Keyboard"
            contentDescription = text
        }
        touchpad?.minimumHeight = dp(if (typing) 70 else 120)
    }
    private fun hideKeyboard(discard: Boolean = true) {
        keyboard?.let { (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).hideSoftInputFromWindow(it.windowToken, 0) }
        if (discard) keyboard?.discardComposition()
    }
    private fun showKeyboard() {
        touchpad?.cancelInteractions()
        if (keyboard == null) {
            keyboard = RemoteKeyboard(this, { value ->
                runCatching { model.text(value) }.onFailure {
                    Toast.makeText(this, "Text is too long. Send a shorter selection.", Toast.LENGTH_SHORT).show()
                }
            }, model::tap).also { content.addView(it, ViewGroup.LayoutParams(1, 1)) }
        }
        keyboard!!.requestFocus()
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).showSoftInput(keyboard, InputMethodManager.SHOW_IMPLICIT)
    }
    private fun address() {
        val input = EditText(this).apply {
            id = R.id.address; hint = "192.168.1.20"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            isSingleLine = true; setPadding(dp(20), dp(16), dp(20), dp(16))
        }
        AlertDialog.Builder(this).setTitle("Add a computer").setMessage("Enter its local IP address or hostname.").setView(input)
            .setNegativeButton("Cancel", null).setPositiveButton("Connect") { _, _ ->
                val host = input.text.toString().trim()
                if (host.isNotEmpty() && host.length <= 253 && host.none(Char::isWhitespace) && !host.contains('/')) model.connect(Computer("Computer", host))
                else Toast.makeText(this, "Enter a local IP address or hostname", Toast.LENGTH_SHORT).show()
            }.show()
    }

    private fun settings() {
        if (settingsDialog?.isShowing == true) return
        if (captureCase == null) model.foreground(false)
        touchpad?.cancelInteractions(false); hideKeyboard()
        val settings = last?.controls ?: PointerSettings()
        val sheet = column().apply {
            setPadding(dp(22), dp(20), dp(22), dp(22))
            background = RemoteStyle.shape(this@RemoteActivity, RemoteStyle.surface, 28f)
        }
        val heading = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        heading.addView(text("Controls", 23f, medium = true), LinearLayout.LayoutParams(0, -2, 1f))
        heading.addView(icon(Glyph.CLOSE, "Close control settings") { settingsDialog?.dismiss() }, LinearLayout.LayoutParams(dp(48), dp(48)).apply { leftMargin = dp(8) })
        add(sheet, heading, 14)
        val list = column()
        add(list, text("CONTROL MODE", 11f, RemoteStyle.muted, true), 10)
        add(list, modeControl(settings, register = false), 22)
        slider(list, "Touchpad speed", settings.touchpadSpeed, 0.35f, 3f, { "%.1f×".format(Locale.US, it) }) { value ->
            setControls(currentControls().copy(touchpadSpeed = value))
        }
        slider(list, "Joystick speed", settings.joystickSpeed, 300f, 2400f, { "${it.toInt()} px/s" }) { value ->
            setControls(currentControls().copy(joystickSpeed = value))
        }
        add(list, text("Push farther from the centre to move faster. Lift your finger to stop.", 13f, RemoteStyle.muted), 24)
        val fpsControls = column()
        slider(fpsControls, "Preview frame rate", settings.previewFps.toFloat(), 1f, 30f,
            { "${kotlin.math.round(it).toInt()} fps" }) { value ->
            setControls(currentControls().copy(previewFps = kotlin.math.round(value).toInt()))
        }
        fpsControls.visibility = if (settings.previewEnabled) View.VISIBLE else View.GONE
        val previewToggle = Switch(this).apply {
            text = "Cursor preview"; textSize = 16f; setTextColor(RemoteStyle.ink)
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            contentDescription = "Show cursor preview from computer"
            minimumHeight = dp(52); switchPadding = dp(18)
            isChecked = settings.previewEnabled
            thumbTintList = android.content.res.ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()),
                intArrayOf(RemoteStyle.accent, RemoteStyle.muted))
            trackTintList = android.content.res.ColorStateList.valueOf(Color.rgb(42, 69, 86))
            setOnCheckedChangeListener { _, enabled ->
                setControls(currentControls().copy(previewEnabled = enabled))
                fpsControls.visibility = if (enabled) View.VISIBLE else View.GONE
                if (enabled && captureCase == null) fpsControls.apply {
                    alpha = 0f
                    animate().alpha(1f).setDuration(180).start()
                }
            }
        }
        add(list, previewToggle, 4)
        add(list, text("Shows the area around the PC pointer. Allow cursor preview in Desk Switch on the computer.", 13f, RemoteStyle.muted)
            .also { previewMessage = it }, 14)
        add(list, fpsControls, 20)
        updatePreviewMessage(last ?: ScreenState())
        val saved = if (captureCase != null) listOf(previewComputer) else model.state.value.saved
        if (saved.isNotEmpty()) {
            add(list, text("YOUR COMPUTERS", 11f, RemoteStyle.muted, true), 12)
            saved.forEach { computer ->
                val line = row(text(computer.name, 15f).apply { maxLines = 2 }, button("Forget") {
                    model.forget(computer)
                    settingsDialog?.dismiss()
                    Toast.makeText(this, "Computer forgotten", Toast.LENGTH_SHORT).show()
                })
                add(list, line, 12)
            }
        }
        add(list, text("Desk Switch Remote ${BuildConfig.VERSION_NAME}", 12f, RemoteStyle.muted), 8)
        add(list, text("Control pauses when this app leaves the screen.", 12f, RemoteStyle.muted), 14)
        add(list, button("Open-source license") {
            val license = javaClass.classLoader!!.getResourceAsStream("licenses/noise-java/LICENSE.txt")!!.bufferedReader().use { it.readText() }
            AlertDialog.Builder(this).setTitle("Noise-Java · MIT").setMessage(license).setPositiveButton("Done", null).show()
        }, 0)
        val scroll = ScrollView(this).apply {
            overScrollMode = View.OVER_SCROLL_NEVER
            scrollBarStyle = View.SCROLLBARS_INSIDE_INSET
            addView(list)
        }
        add(sheet, scroll, 0, 1f)
        val maxHeight = (resources.displayMetrics.heightPixels * 0.84).toInt()
        val dialog = Dialog(this).apply {
            requestWindowFeature(Window.FEATURE_NO_TITLE)
            setContentView(sheet)
            window?.setBackgroundDrawableResource(android.R.color.transparent)
            window?.setGravity(Gravity.BOTTOM)
            window?.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
            window?.setDimAmount(0.6f)
            setOnDismissListener {
                settingsDialog = null; previewMessage = null
                if (captureCase == null) model.foreground(previewVisible)
            }
            setOnShowListener {
                window?.setLayout(-1, maxHeight)
                if (captureCase == "settings") capture(window!!.decorView)
                else {
                    sheet.translationY = dp(24).toFloat(); sheet.alpha = 0f
                    sheet.animate().translationY(0f).alpha(1f).setDuration(220).setInterpolator(DecelerateInterpolator()).start()
                }
            }
        }
        settingsDialog = dialog
        dialog.show()
    }

    private fun slider(parent: LinearLayout, title: String, value: Float, minimum: Float, maximum: Float,
        format: (Float) -> String, changed: (Float) -> Unit) {
        val labels = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        labels.addView(text(title, 16f, medium = true), LinearLayout.LayoutParams(0, -2, 1f))
        val amount = text(format(value), 14f, RemoteStyle.accent)
        labels.addView(amount, LinearLayout.LayoutParams(-2, -2).apply { leftMargin = dp(12) })
        add(parent, labels, 4)
        add(parent, SeekBar(this).apply {
            max = 100; progress = ((value - minimum) / (maximum - minimum) * 100).toInt()
            contentDescription = title
            minimumHeight = dp(48)
            progressTintList = android.content.res.ColorStateList.valueOf(RemoteStyle.accent)
            thumbTintList = android.content.res.ColorStateList.valueOf(RemoteStyle.accent)
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(seekBar: SeekBar, progress: Int, fromUser: Boolean) {
                    if (fromUser) {
                        val next = minimum + (maximum - minimum) * progress / 100
                        amount.text = format(next); changed(next)
                    }
                }
                override fun onStartTrackingTouch(seekBar: SeekBar) = Unit
                override fun onStopTrackingTouch(seekBar: SeekBar) = Unit
            })
        }, 14)
    }

    private fun capture(view: View, attempt: Int = 0) {
        if (!BuildConfig.DEBUG || captureCase == null) return
        view.postDelayed({
            if (view.width <= 0 || view.height <= 0) {
                if (attempt < 20) capture(view, attempt + 1) else finish()
            } else {
                try {
                    val bitmap = android.graphics.Bitmap.createBitmap(view.width, view.height, android.graphics.Bitmap.Config.ARGB_8888)
                    view.draw(android.graphics.Canvas(bitmap))
                    java.io.File(getExternalFilesDir(null), "preview-$captureCase.png").outputStream().use {
                        bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                    }
                    val layout = org.json.JSONArray()
                    fun describe(node: View, x: Int, y: Int, depth: Int) {
                        if (depth > 12 || layout.length() >= 256) return
                        val item = org.json.JSONObject().put("view", node.javaClass.simpleName)
                            .put("x", x).put("y", y).put("width", node.width).put("height", node.height)
                        if (node is TextView) item.put("text", node.text.toString().take(120))
                        layout.put(item)
                        if (node is ViewGroup) for (index in 0 until node.childCount) {
                            val child = node.getChildAt(index)
                            describe(child, x + child.left - node.scrollX, y + child.top - node.scrollY, depth + 1)
                        }
                    }
                    describe(view, 0, 0, 0)
                    java.io.File(getExternalFilesDir(null), "preview-$captureCase-layout.json").writeText(layout.toString())
                    bitmap.recycle()
                } finally { finish() }
            }
        }, 100)
    }
}

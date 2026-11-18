package io.github.deskswitch.remote.ui

import android.app.Activity
import android.app.AlertDialog
import android.app.Dialog
import android.content.Context
import android.content.res.Configuration
import android.graphics.Typeface
import android.os.Bundle
import android.text.InputType
import android.text.TextUtils
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.Window
import android.view.WindowManager
import android.view.animation.DecelerateInterpolator
import android.view.inputmethod.InputMethodManager
import android.widget.*
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import io.github.deskswitch.remote.BuildConfig
import io.github.deskswitch.remote.R
import io.github.deskswitch.remote.data.ApkInstaller
import io.github.deskswitch.remote.data.AppearancePreferences
import io.github.deskswitch.remote.data.ComputerDiscovery
import io.github.deskswitch.remote.data.NativeWire
import io.github.deskswitch.remote.data.NoiseSessionFactory
import io.github.deskswitch.remote.data.PhoneStore
import io.github.deskswitch.remote.data.PointerPreferences
import io.github.deskswitch.remote.data.UpdatePreferences
import io.github.deskswitch.remote.data.phoneName
import io.github.deskswitch.remote.domain.AppLanguage
import io.github.deskswitch.remote.domain.AppTheme
import io.github.deskswitch.remote.domain.AppearanceSettings
import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.Pairing
import io.github.deskswitch.remote.domain.PointerMode
import io.github.deskswitch.remote.domain.PointerSettings
import io.github.deskswitch.remote.update.GitHubUpdates
import io.github.deskswitch.remote.update.UpdateFailure
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.withContext

private const val RESTORE_CONTROLS = "restoreControls"
private const val UPDATES_EXPANDED = "updatesExpanded"

class RemoteActivity : ComponentActivity() {
    private val model: RemoteViewModel by viewModels {
        object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = RemoteViewModel(
                PhoneStore(applicationContext),
                { onTrusted -> NoiseSessionFactory(BuildConfig.VERSION_NAME, phoneName(applicationContext), onTrusted) },
                PointerPreferences(applicationContext),
                AppearancePreferences(applicationContext),
            ) as T
        }
    }
    private val installer by lazy { ApkInstaller(applicationContext) }
    private val updates: UpdateViewModel by viewModels {
        object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T = UpdateViewModel(
                BuildConfig.VERSION_NAME, GitHubUpdates(), installer, UpdatePreferences(applicationContext),
            ) as T
        }
    }
    private val updateSystemActivity = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (!updates.handingOff) return@registerForActivityResult
        updates.systemReturned(result.resultCode == Activity.RESULT_OK, result.resultCode == Activity.RESULT_CANCELED)
        if (settingsDialog == null && captureCase == null) {
            restoreControls = true
            content.post { restoreSettingsIfRequested() }
        }
    }
    private var updatesExpanded = false
    private var renderUpdates: ((UpdateScreenState) -> Unit)? = null
    private var discovery: ComputerDiscovery? = null
    private lateinit var content: LinearLayout
    private var last: ScreenState? = null
    private var keyboard: RemoteKeyboard? = null
    private var keyboardButton: Button? = null
    private var touchpad: TouchpadView? = null
    private var scrollStick: ScrollStickView? = null
    private var shortcuts: View? = null
    private var modeButton: ImageButton? = null
    private var settingsDialog: Dialog? = null
    private var appearancePicker: AlertDialog? = null
    private var restoreControls = false
    private var previewMessage: TextView? = null
    private var remotePreviewError: TextView? = null
    private var previewVisible = false
    private var typing = false
    private val landscape get() = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
    private val captureCase by lazy {
        if (BuildConfig.DEBUG) intent.getStringExtra("preview")?.takeIf {
            it in setOf("remote", "joystick", "nearby", "settings", "pairing", "updates")
        } else null
    }
    private val previewComputer = Computer("Living room PC", "192.168.1.20")
    private val colors by lazy(LazyThreadSafetyMode.NONE) { RemoteStyle.palette(this) }
    private fun dp(value: Int) = RemoteStyle.dp(this, value.toFloat()).toInt()

    override fun attachBaseContext(newBase: Context) {
        super.attachBaseContext(newBase.withAppearance(AppearancePreferences(newBase).load()))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        val light = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK != Configuration.UI_MODE_NIGHT_YES
        setTheme(if (light) R.style.LightAppTheme else R.style.AppTheme)
        super.onCreate(savedInstanceState)
        restoreControls = savedInstanceState?.getBoolean(RESTORE_CONTROLS) == true
        updatesExpanded = savedInstanceState?.getBoolean(UPDATES_EXPANDED) == true
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            @Suppress("DEPRECATION")
            window.setDecorFitsSystemWindows(false)
            window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING)
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = window.decorView.systemUiVisibility or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
        }
        content = column().apply { setBackgroundColor(colors.background) }
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
            view.setPadding(dp(20) + padding[0], dp(if (landscape) 4 else 8) + padding[1], dp(20) + padding[2], dp(12) + padding[3])
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
            if (captureCase == "settings" || captureCase == "updates") {
                updatesExpanded = captureCase == "updates"
                content.post { settings() }
            } else capture(content)
        } else lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                launch { model.state.collect(::render) }
                launch { updates.state.collect { renderUpdates?.invoke(it) } }
                launch {
                    repeatOnLifecycle(Lifecycle.State.RESUMED) {
                        updates.effects.collect(::handleUpdateEffect)
                    }
                }
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
        if (captureCase == null) {
            updates.startup()
            if (restoreControls) content.post { restoreSettingsIfRequested() }
        }
    }

    private fun restoreSettingsIfRequested() {
        if (!restoreControls || isFinishing || isDestroyed) return
        restoreControls = false
        settings()
    }

    private fun handleUpdateEffect(effect: UpdateEffect) {
        if (effect is UpdateEffect.StartupNotice) {
            Toast.makeText(this, getString(R.string.update_available, effect.version), Toast.LENGTH_SHORT).show()
            return
        }
        try {
            val intent = when (effect) {
                UpdateEffect.RequestPermission -> installer.permissionIntent()
                is UpdateEffect.OpenInstaller -> installer.installIntent(effect.file)
                is UpdateEffect.StartupNotice -> return
            }
            updateSystemActivity.launch(intent)
        } catch (_: android.content.ActivityNotFoundException) {
            updates.handoffFailed()
        } catch (_: SecurityException) {
            updates.handoffFailed()
        } catch (_: IllegalArgumentException) {
            updates.handoffFailed()
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(RESTORE_CONTROLS, restoreControls || settingsDialog?.isShowing == true)
        outState.putBoolean(UPDATES_EXPANDED, updatesExpanded)
    }

    override fun onStart() {
        super.onStart()
        if (captureCase == null) model.discovered(emptyList())
        if (captureCase == null) discovery = runCatching {
            ComputerDiscovery(this, { model.discovered(it) }, { code ->
                model.discoveryFailed(code?.let { ScreenIssue.Local(R.string.discovery_failed_code, it) })
            })
        }.onFailure {
            model.discoveryFailed(ScreenIssue.Local(R.string.discovery_unavailable))
        }.getOrNull()
    }

    override fun onResume() {
        super.onResume()
        previewVisible = true
        touchpad?.setHintForeground(true)
        if (captureCase == null) model.foreground(settingsDialog == null && !restoreControls)
    }

    override fun onPause() {
        previewVisible = false
        touchpad?.setHintForeground(false)
        if (captureCase == null) model.foreground(false)
        touchpad?.preview(null)
        touchpad?.cancelInteractions(false)
        scrollStick?.cancelInteraction()
        super.onPause()
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val pad = touchpad
        if (event.actionMasked == MotionEvent.ACTION_DOWN) pad?.remoteTouch(true)
        val handled = super.dispatchTouchEvent(event)
        if (!handled || event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
            pad?.remoteTouch(false)
        }
        return handled
    }

    override fun onStop() {
        content.animate().cancel()
        discovery?.close(); discovery = null
        keyboard?.discardComposition()
        settingsDialog?.dismiss(); settingsDialog = null
        if (captureCase == null && !isChangingConfigurations && !updates.handingOff) model.disconnect()
        super.onStop()
    }

    override fun onDestroy() {
        appearancePicker?.dismiss()
        settingsDialog?.dismiss()
        super.onDestroy()
    }

    private fun column() = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
    private fun text(value: String, size: Float = 16f, color: Int = colors.ink, medium: Boolean = false) = TextView(this).apply {
        text = value; textSize = size; setTextColor(color)
        typeface = Typeface.create(if (medium) "sans-serif-medium" else "sans-serif", Typeface.NORMAL)
        setLineSpacing(dp(3).toFloat(), 1f)
    }
    private fun add(parent: LinearLayout, view: View, gap: Int = 12, weight: Float = 0f) {
        parent.addView(view, LinearLayout.LayoutParams(-1, if (weight > 0) 0 else -2, weight).apply { bottomMargin = dp(gap) })
    }
    private fun button(value: String, primary: Boolean = false, glyph: Glyph? = null, action: () -> Unit) = Button(this).apply {
        text = value; isAllCaps = false; textSize = 15f; setTextColor(if (primary) colors.accentInk else colors.ink)
        background = RemoteStyle.interactive(this@RemoteActivity, primary)
        stateListAnimator = null
        minHeight = dp(52); minimumWidth = 0; setPadding(dp(12), dp(8), dp(12), dp(8))
        isSingleLine = true; ellipsize = TextUtils.TruncateAt.END
        if (glyph != null) {
            setCompoundDrawablesWithIntrinsicBounds(GlyphDrawable(this@RemoteActivity, glyph,
                if (primary) colors.accentInk else colors.muted), null, null, null)
            compoundDrawablePadding = dp(9)
        }
        contentDescription = value
        RemoteStyle.press(this)
        setOnClickListener { action() }
    }
    private fun icon(glyph: Glyph, description: String, color: Int = colors.muted, action: () -> Unit) = ImageButton(this).apply {
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
        orientation = LinearLayout.HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL
        val connected = state.connected
        val narrow = resources.configuration.screenWidthDp < 400
        if (landscape || connected == null || !narrow) {
            val logo = ImageView(this@RemoteActivity).apply {
                setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.BRAND, colors.accent))
                setPadding(dp(5), dp(5), dp(5), dp(5))
                importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
            }
            val size = dp(if (landscape) 34 else 42)
            addView(logo, LinearLayout.LayoutParams(size, size).apply { marginEnd = dp(8) })
        }
        val title = text(getString(R.string.brand), if (landscape) 16f else 20f, medium = true).apply {
            isSingleLine = true
            ellipsize = TextUtils.TruncateAt.END
        }
        val status = text(if (connected == null) getString(R.string.phone_remote) else getString(R.string.connected_computer, connected.name),
            12f, if (connected == null) colors.muted else colors.success).apply {
            isSingleLine = true
            ellipsize = TextUtils.TruncateAt.END
        }
        val titles = if (landscape) LinearLayout(this@RemoteActivity).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            addView(title, LinearLayout.LayoutParams(-2, -2))
            addView(status, LinearLayout.LayoutParams(0, -2, 1f).apply { marginStart = dp(12) })
        } else column().apply {
            add(this, title, 2)
            add(this, status, 0)
        }
        addView(titles, LinearLayout.LayoutParams(0, -2, 1f))
        if (connected != null) {
            val toggle = icon(Glyph.TOUCHPAD, "", colors.accent) {
                val settings = currentControls()
                setControls(settings.copy(mode = alternateMode(settings.mode)))
            }.apply { id = R.id.pointer_mode }
            toggle.background = RemoteStyle.interactive(this@RemoteActivity, selected = true)
            modeButton = toggle
            updateModeButton(state.controls.mode)
            addView(toggle, LinearLayout.LayoutParams(dp(48), dp(48)).apply { marginStart = dp(8) })
        }
        addView(icon(Glyph.SETTINGS, getString(R.string.control_settings)) { settings() }.apply { id = R.id.settings },
            LinearLayout.LayoutParams(dp(48), dp(48)).apply { marginStart = dp(8) })
        if (connected != null) addView(icon(Glyph.DISCONNECT, getString(R.string.disconnect), colors.danger) {
            touchpad?.cancelInteractions(false); scrollStick?.cancelInteraction(); hideKeyboard(); model.disconnect()
        }.apply { id = R.id.disconnect }, LinearLayout.LayoutParams(dp(48), dp(48)).apply { marginStart = dp(8) })
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
        scrollStick?.cancelInteraction(); scrollStick = null
        keyboard?.discardComposition(); keyboard = null; keyboardButton = null
        touchpad = null; remotePreviewError = null; shortcuts = null; modeButton = null
        content.removeAllViews()
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        add(content, appBar(state), if (landscape) 8 else 16)
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
        val title = getString(if (landscape) R.string.home_title else R.string.home_title_portrait)
        add(content, text(title, if (landscape) 26f else 34f, medium = true), 10)
        add(content, text(getString(R.string.choose_computer), 15f, colors.muted), 22)
        (state.error?.localized(this) ?: state.discoveryError?.localized(this))?.let {
            add(content, text(it, 14f, colors.danger).apply {
                setPadding(dp(15), dp(12), dp(15), dp(12))
                background = RemoteStyle.shape(this@RemoteActivity, colors.dangerSurface, 16f, colors.dangerBorder)
            }, 14)
        }
        val list = column()
        val known = state.saved
        val nearby = state.nearby.filterNot { found -> known.any { it.host == found.host } }
        if (known.isNotEmpty()) {
            add(list, text(getString(R.string.saved_computers), 11f, colors.muted, true), 10)
            known.forEach { add(list, computerCard(it, getString(R.string.saved_computer)), 10) }
        }
        if (nearby.isNotEmpty()) {
            add(list, text(getString(R.string.nearby), 11f, colors.muted, true), 10)
            nearby.forEach { add(list, computerCard(it, getString(R.string.ready_to_connect)), 10) }
        }
        if (known.isEmpty() && nearby.isEmpty()) {
            val empty = column().apply {
                gravity = Gravity.CENTER; setPadding(dp(20), dp(28), dp(20), dp(28))
                background = RemoteStyle.shape(this@RemoteActivity)
            }
            empty.addView(ImageView(this).apply { setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.WIFI, colors.accent)) }, LinearLayout.LayoutParams(dp(42), dp(42)))
            add(empty, text(getString(R.string.searching_computers), 16f, medium = true).apply { gravity = Gravity.CENTER; setPadding(0, dp(18), 0, 0) }, 8)
            add(empty, text(getString(R.string.discovery_help), 14f, colors.muted).apply { gravity = Gravity.CENTER }, 0)
            add(list, empty, 0)
        }
        add(content, ScrollView(this).apply { isFillViewport = false; overScrollMode = View.OVER_SCROLL_NEVER; addView(list) }, 16, 1f)
        add(content, button(getString(R.string.add_computer), glyph = Glyph.ADD) { address() }.apply { id = R.id.connect }, 0)
    }

    private fun computerCard(computer: Computer, status: String): View = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(16), dp(16), dp(16), dp(16))
        background = RemoteStyle.interactive(this@RemoteActivity)
        minimumHeight = dp(92); isClickable = true; isFocusable = true
        contentDescription = getString(R.string.connect_to, computer.name, computer.host)
        val art = ImageView(this@RemoteActivity).apply {
            setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.COMPUTER, colors.accent))
            setPadding(dp(12), dp(12), dp(12), dp(12))
            background = RemoteStyle.shape(this@RemoteActivity, colors.accentSurface, 15f, null)
        }
        addView(art, LinearLayout.LayoutParams(dp(54), dp(54)).apply { rightMargin = dp(14) })
        val titles = column()
        add(titles, text(computer.name, 17f, medium = true).apply { isSingleLine = true; ellipsize = TextUtils.TruncateAt.END }, 4)
        add(titles, text(status, 12f, colors.muted), 3)
        add(titles, text(computer.host, 11f, colors.muted), 0)
        addView(titles, LinearLayout.LayoutParams(0, -2, 1f))
        addView(ImageView(this@RemoteActivity).apply { setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.CHEVRON, colors.muted)) }, LinearLayout.LayoutParams(dp(22), dp(22)))
        RemoteStyle.press(this)
        setOnClickListener { model.connect(computer) }
    }

    private fun pairing(pairing: Pairing) {
        val card = FrameLayout(this).apply {
            background = RemoteStyle.shape(this@RemoteActivity, colors.surface, 26f)
            addView(column().apply {
                gravity = Gravity.CENTER
                addView(text(getString(R.string.pairing_code), 11f, colors.muted, true).apply { gravity = Gravity.CENTER })
                addView(text(pairing.code, 44f, colors.accent).apply {
                    typeface = Typeface.MONOSPACE; gravity = Gravity.CENTER
                    isSingleLine = true
                    setAutoSizeTextTypeUniformWithConfiguration(20, 44, 1, android.util.TypedValue.COMPLEX_UNIT_SP)
                    setPadding(dp(8), dp(20), dp(8), dp(12))
                })
                addView(text(getString(R.string.confirm_both), 13f, colors.muted).apply { gravity = Gravity.CENTER })
            }, FrameLayout.LayoutParams(-1, -2, Gravity.CENTER))
        }
        val approve = button(getString(R.string.codes_match), true, Glyph.CHECK) { model.approve(true) }.apply { id = R.id.pair }
        val cancel = button(getString(R.string.cancel)) { model.approve(false) }
        if (landscape) {
            val body = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
            body.addView(card, LinearLayout.LayoutParams(0, -1, 1f).apply { rightMargin = dp(18) })
            val actions = column()
            add(actions, text(getString(R.string.check_code_short, pairing.computer.name), 15f, colors.muted), 16)
            add(actions, approve, 10); add(actions, cancel, 0)
            body.addView(actions, LinearLayout.LayoutParams(dp(210), -2))
            add(content, body, 0, 1f)
        } else {
            add(content, text(getString(R.string.confirm_pairing), 30f, medium = true), 12)
            add(content, text(getString(R.string.check_code, pairing.computer.name), 16f, colors.muted), 20)
            add(content, card, 20, 1f)
            add(content, approve, 10); add(content, cancel, 0)
        }
    }

    private fun connecting(computer: Computer) {
        add(content, text(getString(R.string.connecting), 29f, medium = true), 12)
        add(content, text(getString(R.string.confirm_connection, computer.name), 16f, colors.muted), 20)
        add(content, FrameLayout(this).apply {
            addView(ProgressBar(this@RemoteActivity).apply { indeterminateTintList = android.content.res.ColorStateList.valueOf(colors.accent) },
                FrameLayout.LayoutParams(dp(42), dp(42), Gravity.CENTER))
        }, 20, 1f)
        add(content, button(getString(R.string.cancel)) { model.disconnect() }, 0)
    }

    private fun alternateMode(mode: PointerMode): PointerMode =
        if (mode == PointerMode.TOUCHPAD) PointerMode.JOYSTICK else PointerMode.TOUCHPAD

    private fun modeName(mode: PointerMode): String =
        getString(if (mode == PointerMode.TOUCHPAD) R.string.touchpad else R.string.joystick)

    private fun updateModeButton(mode: PointerMode) {
        val toggle = modeButton ?: return
        if (toggle.tag == mode) return
        val glyph = if (mode == PointerMode.TOUCHPAD) Glyph.TOUCHPAD else Glyph.JOYSTICK
        val label = getString(R.string.pointer_mode_toggle, modeName(mode), modeName(alternateMode(mode)))
        toggle.setImageDrawable(GlyphDrawable(this, glyph, colors.accent))
        toggle.contentDescription = label
        toggle.tooltipText = label
        toggle.tag = mode
    }

    private fun mouseButton(label: String, glyph: Glyph, code: Int): View {
        val compact = landscape || resources.configuration.screenWidthDp <= 360 || resources.configuration.fontScale > 1.25f
        if (compact) return icon(glyph, label, colors.ink) { clickRemote(code) }
        return button(label, glyph = glyph) { clickRemote(code) }.apply {
            setPaddingRelative(dp(8), paddingTop, dp(8), paddingBottom)
            compoundDrawablePadding = dp(5)
            isSingleLine = false
            maxLines = 2
        }
    }

    private fun remote(@Suppress("UNUSED_PARAMETER") computer: Computer, settings: PointerSettings) {
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val root = LinearLayout(this).apply { orientation = if (landscape) LinearLayout.HORIZONTAL else LinearLayout.VERTICAL }
        val surface = column()
        add(surface, text(last?.previewError.orEmpty(), 13f, colors.danger).apply {
            maxLines = 3
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
            visibility = if (last?.previewError == null || typing) View.GONE else View.VISIBLE
            remotePreviewError = this
        }, 8)
        val pad = TouchpadView(this, model::input).apply {
            id = R.id.touchpad
            preferences(settings)
            setHintForeground(previewVisible)
            touchpad = this
        }
        add(surface, pad, if (landscape) 0 else 12, 1f)
        val controls = column()
        val left = mouseButton(getString(R.string.left_click), Glyph.LEFT_CLICK, 272)
        val right = mouseButton(getString(R.string.right_click), Glyph.RIGHT_CLICK, 273)
        val stick = ScrollStickView(this) { input ->
            touchpad?.noteRemoteActivity()
            model.input(input)
        }.apply { preferences(settings); scrollStick = this }
        val clicks = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            addView(left, LinearLayout.LayoutParams(0, dp(52), 1f))
            addView(stick, LinearLayout.LayoutParams(dp(72), dp(52)).apply {
                marginStart = dp(8); marginEnd = dp(8)
            })
            addView(right, LinearLayout.LayoutParams(0, dp(52), 1f))
        }
        add(controls, clicks, 10)
        val shortcutButtons = arrayOf(
            button(getString(R.string.key_back)) { touchpad?.noteRemoteActivity(); model.chord(56, 105) },
            button(getString(R.string.key_space)) { tapRemote(57) },
            button(getString(R.string.key_enter)) { tapRemote(28) },
        )
        if (landscape) shortcutButtons.forEach { key ->
            key.textSize = 13f
            key.setPaddingRelative(dp(4), key.paddingTop, dp(4), key.paddingBottom)
        }
        add(controls, row(*shortcutButtons).also { shortcuts = it }, 10)
        add(controls, button(getString(R.string.keyboard), true, Glyph.KEYBOARD) {
            touchpad?.noteRemoteActivity()
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
        scrollStick?.preferences(settings)
        updateModeButton(settings.mode)
    }
    private fun currentControls() = if (captureCase == null) model.state.value.controls else last?.controls ?: PointerSettings()
    private fun updatePreviewMessage(state: ScreenState) {
        remotePreviewError?.apply {
            text = state.previewError.orEmpty()
            visibility = if (state.previewError == null || typing) View.GONE else View.VISIBLE
        }
        previewMessage?.apply {
            text = state.previewError ?: getString(R.string.preview_help)
            setTextColor(if (state.previewError == null) colors.muted else colors.danger)
        }
    }
    private fun setControls(settings: PointerSettings) {
        if (captureCase == null) model.controls(settings)
        else { last = last?.copy(controls = settings); updateControls(settings) }
    }
    private fun applyTypingLayout() {
        shortcuts?.visibility = if (typing) View.GONE else View.VISIBLE
        remotePreviewError?.visibility = if (typing || last?.previewError == null) View.GONE else View.VISIBLE
        keyboardButton?.apply {
            text = if (typing) getString(R.string.hide_keyboard) else getString(R.string.keyboard)
            contentDescription = text
        }
        touchpad?.minimumHeight = dp(if (typing) 70 else 120)
    }
    private fun hideKeyboard(discard: Boolean = true) {
        keyboard?.let { (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).hideSoftInputFromWindow(it.windowToken, 0) }
        if (discard) keyboard?.discardComposition()
    }
    private fun clickRemote(code: Int) {
        touchpad?.noteRemoteActivity()
        model.click(code)
    }
    private fun tapRemote(code: Int) {
        touchpad?.noteRemoteActivity()
        model.tap(code)
    }
    private fun showKeyboard() {
        touchpad?.cancelInteractions()
        scrollStick?.cancelInteraction()
        if (keyboard == null) {
            keyboard = RemoteKeyboard(this, { value ->
                touchpad?.noteRemoteActivity()
                runCatching { model.text(value) }.onFailure {
                    Toast.makeText(this, getString(R.string.text_too_long), Toast.LENGTH_SHORT).show()
                }
            }, ::tapRemote).also { content.addView(it, ViewGroup.LayoutParams(1, 1)) }
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
        AlertDialog.Builder(this).setTitle(getString(R.string.add_computer)).setMessage(getString(R.string.address_prompt)).setView(input)
            .setNegativeButton(getString(R.string.cancel), null).setPositiveButton(getString(R.string.connect)) { _, _ ->
                val host = input.text.toString().trim()
                if (host.isNotEmpty() && host.length <= 253 && host.none(Char::isWhitespace) && !host.contains('/')) model.connect(Computer(getString(R.string.computer), host))
                else Toast.makeText(this, getString(R.string.address_prompt), Toast.LENGTH_SHORT).show()
            }.show()
    }

    private fun themeName(theme: AppTheme): String = getString(when (theme) {
        AppTheme.SYSTEM -> R.string.follow_system
        AppTheme.LIGHT -> R.string.theme_light
        AppTheme.DARK -> R.string.theme_dark
    })

    private fun languageName(language: AppLanguage): String =
        if (language == AppLanguage.SYSTEM) getString(R.string.follow_system) else language.displayName

    private fun appearanceRow(label: String, value: String, open: () -> Unit): View = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(14), dp(8), dp(12), dp(8))
        minimumHeight = dp(52)
        background = RemoteStyle.interactive(this@RemoteActivity)
        addView(text(label, 15f, medium = true), LinearLayout.LayoutParams(0, -2, 1f))
        addView(text(value, 14f, colors.muted).apply { maxWidth = dp(152) })
        addView(ImageView(this@RemoteActivity).apply {
            setImageDrawable(GlyphDrawable(this@RemoteActivity, Glyph.CHEVRON, colors.muted))
        }, LinearLayout.LayoutParams(dp(20), dp(20)).apply { marginStart = dp(8) })
        contentDescription = "$label · $value"
        isFocusable = true
        setOnClickListener { open() }
    }

    private fun <T> appearanceChoice(title: Int, choices: List<T>, current: T, label: (T) -> String, choose: (T) -> Unit) {
        if (appearancePicker?.isShowing == true) return
        val dialog = AlertDialog.Builder(this).setTitle(title)
            .setSingleChoiceItems(choices.map(label).toTypedArray(), choices.indexOf(current)) { picker, index ->
                picker.dismiss()
                choose(choices[index])
            }.setNegativeButton(R.string.cancel, null).create()
        appearancePicker = dialog
        dialog.setOnDismissListener { if (appearancePicker === dialog) appearancePicker = null }
        dialog.show()
    }

    private fun changeAppearance(appearance: AppearanceSettings) {
        if (model.appearance.value == appearance) return
        model.setAppearance(appearance)
        restoreControls = true
        settingsDialog?.dismiss()
        recreate()
    }

    private fun settings() {
        if (settingsDialog?.isShowing == true) return
        if (captureCase == null) model.foreground(false)
        touchpad?.cancelInteractions(false); scrollStick?.cancelInteraction(); hideKeyboard()
        val settings = currentControls()
        val sheet = column().apply {
            setPadding(dp(22), dp(20), dp(22), dp(22))
            background = RemoteStyle.shape(this@RemoteActivity, colors.surface, 28f)
        }
        val heading = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        heading.addView(text(getString(R.string.controls), 23f, medium = true), LinearLayout.LayoutParams(0, -2, 1f))
        heading.addView(icon(Glyph.CLOSE, getString(R.string.close_settings)) { settingsDialog?.dismiss() }, LinearLayout.LayoutParams(dp(48), dp(48)).apply { leftMargin = dp(8) })
        add(sheet, heading, 14)
        val list = column()
        addUpdatesPanel(list)
        add(list, text(getString(R.string.scrolling), 11f, colors.muted, true), 6)
        add(list, controlSwitch(getString(R.string.double_tap_scroll), settings.doubleTapScroll) { enabled ->
            setControls(currentControls().copy(doubleTapScroll = enabled))
        }, 2)
        add(list, text(getString(R.string.double_tap_help), 13f, colors.muted), 16)
        slider(list, getString(R.string.scroll_gesture_speed), settings.scrollGestureSpeed, 0.5f, 3f, { getString(R.string.speed_multiplier, it) }) {
            setControls(currentControls().copy(scrollGestureSpeed = it))
        }
        slider(list, getString(R.string.scroll_joystick_speed), settings.scrollStickSpeed, 0.5f, 3f, { getString(R.string.speed_multiplier, it) }) {
            setControls(currentControls().copy(scrollStickSpeed = it))
        }
        add(list, controlSwitch(getString(R.string.reverse_scroll), settings.invertScroll) { enabled ->
            setControls(currentControls().copy(invertScroll = enabled))
        }, 22)
        add(list, text(getString(R.string.pointer_speed), 11f, colors.muted, true), 10)
        slider(list, getString(R.string.touchpad_speed), settings.touchpadSpeed, 0.35f, 3f, { getString(R.string.speed_multiplier, it) }) { value ->
            setControls(currentControls().copy(touchpadSpeed = value))
        }
        slider(list, getString(R.string.joystick_speed), settings.joystickSpeed, 300f, 2400f, { getString(R.string.pixels_per_second, it.toInt()) }) { value ->
            setControls(currentControls().copy(joystickSpeed = value))
        }
        add(list, text(getString(R.string.joystick_speed_help), 13f, colors.muted), 24)
        val fpsControls = column()
        slider(fpsControls, getString(R.string.preview_frame_rate), settings.previewFps.toFloat(), 1f, 30f,
            { getString(R.string.frames_per_second, kotlin.math.round(it).toInt()) }) { value ->
            setControls(currentControls().copy(previewFps = kotlin.math.round(value).toInt()))
        }
        fpsControls.visibility = if (settings.previewEnabled) View.VISIBLE else View.GONE
        val previewToggle = controlSwitch(getString(R.string.cursor_preview), settings.previewEnabled) { enabled ->
            setControls(currentControls().copy(previewEnabled = enabled))
            fpsControls.visibility = if (enabled) View.VISIBLE else View.GONE
            if (enabled && captureCase == null) fpsControls.apply {
                alpha = 0f
                animate().alpha(1f).setDuration(180).start()
            }
        }.apply { contentDescription = getString(R.string.cursor_preview_action) }
        add(list, previewToggle, 4)
        add(list, text(getString(R.string.preview_help), 13f, colors.muted)
            .also { previewMessage = it }, 14)
        add(list, fpsControls, 20)
        updatePreviewMessage(last ?: ScreenState())
        val appearance = model.appearance.value
        add(list, text(getString(R.string.appearance), 11f, colors.muted, true), 10)
        add(list, appearanceRow(getString(R.string.theme), themeName(appearance.theme)) {
            appearanceChoice(R.string.theme, AppTheme.entries, appearance.theme, ::themeName) {
                changeAppearance(appearance.copy(theme = it))
            }
        }, 8)
        add(list, appearanceRow(getString(R.string.language), languageName(appearance.language)) {
            appearanceChoice(R.string.language, AppLanguage.entries, appearance.language, ::languageName) {
                changeAppearance(appearance.copy(language = it))
            }
        }, 20)
        val saved = if (captureCase != null) listOf(previewComputer) else model.state.value.saved
        if (saved.isNotEmpty()) {
            add(list, text(getString(R.string.saved_computers), 11f, colors.muted, true), 12)
            saved.forEach { computer ->
                val line = row(text(computer.name, 15f).apply { maxLines = 2 }, button(getString(R.string.forget)) {
                    model.forget(computer)
                    settingsDialog?.dismiss()
                    Toast.makeText(this, getString(R.string.computer_forgotten), Toast.LENGTH_SHORT).show()
                })
                add(list, line, 12)
            }
        }

        add(list, text(getString(R.string.background_help), 12f, colors.muted), 14)
        add(list, button(getString(R.string.licenses)) {
            val licenses = resources.openRawResource(R.raw.native_licenses).bufferedReader().use { it.readText() }
            AlertDialog.Builder(this).setTitle(getString(R.string.licenses)).setMessage(licenses).setPositiveButton(getString(R.string.done), null).show()
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
                renderUpdates = null
                if (captureCase == null) model.foreground(previewVisible && !restoreControls)
            }
            setOnShowListener {
                window?.setLayout(-1, maxHeight)
                if (captureCase == "settings" || captureCase == "updates") capture(window!!.decorView)
                else {
                    sheet.translationY = dp(24).toFloat(); sheet.alpha = 0f
                    sheet.animate().translationY(0f).alpha(1f).setDuration(220).setInterpolator(DecelerateInterpolator()).start()
                }
            }
        }
        settingsDialog = dialog
        dialog.show()
    }

    private fun addUpdatesPanel(list: LinearLayout) {
        val details = column().apply { visibility = if (updatesExpanded) View.VISIBLE else View.GONE }
        val heading = button(getString(R.string.updates)) {
            updatesExpanded = !updatesExpanded
            details.visibility = if (updatesExpanded) View.VISIBLE else View.GONE
        }.apply {
            gravity = Gravity.CENTER_VERTICAL or Gravity.START
        }
        add(list, heading, 6)
        add(list, details, 18)
        add(details, text(getString(R.string.app_version, BuildConfig.VERSION_NAME), 14f, medium = true), 2)
        add(details, text(getString(R.string.update_protocol_versions, NativeWire.PROTOCOL, NativeWire.WORKSPACE_PROTOCOL),
            13f, colors.muted), 6)
        val snapshot = if (captureCase == null) updates.state.value else UpdateScreenState()
        val startup = controlSwitch(getString(R.string.update_startup), snapshot.startupCheck) {
            if (captureCase == null) updates.setStartupCheck(it)
        }
        add(details, startup, 6)

        val status = text("", 14f, colors.muted)
        val progress = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progressTintList = android.content.res.ColorStateList.valueOf(colors.accent)
        }
        val check = updateButton(R.string.update_check) { updates.check() }
        val download = updateButton(R.string.update_download, primary = true) { updates.downloadAndInstall() }
        val cancel = updateButton(android.R.string.cancel) { updates.cancelDownload() }
        val permission = updateButton(R.string.update_open_settings) { updates.requestPermission() }
        val install = updateButton(R.string.update_install, primary = true) { updates.install() }
        add(details, status, 8)
        add(details, progress, 8)
        add(details, download, 4)
        add(details, check, 4)
        add(details, cancel, 4)
        add(details, permission, 4)
        add(details, install, 4)

        renderUpdates = { state ->
            val update = state.status
            startup.isChecked = state.startupCheck
            status.text = updateMessage(update)
            status.visibility = if (status.text.isEmpty()) View.GONE else View.VISIBLE
            progress.visibility = if (update is UpdateStatus.Downloading) View.VISIBLE else View.GONE
            if (update is UpdateStatus.Downloading) progress.progress = update.progress
            check.visibility = if (update == UpdateStatus.Idle || update == UpdateStatus.Current ||
                update == UpdateStatus.Checking || update is UpdateStatus.Failed || update is UpdateStatus.Available)
                View.VISIBLE else View.GONE
            check.isEnabled = update != UpdateStatus.Checking
            download.visibility = if (update is UpdateStatus.Available || update is UpdateStatus.Failed && update.canRetryDownload)
                View.VISIBLE else View.GONE
            cancel.visibility = if (update is UpdateStatus.Downloading) View.VISIBLE else View.GONE
            permission.visibility = if (update == UpdateStatus.PermissionRequired || update is UpdateStatus.HandoffFailed && update.permission)
                View.VISIBLE else View.GONE
            install.visibility = if (update == UpdateStatus.Ready || update == UpdateStatus.InstallFailed ||
                update is UpdateStatus.HandoffFailed && !update.permission)
                View.VISIBLE else View.GONE
        }
        renderUpdates?.invoke(snapshot)
    }

    private fun updateButton(title: Int, primary: Boolean = false, action: () -> Unit) =
        button(getString(title), primary) {
            if (captureCase == null) action()
        }.apply {
            isSingleLine = false
            ellipsize = null
        }

    private fun updateMessage(status: UpdateStatus): String = when (status) {
        UpdateStatus.Idle -> ""
        UpdateStatus.Checking -> getString(R.string.update_checking)
        UpdateStatus.Current -> getString(R.string.update_current)
        is UpdateStatus.Available -> getString(R.string.update_available, status.update.version)
        is UpdateStatus.Downloading -> getString(R.string.update_progress, status.progress)
        UpdateStatus.Verifying -> getString(R.string.update_verifying)
        UpdateStatus.Ready -> getString(R.string.update_ready)
        UpdateStatus.PermissionRequired, UpdateStatus.AwaitingPermission -> getString(R.string.update_permission)
        UpdateStatus.AwaitingInstall -> getString(R.string.update_confirm_install)
        UpdateStatus.InstallFinished -> getString(R.string.update_installed)
        UpdateStatus.InstallFailed -> getString(R.string.update_error_install_failed)
        is UpdateStatus.HandoffFailed -> getString(R.string.update_error_installer)
        is UpdateStatus.Failed -> getString(when (status.failure) {
            UpdateFailure.NETWORK -> R.string.update_error_network
            UpdateFailure.RATE_LIMIT -> R.string.update_error_rate_limit
            UpdateFailure.RELEASE_DATA -> R.string.update_error_release
            UpdateFailure.CORRUPT_FILE -> R.string.update_error_corrupt
            UpdateFailure.INCOMPATIBLE_APK -> R.string.update_error_incompatible
            UpdateFailure.SIGNER_MISMATCH -> R.string.update_error_signer
            UpdateFailure.STORAGE -> R.string.update_error_storage
        })
    }

    private fun controlSwitch(title: String, enabled: Boolean, changed: (Boolean) -> Unit) = Switch(this).apply {
        text = title; textSize = 16f; setTextColor(colors.ink)
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        minimumHeight = dp(52); switchPadding = dp(18)
        isChecked = enabled
        thumbTintList = android.content.res.ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()),
            intArrayOf(colors.accent, colors.muted))
        trackTintList = android.content.res.ColorStateList.valueOf(colors.switchTrack)
        setOnCheckedChangeListener { _, checked -> changed(checked) }
    }

    private fun slider(parent: LinearLayout, title: String, value: Float, minimum: Float, maximum: Float,
        format: (Float) -> String, changed: (Float) -> Unit) {
        val labels = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        labels.addView(text(title, 16f, medium = true), LinearLayout.LayoutParams(0, -2, 1f))
        val amount = text(format(value), 14f, colors.accent)
        labels.addView(amount, LinearLayout.LayoutParams(-2, -2).apply { leftMargin = dp(12) })
        add(parent, labels, 4)
        add(parent, SeekBar(this).apply {
            max = 100; progress = ((value - minimum) / (maximum - minimum) * 100).toInt()
            contentDescription = title
            minimumHeight = dp(48)
            progressTintList = android.content.res.ColorStateList.valueOf(colors.accent)
            thumbTintList = android.content.res.ColorStateList.valueOf(colors.accent)
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

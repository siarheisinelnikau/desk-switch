package io.github.deskswitch.remote.ui

import android.content.Context
import androidx.annotation.StringRes
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.deskswitch.remote.R
import io.github.deskswitch.remote.data.ProtocolKind
import io.github.deskswitch.remote.data.ProtocolVersionMismatch
import io.github.deskswitch.remote.domain.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

internal sealed interface ScreenIssue {
    data class Detail(val message: String) : ScreenIssue
    data class Local(@StringRes val resource: Int, val errorCode: Int? = null) : ScreenIssue
    data class Protocol(val mismatch: ProtocolVersionMismatch) : ScreenIssue

    fun localized(context: Context): String = when (this) {
        is Detail -> message
        is Local -> if (errorCode == null) context.getString(resource) else context.getString(resource, errorCode)
        is Protocol -> context.getString(when (mismatch.kind) {
            ProtocolKind.DEVICE -> R.string.device_protocol_mismatch
            ProtocolKind.WORKSPACE -> R.string.workspace_protocol_mismatch
        }, mismatch.expected, mismatch.received)
    }
}

internal data class ScreenState(
    val saved: List<Computer> = emptyList(), val nearby: List<Computer> = emptyList(),
    val connecting: Computer? = null, val connected: Computer? = null,
    val pairing: Pairing? = null, val error: ScreenIssue? = null, val discoveryError: ScreenIssue? = null,
    val controls: PointerSettings = PointerSettings(),
    val previewError: String? = null,
)
class RemoteViewModel internal constructor(
    private val store: RemoteStore,
    private val makeFactory: ((Computer) -> Unit) -> SessionFactory,
    private val controlsStore: PointerSettingsStore,
    private val appearanceStore: AppearanceSettingsStore,
) : ViewModel() {
    private val mutable = MutableStateFlow(ScreenState(saved = store.computers(), controls = controlsStore.load().normalized()))
    internal val state = mutable.asStateFlow()
    private val mutableAppearance = MutableStateFlow(appearanceStore.load())
    internal val appearance = mutableAppearance.asStateFlow()
    private val mutableFrames = MutableStateFlow<ByteArray?>(null)
    val previewFrames = mutableFrames.asStateFlow()
    private val mutableInputErrors = MutableSharedFlow<String>(extraBufferCapacity = 1)
    val inputErrors = mutableInputErrors.asSharedFlow()
    @Volatile private var foreground = false
    private var previewRequest: ScreenOptions? = null
    private val generation = AtomicLong()
    private var factory: SessionFactory? = null
    private var session: RemoteSession? = null
    private val consent = AtomicReference<CompletableFuture<Boolean>?>()
    fun discovered(computers: List<Computer>) = viewModelScope.launch {
        val saved = store.computers().map { known ->
            computers.find { it.advertisedKey == known.key }?.copy(key = known.key) ?: known
        }
        val connected = mutable.value.connected?.let { current ->
            computers.find { it.advertisedKey == current.key }?.let { found ->
                current.copy(previewSupported = found.previewSupported)
            } ?: current
        }
        mutable.value = mutable.value.copy(nearby = computers.sortedBy { it.name }, saved = saved, connected = connected)
    }
    internal fun discoveryFailed(issue: ScreenIssue?) = viewModelScope.launch {
        mutable.value = mutable.value.copy(discoveryError = issue)
    }
    internal fun setAppearance(settings: AppearanceSettings) {
        if (settings == mutableAppearance.value) return
        appearanceStore.save(settings)
        mutableAppearance.value = settings
    }
    fun controls(settings: PointerSettings) {
        val value = settings.normalized()
        controlsStore.save(value)
        mutable.value = mutable.value.copy(controls = value, previewError = if (value.previewEnabled) mutable.value.previewError else null)
        if (!value.previewEnabled) mutableFrames.value = null
        updatePreview()
    }
    fun foreground(active: Boolean) {
        foreground = active
        if (!active) mutableFrames.value = null
        updatePreview()
    }
    private fun updatePreview() {
        val active = session ?: return
        val settings = mutable.value.controls
        val options = ScreenOptions(enabled = foreground && settings.previewEnabled, fps = settings.previewFps)
        // Off is quiet until an already-started stream needs to be stopped.
        if (options != previewRequest && (options.enabled || previewRequest?.enabled == true)) {
            runCatching { active.preview(options) }.onFailure { mutableFrames.value = null }
        }
        previewRequest = if (options.enabled) options else null
    }
    fun connect(computer: Computer) {
        disconnect()
        val run = generation.incrementAndGet()
        val adapter = makeFactory { saved -> if (generation.get() == run) store.save(saved) }
        var lastInputErrorAt: Long? = null
        adapter.setInputErrorListener { error ->
            viewModelScope.launch {
                val now = System.nanoTime()
                val last = lastInputErrorAt
                if (generation.get() == run && foreground && session != null &&
                    (last == null || now - last >= TimeUnit.MILLISECONDS.toNanos(2500))) {
                    lastInputErrorAt = now
                    mutableInputErrors.tryEmit(error.take(160))
                }
            }
        }
        adapter.setPreviewListener { frame ->
            if (generation.get() == run && foreground && mutable.value.controls.previewEnabled) mutableFrames.value = frame
        }
        adapter.setPreviewErrorListener { error ->
            viewModelScope.launch {
                if (generation.get() == run && foreground && mutable.value.controls.previewEnabled) {
                    mutable.value = mutable.value.copy(previewError = error)
                }
            }
        }
        factory = adapter
        mutable.value = mutable.value.copy(connecting = computer, error = null)
        viewModelScope.launch(Dispatchers.IO) {
            try {
                val known = store.computers().find { it.key == computer.advertisedKey }
                    ?: store.computers().find { it.host == computer.host && it.port == computer.port }
                val pinned = computer.copy(key = computer.key ?: known?.key)
                val connected = adapter.connect(pinned, store.identity(), { pairing ->
                    val answer = CompletableFuture<Boolean>()
                    consent.set(answer)
                    viewModelScope.launch { if (generation.get() == run) mutable.value = mutable.value.copy(pairing = pairing) else answer.complete(false) }
                    try { answer.get(90, TimeUnit.SECONDS) } finally { consent.compareAndSet(answer, null) }
                }, { error ->
                    if (generation.compareAndSet(run, run + 1)) {
                        mutableFrames.value = null
                        viewModelScope.launch {
                            session = null; previewRequest = null
                            mutable.value = mutable.value.copy(connected = null, connecting = null, pairing = null,
                                error = error?.let { ScreenIssue.Detail(it) }, previewError = null)
                        }
                    }
                })
                viewModelScope.launch {
                    if (generation.get() == run) {
                        session = connected
                        previewRequest = null
                        val saved = store.computers().find { it.host == computer.host }
                        val current = (saved ?: computer).copy(previewSupported = computer.previewSupported || saved?.previewSupported == true)
                        mutable.value = mutable.value.copy(connecting = null, pairing = null, connected = current, saved = store.computers())
                        updatePreview()
                    } else connected.close()
                }
            } catch (error: Exception) {
                viewModelScope.launch {
                    if (generation.get() != run) return@launch
                    val issue = if (error is ProtocolVersionMismatch) ScreenIssue.Protocol(error) else {
                        error.message?.take(240)?.takeIf(String::isNotBlank)?.let { ScreenIssue.Detail(it) }
                            ?: ScreenIssue.Local(R.string.connection_unavailable)
                    }
                    mutable.value = mutable.value.copy(connecting = null, pairing = null, error = issue)
                }
            }
        }
    }
    fun approve(accept: Boolean) {
        mutable.value = mutable.value.copy(pairing = null)
        consent.get()?.complete(accept)
        if (!accept) disconnect()
    }
    fun input(event: Input) { session?.input(event) }
    fun text(text: String) { if (text.isNotEmpty()) session?.text(text) }
    fun tap(code: Int) { input(Input.Key(code, true)); input(Input.Key(code, false)) }
    fun chord(vararg codes: Int) { codes.forEach { input(Input.Key(it, true)) }; codes.reversed().forEach { input(Input.Key(it, false)) } }
    fun click(code: Int) { input(Input.Button(code, true)); input(Input.Button(code, false)) }
    fun forget(computer: Computer) {
        if (computer.key != null && mutable.value.connected?.key == computer.key) disconnect()
        store.forget(computer.key)
        mutable.value = mutable.value.copy(saved = store.computers())
    }
    fun disconnect() {
        generation.incrementAndGet(); consent.getAndSet(null)?.complete(false)
        mutableFrames.value = null; previewRequest = null
        session?.close(); session = null; factory?.close(); factory = null
        mutable.value = mutable.value.copy(connected = null, connecting = null, pairing = null, previewError = null)
    }
    override fun onCleared() { disconnect() }
}

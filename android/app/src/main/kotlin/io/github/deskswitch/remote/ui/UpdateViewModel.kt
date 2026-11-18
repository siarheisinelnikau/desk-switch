package io.github.deskswitch.remote.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.deskswitch.remote.data.ApkInstaller
import io.github.deskswitch.remote.data.UpdatePreferences
import io.github.deskswitch.remote.update.AndroidUpdate
import io.github.deskswitch.remote.update.UpdateException
import io.github.deskswitch.remote.update.UpdateFailure
import io.github.deskswitch.remote.update.UpdateSource
import java.io.File
import java.io.IOException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal sealed interface UpdateStatus {
    data object Idle : UpdateStatus
    data object Checking : UpdateStatus
    data object Current : UpdateStatus
    data class Available(val update: AndroidUpdate) : UpdateStatus
    data class Downloading(val progress: Int) : UpdateStatus
    data object Verifying : UpdateStatus
    data object Ready : UpdateStatus
    data object PermissionRequired : UpdateStatus
    data object AwaitingPermission : UpdateStatus
    data object AwaitingInstall : UpdateStatus
    data object InstallFinished : UpdateStatus
    data object InstallFailed : UpdateStatus
    data class HandoffFailed(val permission: Boolean) : UpdateStatus
    data class Failed(val failure: UpdateFailure, val canRetryDownload: Boolean = false) : UpdateStatus
}

internal data class UpdateScreenState(val startupCheck: Boolean = true, val status: UpdateStatus = UpdateStatus.Idle)

internal sealed interface UpdateEffect {
    data class StartupNotice(val version: String) : UpdateEffect
    data object RequestPermission : UpdateEffect
    data class OpenInstaller(val file: File) : UpdateEffect
}

internal class UpdateViewModel(
    private val installedVersion: String,
    private val source: UpdateSource,
    private val installer: ApkInstaller,
    private val preferences: UpdatePreferences,
) : ViewModel() {
    private val mutableState = MutableStateFlow(UpdateScreenState(startupCheck = preferences.startupCheck))
    val state = mutableState.asStateFlow()
    private val events = Channel<UpdateEffect>(Channel.BUFFERED)
    val effects = events.receiveAsFlow()
    private var operation: Job? = null
    private var startupAttempted = false
    private var available: AndroidUpdate? = null
    private var verifiedFile: File? = null

    val handingOff get() = state.value.status == UpdateStatus.AwaitingPermission ||
        state.value.status == UpdateStatus.AwaitingInstall

    fun startup() {
        if (startupAttempted) return
        startupAttempted = true
        if (state.value.startupCheck) check(automatic = true)
    }

    fun setStartupCheck(enabled: Boolean) {
        preferences.startupCheck = enabled
        mutableState.update { it.copy(startupCheck = enabled) }
    }

    fun check(automatic: Boolean = false) {
        if (operation?.isCompleted == false || handingOff) return
        setStatus(UpdateStatus.Checking)
        operation = viewModelScope.launch {
            try {
                available = withContext(Dispatchers.IO) { source.check(installedVersion) }
                val update = available
                verifiedFile?.delete()
                verifiedFile = null
                setStatus(if (update != null) UpdateStatus.Available(update)
                    else if (automatic) UpdateStatus.Idle else UpdateStatus.Current)
                if (automatic && update != null) events.trySend(UpdateEffect.StartupNotice(update.version))
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: UpdateException) {
                setStatus(if (automatic) UpdateStatus.Idle else UpdateStatus.Failed(failure.failure))
            }
        }
    }

    fun downloadAndInstall() {
        val update = available ?: return
        if (operation?.isCompleted == false || handingOff) return
        verifiedFile?.takeIf { it.isFile }?.let {
            install()
            return
        }
        setStatus(UpdateStatus.Downloading(0))
        operation = viewModelScope.launch {
            var temporary: File? = null
            try {
                val file = withContext(Dispatchers.IO) {
                    val destination = createDownloadFile()
                    temporary = destination
                    val caller = currentCoroutineContext()
                    source.download(update, destination, { percent ->
                        mutableState.update { state ->
                            if (caller[Job]?.isActive == true) {
                                state.copy(status = UpdateStatus.Downloading(percent.coerceIn(0, 100)))
                            } else state
                        }
                    }, { caller.ensureActive() })
                    caller.ensureActive()
                    mutableState.update { state ->
                        if (caller[Job]?.isActive == true) state.copy(status = UpdateStatus.Verifying) else state
                    }
                    caller.ensureActive()
                    installer.verify(update, destination)
                    destination
                }
                verifiedFile = file
                install()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: UpdateException) {
                val canRetry = failure.failure in setOf(
                    UpdateFailure.NETWORK, UpdateFailure.RATE_LIMIT, UpdateFailure.CORRUPT_FILE, UpdateFailure.STORAGE,
                )
                setStatus(UpdateStatus.Failed(failure.failure, canRetryDownload = canRetry))
            } finally {
                if (temporary != verifiedFile) temporary?.delete()
            }
        }
    }

    private fun createDownloadFile(): File {
        try {
            val directory = installer.directory
            directory.mkdirs()
            return File.createTempFile("desk-switch-", ".apk", directory)
        } catch (failure: IOException) {
            throw UpdateException(UpdateFailure.STORAGE, failure)
        }
    }

    fun cancelDownload() {
        if (state.value.status !is UpdateStatus.Downloading) return
        operation?.cancel()
        setStatus(available?.let { UpdateStatus.Available(it) } ?: UpdateStatus.Idle)
    }

    fun requestPermission() {
        val status = state.value.status
        if (status != UpdateStatus.PermissionRequired && !(status is UpdateStatus.HandoffFailed && status.permission)) return
        setStatus(UpdateStatus.AwaitingPermission)
        events.trySend(UpdateEffect.RequestPermission)
    }

    fun install() {
        val file = verifiedFile?.takeIf { it.isFile }
        when {
            file == null -> setStatus(UpdateStatus.Failed(UpdateFailure.STORAGE))
            !installer.allowed() -> setStatus(UpdateStatus.PermissionRequired)
            else -> {
                setStatus(UpdateStatus.AwaitingInstall)
                events.trySend(UpdateEffect.OpenInstaller(file))
            }
        }
    }

    fun systemReturned(installed: Boolean, cancelled: Boolean) {
        when (state.value.status) {
            UpdateStatus.AwaitingPermission -> setStatus(
                if (installer.allowed()) UpdateStatus.Ready else UpdateStatus.PermissionRequired
            )
            UpdateStatus.AwaitingInstall -> setStatus(when {
                installed -> UpdateStatus.InstallFinished
                cancelled -> UpdateStatus.Ready
                else -> UpdateStatus.InstallFailed
            })
            else -> Unit
        }
    }

    fun handoffFailed() {
        setStatus(UpdateStatus.HandoffFailed(permission = state.value.status == UpdateStatus.AwaitingPermission))
    }

    private fun setStatus(status: UpdateStatus) {
        mutableState.update { it.copy(status = status) }
    }

    override fun onCleared() {
        if (state.value.status != UpdateStatus.AwaitingInstall) verifiedFile?.delete()
        events.close()
    }
}

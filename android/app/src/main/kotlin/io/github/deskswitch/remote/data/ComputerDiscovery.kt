package io.github.deskswitch.remote.data

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import io.github.deskswitch.remote.domain.Computer
import java.util.concurrent.atomic.AtomicBoolean

@Suppress("DEPRECATION")
class ComputerDiscovery(
    context: Context,
    private val changed: (List<Computer>) -> Unit,
    private val failed: (String?) -> Unit,
) : AutoCloseable {
    private val manager = context.getSystemService(NsdManager::class.java)
    private val closed = AtomicBoolean(false)
    private val found = mutableMapOf<String, Computer>()
    private val resolving = java.util.ArrayDeque<NsdServiceInfo>()
    private var busy = false
    private val listener = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(type: String) { if (!closed.get()) failed(null) }
        override fun onDiscoveryStopped(type: String) = Unit
        override fun onStartDiscoveryFailed(type: String, code: Int) {
            if (!closed.get()) failed("Wi-Fi discovery could not start ($code). Reopen the app to retry, or add the computer by address.")
        }
        override fun onStopDiscoveryFailed(type: String, code: Int) = Unit
        override fun onServiceFound(service: NsdServiceInfo) = synchronized(this@ComputerDiscovery) {
            if (!closed.get() && resolving.size < 16) { resolving.add(service); resolveNext() }
        }
        override fun onServiceLost(service: NsdServiceInfo) = synchronized(this@ComputerDiscovery) {
            found.remove(service.serviceName)
            if (!closed.get()) changed(found.values.toList())
            Unit
        }
    }
    init { manager.discoverServices(ComputerAdvertisement.SERVICE, NsdManager.PROTOCOL_DNS_SD, listener) }
    @Synchronized private fun resolveNext() {
        if (closed.get() || busy || resolving.isEmpty()) return
        busy = true
        val next = resolving.removeFirst()
        manager.resolveService(next, object : NsdManager.ResolveListener {
            override fun onResolveFailed(info: NsdServiceInfo, error: Int) = synchronized(this@ComputerDiscovery) { busy = false; resolveNext() }
            override fun onServiceResolved(info: NsdServiceInfo) = synchronized(this@ComputerDiscovery) {
                if (!closed.get()) runCatching {
                    val host = info.host.hostAddress ?: return@runCatching
                    val attributes = info.attributes.mapValues { it.value.toString(Charsets.UTF_8) }
                    ComputerAdvertisement.parse(info.serviceName, host, attributes)?.let {
                        found[info.serviceName] = it
                        changed(found.values.toList())
                    }
                }
                busy = false; resolveNext()
            }
        })
    }
    override fun close() {
        if (closed.compareAndSet(false, true)) runCatching { manager.stopServiceDiscovery(listener) }
        synchronized(this) { resolving.clear(); found.clear() }
    }
}

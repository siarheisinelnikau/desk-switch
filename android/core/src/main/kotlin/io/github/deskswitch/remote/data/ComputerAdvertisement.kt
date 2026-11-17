package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Computer

/** Capability hints from the original desktop announcement, never trust grants. */
object ComputerAdvertisement {
    const val SERVICE = "_desk-switch._tcp."

    fun parse(serviceName: String, host: String, attributes: Map<String, String>): Computer? = runCatching {
        require(attributes["app"] == "desk-switch")
        require(attributes["protocol"] == NativeWire.PROTOCOL.toString())
        require(attributes["workspace-protocol"] == NativeWire.WORKSPACE_PROTOCOL.toString())
        val key = attributes.getValue("key")
        require(NoiseSessionFactory.unhex(key).any { it != 0.toByte() })
        val port = attributes.getValue("remote-port").toInt()
        val inputPort = attributes.getValue("remote-input-port").toInt()
        require(port in 1..65535 && inputPort in 1..65535)
        require(host.isNotBlank() && host.length <= 253)
        val name = attributes["name"]?.trim()?.take(256)?.filterNot(Char::isISOControl)
            ?.takeIf { it.isNotBlank() } ?: serviceName.take(256)
        Computer(name, host, port, inputPort, advertisedKey = key, previewSupported = attributes["screen-preview"] == "1")
    }.getOrNull()
}

package io.github.deskswitch.remote.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ProtocolCompatibilityTest {
    @Test fun differentApplicationVersionsKeepTheSameDeviceProtocol() {
        val released = "0.3.2-experimental"
        val payload = NativeWire.version(released)
        assertEquals(released, payload.copyOfRange(11, payload.size).toString(Charsets.US_ASCII))
        NativeWire.requireVersion(payload)
        NativeWire.requireVersion(NativeWire.version("42.0.1"))
        NativeWire.requireProtocols(NativeWire.PROTOCOL, NativeWire.WORKSPACE_PROTOCOL)
    }

    @Test fun mismatchesIdentifyTheProtocolAndBothVersions() {
        val bytes = NativeWire.version("0.11.3")
        bytes[8] = 1
        val device = assertThrows(ProtocolVersionMismatch::class.java) { NativeWire.requireVersion(bytes) }
        assertEquals(ProtocolKind.DEVICE, device.kind)
        assertEquals(NativeWire.PROTOCOL, device.expected)
        assertEquals(256 + NativeWire.PROTOCOL, device.received)

        val workspace = assertThrows(ProtocolVersionMismatch::class.java) {
            NativeWire.requireProtocols(NativeWire.PROTOCOL, NativeWire.WORKSPACE_PROTOCOL + 1)
        }
        assertEquals(ProtocolKind.WORKSPACE, workspace.kind)
        assertEquals(NativeWire.WORKSPACE_PROTOCOL, workspace.expected)
        assertEquals(NativeWire.WORKSPACE_PROTOCOL + 1, workspace.received)
    }
}

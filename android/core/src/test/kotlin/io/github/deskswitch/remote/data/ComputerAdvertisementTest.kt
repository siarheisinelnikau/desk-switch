package io.github.deskswitch.remote.data

import org.junit.Assert.*
import org.junit.Test

class ComputerAdvertisementTest {
    private fun record() = mapOf(
        "app" to "desk-switch", "name" to "TV computer", "version" to "99.1.0",
        "protocol" to "9", "workspace-protocol" to "3", "key" to "01".repeat(32),
        "remote-port" to "45882", "remote-input-port" to "45883",
    )

    @Test fun originalDesktopAnnouncementUsesProtocolCompatibilityAndNeverGrantsTrust() {
        val computer = ComputerAdvertisement.parse("ds-fixture", "192.168.1.10", record())!!
        assertEquals("_desk-switch._tcp.", ComputerAdvertisement.SERVICE)
        assertEquals("TV computer", computer.name)
        assertEquals(45883, computer.inputPort)
        assertEquals("01".repeat(32), computer.advertisedKey)
        assertNull(computer.key)
    }

    @Test fun desktopWithoutPhoneSupportOrDifferentProtocolIsNotAnAvailableRemote() {
        for (record in listOf(
            record() - "remote-port", record() - "remote-input-port",
            record() + ("protocol" to "10"), record() + ("workspace-protocol" to "4"),
            record() + ("remote-port" to "0"), record() + ("key" to "00".repeat(32)),
            record() + ("app" to "other-app"),
        )) assertNull(ComputerAdvertisement.parse("fixture", "192.168.1.10", record))
    }
}

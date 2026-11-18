package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.ConnectionChoices
import io.github.deskswitch.remote.domain.WorkspaceIdentity
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class WorkspacePairingTest {
    private val firstKey = "01".repeat(32)
    private val alternateKey = "02".repeat(32)
    private val unknownKey = "03".repeat(32)
    private val workspace = WorkspaceIdentity("10".repeat(32), setOf(firstKey, alternateKey))
    private val first = Computer("First PC", "192.168.1.10", key = firstKey)
    private val alternate = Computer("Second PC", "192.168.1.11", key = alternateKey)

    private fun advertised(computer: Computer, workspaceId: String = workspace.id) =
        computer.copy(key = null, advertisedKey = computer.key, advertisedWorkspaceId = workspaceId)

    @Test fun advertisedWorkspaceGroupsNearbyWithoutPinningAnUntrustedKey() {
        val computers = listOf(advertised(first), advertised(alternate))
        val choices = ConnectionChoices.from(emptyList(), computers)
        assertEquals(listOf(computers.first()), choices.nearby)
        assertNull(ConnectionChoices.pin(choices.nearby.single(), emptyList()).key)
        assertNull(choices.nearby.single().workspace)
    }

    @Test fun savedWorkspaceSelectsOnlyAKeyFromItsAuthenticatedRoster() {
        val saved = listOf(first.copy(workspace = workspace))
        val impostor = advertised(first.copy(name = "A", key = unknownKey))
        val restored = advertised(alternate)
        val choices = ConnectionChoices.from(saved, listOf(impostor, restored))
        assertEquals(alternateKey, choices.saved.single().key)
        assertEquals(alternate.host, choices.saved.single().host)
        assertEquals(workspace, choices.saved.single().workspace)
        assertEquals(listOf(impostor), choices.nearby)
        assertNull(ConnectionChoices.pin(impostor, saved).key)
        assertEquals(first, ConnectionChoices.from(listOf(first), listOf(impostor)).saved.single())
        assertEquals(alternateKey, ConnectionChoices.pin(restored, saved).key)
    }

    @Test fun wireRosterMustIncludeAuthenticatedResponderAndCanonicalKeys() {
        val metadata = JSONObject().put("workspace", WorkspaceWire.write(workspace))
        assertEquals(workspace, WorkspaceWire.read(metadata, firstKey))
        assertNull(WorkspaceWire.read(JSONObject().put("Decision", true), firstKey))
        assertThrows(IllegalArgumentException::class.java) { WorkspaceWire.read(metadata, unknownKey) }
        metadata.getJSONObject("workspace").put("members", JSONArray(listOf(firstKey, "AA".repeat(32))))
        assertThrows(IllegalArgumentException::class.java) { WorkspaceWire.read(metadata, firstKey) }
        metadata.getJSONObject("workspace").put("members", JSONArray(List(33) { firstKey }))
        assertThrows(IllegalArgumentException::class.java) { WorkspaceWire.read(metadata, firstKey) }
    }

    @Test fun legacyHandshakeDoesNotReadAStatusBeforeSendingDecision() {
        val legacy = JSONObject()
        assertTrue(requiresPairingCode(legacy, pinned = false) { error("Legacy peer awaits Decision") })
        assertFalse(requiresPairingCode(legacy, pinned = true) { error("Legacy peer awaits Decision") })
    }

    @Test fun negotiatedStatusCanRequireACodeEvenForAPinnedComputer() {
        val hello = JSONObject().put("phone_pairing_status", true)
        fun status(confirm: Boolean) = JSONObject().put("PhonePairing", JSONObject().put("confirm", confirm))
        assertTrue(requiresPairingCode(hello, pinned = false) { status(true) })
        assertTrue(requiresPairingCode(hello, pinned = true) { status(true) })
        assertFalse(requiresPairingCode(hello, pinned = true) { status(false) })
        assertThrows(IllegalStateException::class.java) {
            requiresPairingCode(hello, pinned = false) { status(false) }
        }
    }

    @Test fun negotiatedRejectionReportsTheDesktopReason() {
        val hello = JSONObject().put("phone_pairing_status", true)
        val status = JSONObject().put("PhonePairing", JSONObject().put("confirm", false).put("error", "Approval was removed"))
        val error = assertThrows(IllegalStateException::class.java) {
            requiresPairingCode(hello, pinned = true) { status }
        }
        assertEquals("Approval was removed", error.message)
    }

    @Test fun legacySavedEntriesMergeAndRoundTripWhenWorkspaceIsAuthenticated() {
        val legacyJson = """[
            {"name":"First PC","host":"192.168.1.10","port":45884,"inputPort":45883,"key":"$firstKey"},
            {"name":"Second PC","host":"192.168.1.11","port":45884,"inputPort":45883,"key":"$alternateKey"}
        ]"""
        val legacy = SavedComputers.read(legacyJson)
        assertEquals(listOf(first, alternate), legacy)
        val enrolled = first.copy(workspace = workspace)
        val merged = SavedComputers.merge(legacy, enrolled.copy(advertisedWorkspaceId = "11".repeat(32)))
        assertEquals(listOf(enrolled), merged)
        assertEquals(merged, SavedComputers.read(SavedComputers.write(merged)))
        assertEquals(merged, SavedComputers.merge(merged, alternate))
        assertTrue(ConnectionChoices.sameEntry(enrolled, alternate.copy(workspace = workspace)))
    }
}

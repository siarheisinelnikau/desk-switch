package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.ConnectionChoices
import org.json.JSONArray
import org.json.JSONObject

object SavedComputers {
    private const val MAX_ENTRIES = 16

    fun read(json: String): List<Computer> = runCatching {
        val array = JSONArray(json)
        require(array.length() <= MAX_ENTRIES)
        (0 until array.length()).mapNotNull { index ->
            runCatching {
                val item = array.getJSONObject(index)
                val key = item.getString("key").also { NoiseSessionFactory.unhex(it) }
                Computer(item.getString("name"), item.getString("host"), item.getInt("port"), item.getInt("inputPort"), key,
                    previewSupported = item.optBoolean("previewSupported", false), workspace = WorkspaceWire.read(item, key))
            }.getOrNull()
        }.fold(emptyList<Computer>(), ::merge)
    }.getOrDefault(emptyList())

    fun write(computers: List<Computer>): String {
        val array = JSONArray()
        computers.forEach {
            val item = JSONObject().put("name", it.name).put("host", it.host).put("port", it.port)
                .put("inputPort", it.inputPort).put("key", it.key).put("previewSupported", it.previewSupported)
            it.workspace?.let { workspace -> item.put("workspace", WorkspaceWire.write(workspace)) }
            array.put(item)
        }
        return array.toString()
    }

    fun merge(computers: List<Computer>, computer: Computer): List<Computer> {
        val workspace = computer.workspace
        if (workspace == null && computers.any { computer.key in it.workspace?.members.orEmpty() }) return computers
        return (computers.filterNot {
            if (workspace == null) ConnectionChoices.sameEntry(it, computer)
            else it.workspace?.id == workspace.id || (it.workspace == null && it.key in workspace.members)
        } + computer.copy(advertisedKey = null, advertisedWorkspaceId = null)).takeLast(MAX_ENTRIES)
    }
}

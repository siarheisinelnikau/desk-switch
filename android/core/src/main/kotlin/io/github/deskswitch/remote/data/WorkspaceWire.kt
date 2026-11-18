package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.WorkspaceIdentity
import org.json.JSONArray
import org.json.JSONObject

internal object WorkspaceWire {
    fun read(container: JSONObject, responder: String): WorkspaceIdentity? {
        if (!container.has("workspace")) return null
        val workspace = container.getJSONObject("workspace")
        val members = workspace.getJSONArray("members")
        require(members.length() in 1..WorkspaceIdentity.MAX_MEMBERS) { "Invalid workspace roster" }
        val keys = (0 until members.length()).map(members::getString).toSet()
        require(keys.size == members.length() && responder in keys) { "Invalid workspace roster" }
        return WorkspaceIdentity(workspace.getString("id"), keys)
    }

    fun write(workspace: WorkspaceIdentity): JSONObject = JSONObject()
        .put("id", workspace.id)
        .put("members", JSONArray(workspace.members.sorted()))
}

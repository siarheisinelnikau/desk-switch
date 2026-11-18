package io.github.deskswitch.remote.domain

data class ConnectionChoices(val saved: List<Computer>, val nearby: List<Computer>) {
    companion object {
        fun from(saved: List<Computer>, nearby: List<Computer>): ConnectionChoices {
            val ordered = nearby.sortedWith(compareBy({ it.name.lowercase() }, { it.advertisedKey }, { it.host }, { it.port }))
            val trusted = saved.flatMap { it.workspace?.members ?: listOfNotNull(it.key) }.toSet()
            val savedTargets = saved.map { known ->
                val members = known.workspace?.members ?: listOfNotNull(known.key)
                val available = ordered.filter { it.advertisedKey in members }
                val target = available.firstOrNull { it.advertisedKey == known.key } ?: available.firstOrNull()
                target?.copy(key = target.advertisedKey, advertisedWorkspaceId = null, workspace = known.workspace) ?: known
            }
            val nearbyTargets = ordered.filterNot { it.advertisedKey in trusted }
                .distinctBy { (it.advertisedWorkspaceId != null) to (it.advertisedWorkspaceId ?: it.advertisedKey ?: it.host) }
            return ConnectionChoices(savedTargets, nearbyTargets)
        }

        fun pin(computer: Computer, saved: List<Computer>): Computer {
            if (computer.key != null) return computer
            val key = computer.advertisedKey
            val known = if (key != null) {
                saved.firstOrNull { it.key == key || key in it.workspace?.members.orEmpty() }
            } else {
                saved.firstOrNull { it.host == computer.host && it.port == computer.port }
            }
            return known?.let { computer.copy(key = key ?: it.key, workspace = it.workspace) } ?: computer
        }

        fun sameEntry(a: Computer, b: Computer): Boolean =
            if (a.workspace != null || b.workspace != null) a.workspace?.id == b.workspace?.id
            else a.key != null && a.key == b.key
    }
}

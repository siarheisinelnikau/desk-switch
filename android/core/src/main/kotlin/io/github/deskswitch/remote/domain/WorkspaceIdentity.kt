package io.github.deskswitch.remote.domain

data class WorkspaceIdentity(val id: String, val members: Set<String>) {
    init {
        require(isCanonicalIdentity(id))
        require(members.size in 1..MAX_MEMBERS && members.all(::isCanonicalIdentity))
    }

    companion object {
        const val MAX_MEMBERS = 32

        fun isCanonicalIdentity(value: String): Boolean =
            value.length == 64 && value.all { it in '0'..'9' || it in 'a'..'f' }
    }
}

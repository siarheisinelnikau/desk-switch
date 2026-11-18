package io.github.deskswitch.remote.domain

sealed interface Input {
    data class Move(val dx: Int, val dy: Int) : Input
    data class Scroll(val dx: Int, val dy: Int) : Input
    data class Key(val code: Int, val down: Boolean) : Input
    data class Button(val code: Int, val down: Boolean) : Input
}

data class Computer(
    val name: String, val host: String, val port: Int = DEFAULT_CONTROL_PORT,
    val inputPort: Int = 45883, val key: String? = null,
    val advertisedKey: String? = null,
    val previewSupported: Boolean = false,
) {
    companion object {
        const val DEFAULT_CONTROL_PORT = 45884
        internal const val LEGACY_CONTROL_PORT = 45882
    }
}
data class Identity(val privateKey: ByteArray, val publicKey: ByteArray)
data class Pairing(val computer: Computer, val code: String, val publicKey: String)

interface RemoteSession : AutoCloseable {
    fun input(event: Input)
    fun text(value: String)
    fun preview(options: ScreenOptions) = Unit
}

interface RemoteStore {
    fun identity(): Identity
    fun computers(): List<Computer>
    fun save(computer: Computer)
    fun forget(key: String?)
}

interface SessionFactory : AutoCloseable {
    fun setPreviewListener(listener: (ByteArray?) -> Unit) = Unit
    fun setPreviewErrorListener(listener: (String?) -> Unit) = Unit
    fun setInputErrorListener(listener: (String) -> Unit) = Unit
    fun connect(computer: Computer, identity: Identity, confirm: (Pairing) -> Boolean, onClosed: (String?) -> Unit): RemoteSession
}

package io.github.deskswitch.remote.domain

/** Optional authenticated cursor-region preview; input does not depend on it. */
data class ScreenOptions(
    val enabled: Boolean = false,
    val fps: Int = 10,
    val width: Int = 384,
    val height: Int = 256,
) {
    init {
        require(fps in 1..30) { "Choose a preview frame rate from 1 to 30 FPS" }
        require(width in 128..640 && height in 96..480) { "Invalid preview region size" }
    }
}

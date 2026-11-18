package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Input
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/** Coalesces queued motion while preserving the order of button, key, and text commands. */
internal class InputQueue(private val capacity: Int = 128, private val reserve: Int = 16) {
    private val lock = ReentrantLock()
    private val available = lock.newCondition()
    private val pending = ArrayDeque<Any>()

    fun offer(command: Any): Boolean = lock.withLock {
        if (command is Input && (command is Input.Move || command is Input.Scroll)) {
            val merged = merge(pending.lastOrNull(), command)
            if (merged != null) {
                pending[pending.lastIndex] = merged
                return true
            }
            // A motion burst must leave space for a press or release to follow it.
            if (pending.size >= capacity - reserve) return true
        }
        if (pending.size == capacity) return false
        pending.addLast(command)
        available.signal()
        true
    }

    fun take(): Any = lock.withLock {
        while (pending.isEmpty()) available.await()
        pending.removeFirst()
    }

    fun clear() = lock.withLock { pending.clear() }

    private fun merge(previous: Any?, next: Input): Input? = when {
        previous is Input.Move && next is Input.Move -> {
            val dx = previous.dx + next.dx
            val dy = previous.dy + next.dy
            val allowed = -NativeWire.MAX_MOVE..NativeWire.MAX_MOVE
            if (dx in allowed && dy in allowed) Input.Move(dx, dy) else null
        }
        previous is Input.Scroll && next is Input.Scroll -> {
            val dx = previous.dx + next.dx
            val dy = previous.dy + next.dy
            val allowed = -NativeWire.MAX_SCROLL..NativeWire.MAX_SCROLL
            if (dx in allowed && dy in allowed) Input.Scroll(dx, dy) else null
        }
        else -> null
    }
}

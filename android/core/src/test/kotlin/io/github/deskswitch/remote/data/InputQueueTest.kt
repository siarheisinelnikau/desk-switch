package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Input
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InputQueueTest {
    @Test(timeout = 2000)
    fun movementAccumulatesWithoutCrossingClicksOrText() {
        val queue = InputQueue()
        val down = Input.Button(272, true)
        val up = Input.Button(272, false)
        repeat(500) { assertTrue(queue.offer(Input.Move(1, 2))) }
        assertTrue(queue.offer(down))
        repeat(500) { assertTrue(queue.offer(Input.Move(2, 1))) }
        assertTrue(queue.offer(up))
        assertTrue(queue.offer("typed text"))
        repeat(500) { assertTrue(queue.offer(Input.Scroll(0, -1))) }

        assertEquals(Input.Move(500, 1000), queue.take())
        assertEquals(down, queue.take())
        assertEquals(Input.Move(1000, 500), queue.take())
        assertEquals(up, queue.take())
        assertEquals("typed text", queue.take())
        assertEquals(Input.Scroll(0, -500), queue.take())
    }

    @Test(timeout = 2000)
    fun saturatedMotionLeavesSpaceForPressAndRelease() {
        val queue = InputQueue(capacity = 4, reserve = 2)
        repeat(500) { assertTrue(queue.offer(Input.Move(16384, 0))) }
        assertTrue(queue.offer(Input.Key(42, true)))
        assertTrue(queue.offer(Input.Key(42, false)))

        repeat(2) { assertEquals(Input.Move(16384, 0), queue.take()) }
        assertEquals(Input.Key(42, true), queue.take())
        assertEquals(Input.Key(42, false), queue.take())
    }
}

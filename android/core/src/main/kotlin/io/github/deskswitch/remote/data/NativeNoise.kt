package io.github.deskswitch.remote.data

import io.github.deskswitch.remote.domain.Identity

fun noiseIdentity(previousPrivateKey: ByteArray? = null): Identity {
    val keys = NativeNoise.identityKeys(previousPrivateKey)
    return try {
        Identity(keys.copyOfRange(0, 32), keys.copyOfRange(32, 64))
    } finally {
        keys.fill(0)
    }
}

internal object NativeNoise {
    init {
        System.loadLibrary("desk_switch_noise")
    }

    external fun identityKeys(previousPrivateKey: ByteArray?): ByteArray
    external fun start(kk: Boolean, privateKey: ByteArray, prologue: ByteArray, remoteKey: ByteArray?, limit: Int): Long
    external fun message(handle: Long, write: Boolean, source: ByteArray): ByteArray

    // The result contains the remote static key followed by the handshake hash.
    external fun finish(handle: Long): ByteArray
    external fun release(handle: Long)
}

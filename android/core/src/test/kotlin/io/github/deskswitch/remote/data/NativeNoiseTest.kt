package io.github.deskswitch.remote.data

import org.junit.Assert.assertArrayEquals
import org.junit.Test

class NativeNoiseTest {
    @Test fun existingRawPrivateKeyRetainsItsPublicIdentity() {
        val secret = NoiseSessionFactory.unhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val public = NoiseSessionFactory.unhex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        val identity = noiseIdentity(secret)

        assertArrayEquals(secret, identity.privateKey)
        assertArrayEquals(public, identity.publicKey)
    }
}

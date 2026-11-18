package io.github.deskswitch.remote.data

import android.content.Context
import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.ConnectionChoices
import io.github.deskswitch.remote.domain.Identity
import io.github.deskswitch.remote.domain.RemoteStore

/** App-private, backup-excluded identity and pinned computers. Never shared or logged. */
class PhoneStore(context: Context) : RemoteStore {
    private val prefs = context.getSharedPreferences("remote", Context.MODE_PRIVATE)
    @Synchronized override fun identity(): Identity {
        val previous = prefs.getString("private", null)
        val secret = previous?.let { NoiseSessionFactory.unhex(it) }
        val identity = try {
            noiseIdentity(secret)
        } finally {
            secret?.fill(0)
        }
        if (previous == null) {
            check(prefs.edit().putString("private", NoiseSessionFactory.hex(identity.privateKey)).commit()) {
                "Could not save the phone identity"
            }
        }
        return identity
    }
    @Synchronized override fun computers(): List<Computer> {
        return SavedComputers.read(prefs.getString("computers", "[]") ?: "[]")
    }
    @Synchronized override fun save(computer: Computer) {
        write(SavedComputers.merge(computers(), computer))
    }
    @Synchronized override fun forget(computer: Computer) {
        write(computers().filterNot { ConnectionChoices.sameEntry(it, computer) })
    }
    private fun write(computers: List<Computer>) {
        check(prefs.edit().putString("computers", SavedComputers.write(computers)).commit()) { "Could not save computers" }
    }
}

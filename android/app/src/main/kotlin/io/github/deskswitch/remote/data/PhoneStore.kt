package io.github.deskswitch.remote.data

import android.content.Context
import com.southernstorm.noise.protocol.Noise
import io.github.deskswitch.remote.domain.Computer
import io.github.deskswitch.remote.domain.Identity
import io.github.deskswitch.remote.domain.RemoteStore
import org.json.JSONArray
import org.json.JSONObject

/** App-private, backup-excluded identity and pinned computers. Never shared or logged. */
class PhoneStore(context: Context) : RemoteStore {
    private val prefs = context.getSharedPreferences("remote", Context.MODE_PRIVATE)
    @Synchronized override fun identity(): Identity {
        val previous = prefs.getString("private", null)
        val key = Noise.createDH("25519")
        try {
            if (previous != null) key.setPrivateKey(NoiseSessionFactory.unhex(previous), 0) else key.generateKeyPair()
            val secret = ByteArray(32).also { key.getPrivateKey(it, 0) }
            val public = ByteArray(32).also { key.getPublicKey(it, 0) }
            if (previous == null) check(prefs.edit().putString("private", NoiseSessionFactory.hex(secret)).commit()) { "Could not save the phone identity" }
            return Identity(secret, public)
        } finally { key.destroy() }
    }
    @Synchronized override fun computers(): List<Computer> {
        return runCatching {
            val array = JSONArray(prefs.getString("computers", "[]"))
            require(array.length() <= 16)
            (0 until array.length()).map { index ->
                val item = array.getJSONObject(index)
                Computer(item.getString("name"), item.getString("host"), item.getInt("port"), item.getInt("inputPort"),
                    item.getString("key").also { NoiseSessionFactory.unhex(it) }, previewSupported = item.optBoolean("previewSupported", false))
            }
        }.getOrDefault(emptyList())
    }
    @Synchronized override fun save(computer: Computer) {
        write((computers().filterNot { it.key == computer.key } + computer).takeLast(16))
    }
    @Synchronized override fun forget(key: String?) { write(computers().filterNot { it.key == key }) }
    private fun write(computers: List<Computer>) {
        val array = JSONArray()
        computers.forEach { array.put(JSONObject().put("name", it.name).put("host", it.host).put("port", it.port)
            .put("inputPort", it.inputPort).put("key", it.key).put("previewSupported", it.previewSupported)) }
        check(prefs.edit().putString("computers", array.toString()).commit()) { "Could not save computers" }
    }
}

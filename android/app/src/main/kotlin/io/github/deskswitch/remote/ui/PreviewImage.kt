package io.github.deskswitch.remote.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory

/** A bounded render adapter. No frame is written to storage or a cache. */
internal object PreviewImage {
    fun decode(bytes: ByteArray): Bitmap? {
        if (bytes.size > 256 * 1024) return null
        val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        if (options.outWidth !in 1..1024 || options.outHeight !in 1..1024) return null
        if (options.outWidth.toLong() * options.outHeight > 1024L * 1024) return null
        options.inJustDecodeBounds = false
        options.inPreferredConfig = Bitmap.Config.ARGB_8888
        return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
    }
}

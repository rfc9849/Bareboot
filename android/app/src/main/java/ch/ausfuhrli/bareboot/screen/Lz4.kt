package ch.ausfuhrli.bareboot.screen

import java.io.IOException

object Lz4 {
    fun decompressSizePrepended(src: ByteArray, offset: Int, length: Int): ByteArray {
        if (length < 4) throw IOException("truncated pixels")
        val size = (src[offset].toInt() and 0xFF) or
            ((src[offset + 1].toInt() and 0xFF) shl 8) or
            ((src[offset + 2].toInt() and 0xFF) shl 16) or
            ((src[offset + 3].toInt() and 0xFF) shl 24)
        if (size < 0 || size > 256 shl 20) throw IOException("invalid pixel size")
        val out = ByteArray(size)
        val end = offset + length
        var s = offset + 4
        var d = 0
        try {
            while (s < end) {
                val token = src[s++].toInt() and 0xFF
                var literals = token ushr 4
                if (literals == 15) {
                    do {
                        val b = src[s++].toInt() and 0xFF
                        literals += b
                    } while (b == 255)
                }
                System.arraycopy(src, s, out, d, literals)
                s += literals
                d += literals
                if (s >= end) break
                val distance = (src[s].toInt() and 0xFF) or ((src[s + 1].toInt() and 0xFF) shl 8)
                s += 2
                var match = token and 15
                if (match == 15) {
                    do {
                        val b = src[s++].toInt() and 0xFF
                        match += b
                    } while (b == 255)
                }
                match += 4
                val from = d - distance
                if (distance == 0 || from < 0) throw IOException("invalid pixel data")
                if (distance >= match) {
                    System.arraycopy(out, from, out, d, match)
                } else {
                    for (i in 0 until match) out[d + i] = out[from + i]
                }
                d += match
            }
        } catch (e: IndexOutOfBoundsException) {
            throw IOException("invalid pixel data", e)
        }
        if (d != size) throw IOException("invalid pixel data")
        return out
    }
}

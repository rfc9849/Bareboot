package ch.ausfuhrli.bareboot.screen

import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import java.io.ByteArrayOutputStream
import java.io.FileOutputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder

class DisplayLink(
    private val fd: ParcelFileDescriptor,
    private val onMessage: (kind: Int, body: ByteArray) -> Unit,
    private val onClosed: (String) -> Unit,
) {
    private val output = FileOutputStream(fd.fileDescriptor)
    private val pending = ArrayDeque<Int>()
    @Volatile private var closed = false

    fun start() {
        Thread({ readLoop() }, "display-link").start()
    }

    fun send(kind: Int, vararg words: Int) {
        val buffer = ByteBuffer.allocate(8 + words.size * 4).order(ByteOrder.LITTLE_ENDIAN)
        buffer.putInt(kind).putInt(words.size * 4)
        words.forEach { buffer.putInt(it) }
        synchronized(output) {
            if (closed) return
            try {
                output.write(buffer.array())
            } catch (_: IOException) {
            }
        }
    }

    fun takeFds(count: Int): IntArray? {
        if (count < 0 || pending.size < count) return null
        return IntArray(count) { pending.removeFirst() }
    }

    fun close() {
        synchronized(output) {
            if (closed) return
            closed = true
        }
        runCatching { Os.shutdown(fd.fileDescriptor, OsConstants.SHUT_RDWR) }
    }

    private fun readLoop() {
        val chunk = ByteArray(256 * 1024)
        val fds = IntArray(17)
        val input = ByteArrayOutputStream()
        var reason = ""
        try {
            while (true) {
                val n = Native.recv(fd.fd, chunk, fds)
                for (i in 1..fds[0]) pending.addLast(fds[i])
                if (n == 0) break
                if (n < 0) throw IOException("receive failed: ${-n}")
                input.write(chunk, 0, n)
                val data = input.toByteArray()
                var pos = 0
                while (data.size - pos >= 8) {
                    val header = ByteBuffer.wrap(data, pos, 8).order(ByteOrder.LITTLE_ENDIAN)
                    val kind = header.int
                    val length = header.int
                    if (length < 0 || length > 64 shl 20) throw IOException("message too large")
                    if (data.size - pos - 8 < length) break
                    onMessage(kind, data.copyOfRange(pos + 8, pos + 8 + length))
                    pos += 8 + length
                }
                input.reset()
                input.write(data, pos, data.size - pos)
            }
        } catch (e: IOException) {
            if (!closed) reason = e.message ?: "connection closed"
        }
        synchronized(output) { closed = true }
        pending.forEach { runCatching { ParcelFileDescriptor.adoptFd(it).close() } }
        runCatching { fd.close() }
        onClosed(reason)
    }

    companion object {
        const val VERSION = 1
        const val CAP_LAYERS = 2

        const val HELLO = 1
        const val CONFIGURE = 2
        const val FOCUS = 4
        const val MOTION = 5
        const val BUTTON = 6
        const val AXIS = 7
        const val KEY = 8
        const val LEAVE = 9
        const val RELEASE = 11

        const val OPEN = 0x10
        const val CLOSE = 0x11
        const val SIZE = 0x13
        const val CURSOR = 0x16
        const val CURSOR_IMAGE = 0x17
        const val PIXELS = 0x21
        const val LAYER = 0x22
        const val LAYER_MOVE = 0x23
        const val LAYER_REMOVE = 0x24
    }
}

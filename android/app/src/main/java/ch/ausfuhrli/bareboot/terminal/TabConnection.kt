package ch.ausfuhrli.bareboot.terminal

import android.os.Handler
import android.os.Looper
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import ch.ausfuhrli.bareboot.ipc.AttachedTab
import ch.ausfuhrli.bareboot.ipc.BarebootClient
import io.github.sagernet.libghostty.GhosttyTerminalSession
import java.io.FileDescriptor
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

data class TerminalTab(
    val id: Int,
    val distro: String,
    val uid: Int,
    val pid: Int,
    val session: GhosttyTerminalSession,
)

class TabConnection(
    private val sessionId: Int,
    private val attached: AttachedTab,
) : GhosttyTerminalSession.Transport {
    private val master = attached.master
    private val io = Executors.newSingleThreadExecutor { Thread(it, "bareboot-tab-$sessionId-io") }
    private val closing = AtomicBoolean(false)
    private val wake: Array<FileDescriptor> = Os.pipe()

    fun start(session: GhosttyTerminalSession, onExit: () -> Unit) {
        if (attached.replay.isNotEmpty()) session.feedOutput(attached.replay)
        Thread({ readLoop(session, onExit) }, "bareboot-tab-$sessionId-read").apply {
            isDaemon = true
            start()
        }
    }

    private fun readLoop(session: GhosttyTerminalSession, onExit: () -> Unit) {
        val buffer = ByteArray(16 * 1024)
        val fds = arrayOf(
            StructPollfd().apply { fd = master; events = OsConstants.POLLIN.toShort() },
            StructPollfd().apply { fd = wake[0]; events = OsConstants.POLLIN.toShort() },
        )
        try {
            while (true) {
                fds.forEach { it.revents = 0 }
                try {
                    Os.poll(fds, -1)
                } catch (e: ErrnoException) {
                    if (e.errno == OsConstants.EINTR) continue
                    throw e
                }
                if (fds[1].revents.toInt() != 0) break
                if (fds[0].revents.toInt() == 0) continue
                val n = try {
                    Os.read(master, buffer, 0, buffer.size)
                } catch (e: ErrnoException) {
                    if (e.errno == OsConstants.EINTR || e.errno == OsConstants.EAGAIN) continue
                    -1
                }
                if (n <= 0) break
                session.feedOutput(buffer, 0, n)
            }
        } catch (_: ErrnoException) {
        }
        val ended = !closing.getAndSet(true)
        io.execute(::release)
        io.shutdown()
        if (ended) {
            session.finish()
            Handler(Looper.getMainLooper()).post(onExit)
        }
    }

    override fun sendInput(data: ByteArray) {
        if (closing.get()) return
        io.execute {
            if (closing.get()) return@execute
            var offset = 0
            try {
                while (offset < data.size) {
                    offset += Os.write(master, data, offset, data.size - offset)
                }
            } catch (_: ErrnoException) {
            }
        }
    }

    override fun sendResize(columns: Int, rows: Int, widthPixels: Int, heightPixels: Int) {
        if (closing.get()) return
        io.execute {
            if (closing.get()) return@execute
            try {
                BarebootClient.resizeTab(attached.socket, sessionId, columns, rows, widthPixels, heightPixels)
            } catch (_: Exception) {
            }
        }
    }

    override fun close() {
        if (closing.getAndSet(true)) return
        try {
            Os.write(wake[1], byteArrayOf(1), 0, 1)
        } catch (_: ErrnoException) {
        }
    }

    private fun release() {
        listOf(master, wake[0], wake[1]).forEach {
            try {
                Os.close(it)
            } catch (_: ErrnoException) {
            }
        }
        try {
            attached.socket.close()
        } catch (_: Exception) {
        }
    }
}

package ch.ausfuhrli.bareboot.ipc

import android.net.LocalSocket
import android.net.LocalSocketAddress
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.EOFException
import java.io.FileDescriptor
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder

data class KernelInfo(
    val pageSize: Int,
    val selinuxEnforcing: Int,
    val hasMntNs: Boolean,
    val hasUtsNs: Boolean,
    val hasPidNs: Boolean,
    val hasIpcNs: Boolean,
    val hasNetNs: Boolean,
    val hasUserNs: Boolean,
    val hasCgroupNs: Boolean,
    val isolatedNetwork: Boolean,
)

data class DistroStatus(
    val name: String,
    val imagePath: String,
    val holderPid: Int,
    val initPid: Int,
)

data class SessionInfo(
    val sessionId: Int,
    val distro: String,
    val pid: Int,
    val uid: Int,
)

data class PortForward(
    val hostPort: Int,
    val guestPort: Int,
)

data class ImageInfo(
    val name: String,
    val imagePath: String,
    val sizeBytes: Long,
    val usedBytes: Long,
    val osLabel: String,
    val username: String,
    val fsSizeBytes: Long,
    val fsFreeBytes: Long,
    val keepAwake: Boolean,
    val init: String,
    val isolatedNet: Boolean,
    val sshPort: Int,
    val portForwards: List<PortForward>,
    val busy: String,
    val gpuEnabled: Boolean,
    val gpuAvailable: Boolean,
    val gpuRunning: Boolean,
) {
    val bootsInit: Boolean get() = init == INIT_SYSTEM
}

const val INIT_NONE = "none"
const val INIT_SYSTEM = "system"

data class DaemonStatus(
    val kernel: KernelInfo,
    val distros: List<DistroStatus>,
    val sessions: List<SessionInfo>,
    val purgeOnUninstall: Boolean,
    val relaxedSync: Boolean,
)

data class PongInfo(
    val version: Int,
    val daemonPid: Int,
    val selinuxEnforcing: Int,
)

enum class CheckState { Missing, Present, Unknown }

data class KernelCheck(val id: String, val state: CheckState)

data class KernelReport(val release: String, val checks: List<KernelCheck>)

private const val LONG_JOB_TIMEOUT_MS = 3 * 60 * 60 * 1000

class BarebootRpcException(val code: Int, message: String) : IOException(message)

class DaemonUnavailableException(cause: IOException) : IOException(cause.message, cause)

const val PROTOCOL_VERSION = 19
const val DEFAULT_SOCKET_PATH = "/dev/socket/bareboot/barebootd.sock"

class BarebootClient(
    private val socketPath: String = DEFAULT_SOCKET_PATH,
) {
    fun ping(): PongInfo = call(encodeTag(1)) { r ->
        val tag = r.u8()
        when (tag) {
            2 -> PongInfo(r.u32(), r.i32(), r.i8())
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun status(): DaemonStatus = call(encodeTag(2)) { r ->
        val tag = r.u8()
        when (tag) {
            3 -> {
                val kernel = KernelInfo(
                    pageSize = r.u32(),
                    selinuxEnforcing = r.i8(),
                    hasMntNs = r.bool(),
                    hasUtsNs = r.bool(),
                    hasPidNs = r.bool(),
                    hasIpcNs = r.bool(),
                    hasNetNs = r.bool(),
                    hasUserNs = r.bool(),
                    hasCgroupNs = r.bool(),
                    isolatedNetwork = r.bool(),
                )
                val dCount = r.u32()
                val distros = List(dCount) {
                    DistroStatus(
                        name = r.str(),
                        imagePath = r.str(),
                        holderPid = r.i32(),
                        initPid = r.i32(),
                    )
                }
                val sCount = r.u32()
                val sessions = List(sCount) {
                    SessionInfo(
                        sessionId = r.u32(),
                        distro = r.str(),
                        pid = r.i32(),
                        uid = r.u32(),
                    )
                }
                DaemonStatus(kernel, distros, sessions, purgeOnUninstall = r.bool(), relaxedSync = r.bool())
            }
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun kernelReport(): KernelReport = call(encodeTag(15)) { r ->
        when (val tag = r.u8()) {
            8 -> {
                val release = r.str()
                val checks = List(r.u32()) {
                    val id = r.str()
                    val state = when (r.u8()) {
                        0 -> CheckState.Missing
                        1 -> CheckState.Present
                        else -> CheckState.Unknown
                    }
                    KernelCheck(id, state)
                }
                KernelReport(release, checks)
            }
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun listImages(): List<ImageInfo> = call(encodeTag(9)) { r ->
        val tag = r.u8()
        when (tag) {
            6 -> {
                val count = r.u32()
                List(count) {
                    ImageInfo(
                        name = r.str(),
                        imagePath = r.str(),
                        sizeBytes = r.u64(),
                        usedBytes = r.u64(),
                        osLabel = r.str(),
                        username = r.str(),
                        fsSizeBytes = r.u64(),
                        fsFreeBytes = r.u64(),
                        keepAwake = r.bool(),
                        init = r.str(),
                        isolatedNet = r.bool(),
                        sshPort = r.u16(),
                        portForwards = List(r.u32()) { PortForward(r.u16(), r.u16()) },
                        busy = r.str(),
                        gpuEnabled = r.bool(),
                        gpuAvailable = r.bool(),
                        gpuRunning = r.bool(),
                    )
                }
            }
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun startDistro(
        name: String,
        imagePath: String,
        sharedDir: String = "/data/media/0/Linux",
    ) {
        val w = FrameWriter().apply {
            u8(3)
            str(name)
            str(imagePath)
            str(sharedDir)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun stopDistro(name: String) {
        val w = FrameWriter().apply {
            u8(4)
            str(name)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun createDistro(name: String, sizeGb: Int, tarballPath: String, username: String, osLabel: String) {
        val w = FrameWriter().apply {
            u8(10)
            str(name)
            u32(sizeGb)
            str(tarballPath)
            str(username)
            str(osLabel)
        }
        call(w.toByteArray(), timeoutMs = LONG_JOB_TIMEOUT_MS) { r -> expectOk(r) }
    }

    fun configureImage(
        name: String,
        osLabel: String,
        username: String,
        keepAwake: Boolean,
        init: String,
        isolatedNet: Boolean,
        sshPort: Int = 0,
        portForwards: List<PortForward> = emptyList(),
    ) {
        val w = FrameWriter().apply {
            u8(17)
            str(name)
            str(osLabel)
            str(username)
            u8(if (keepAwake) 1 else 0)
            str(init)
            u8(if (isolatedNet) 1 else 0)
            u16(sshPort)
            u32(portForwards.size)
            portForwards.forEach {
                u16(it.hostPort)
                u16(it.guestPort)
            }
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun configureGpu(name: String, enabled: Boolean) {
        val w = FrameWriter().apply {
            u8(22)
            str(name)
            u8(if (enabled) 1 else 0)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun setDaemonOptions(purgeOnUninstall: Boolean, relaxedSync: Boolean) {
        val w = FrameWriter().apply {
            u8(20)
            u8(if (purgeOnUninstall) 1 else 0)
            u8(if (relaxedSync) 1 else 0)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun readLog(distro: String, name: String): String {
        val w = FrameWriter().apply {
            u8(21)
            str(distro)
            str(name)
        }
        return call(w.toByteArray()) { r ->
            when (val tag = r.u8()) {
                9 -> String(r.rest(), Charsets.UTF_8)
                255 -> throw BarebootRpcException(r.u32(), r.str())
                else -> throw IOException("unexpected response tag $tag")
            }
        }
    }

    fun renameImage(name: String, newName: String) {
        val w = FrameWriter().apply {
            u8(18)
            str(name)
            str(newName)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun exportImage(name: String, destPath: String) {
        val w = FrameWriter().apply {
            u8(19)
            str(name)
            str(destPath)
        }
        call(w.toByteArray(), timeoutMs = LONG_JOB_TIMEOUT_MS) { r -> expectOk(r) }
    }

    fun deleteDistro(name: String) {
        val w = FrameWriter().apply {
            u8(16)
            str(name)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    fun resizeDistro(name: String, sizeGb: Int) {
        val w = FrameWriter().apply {
            u8(11)
            str(name)
            u32(sizeGb)
        }
        call(w.toByteArray(), timeoutMs = LONG_JOB_TIMEOUT_MS) { r -> expectOk(r) }
    }

    fun createTab(distro: String, uid: Int, argv: List<String> = emptyList(), env: List<String> = emptyList()): SessionInfo {
        val w = FrameWriter().apply {
            u8(12)
            str(distro)
            u32(uid)
            strVec(argv)
            strVec(env)
        }
        return call(w.toByteArray()) { r ->
            when (val tag = r.u8()) {
                4 -> SessionInfo(r.u32(), distro, r.i32(), uid)
                255 -> throw BarebootRpcException(r.u32(), r.str())
                else -> throw IOException("unexpected response tag $tag")
            }
        }
    }

    fun listTabs(): List<SessionInfo> = call(encodeTag(14)) { r ->
        when (val tag = r.u8()) {
            5 -> List(r.u32()) { SessionInfo(r.u32(), r.str(), r.i32(), r.u32()) }
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun killSession(sessionId: Int, signal: Int = 15) {
        val w = FrameWriter().apply {
            u8(7)
            u32(sessionId)
            i32(signal)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    private fun expectOk(r: FrameReader) {
        when (val tag = r.u8()) {
            1 -> Unit
            255 -> throw BarebootRpcException(r.u32(), r.str())
            else -> throw IOException("unexpected response tag $tag")
        }
    }

    fun attachTab(sessionId: Int, cols: Int, rows: Int, widthPixels: Int, heightPixels: Int): AttachedTab {
        val sock = connect()
        try {
            sock.soTimeout = 15_000
            val w = FrameWriter().apply {
                u8(13)
                u32(sessionId)
                u16(cols)
                u16(rows)
                u16(widthPixels.coerceIn(0, 0xFFFF))
                u16(heightPixels.coerceIn(0, 0xFFFF))
            }
            val reader = exchange(sock, w.toByteArray())
            val fds = sock.ancillaryFileDescriptors
            when (val tag = reader.u8()) {
                7 -> {
                    val master = fds?.singleOrNull() ?: throw IOException("terminal tab arrived without a PTY")
                    sock.soTimeout = 0
                    return AttachedTab(sock, master, reader.rest())
                }
                255 -> throw BarebootRpcException(reader.u32(), reader.str())
                else -> throw IOException("unexpected response tag $tag")
            }
        } catch (e: Exception) {
            sock.close()
            throw e
        }
    }

    fun startScreen(name: String, displayId: Int) {
        val w = FrameWriter().apply {
            u8(24)
            str(name)
            u32(displayId)
        }
        call(w.toByteArray()) { r -> expectOk(r) }
    }

    private fun connect(): LocalSocket {
        val sock = LocalSocket(LocalSocket.SOCKET_STREAM)
        try {
            val addr = if (socketPath.startsWith("@")) {
                LocalSocketAddress(socketPath.substring(1), LocalSocketAddress.Namespace.ABSTRACT)
            } else {
                LocalSocketAddress(socketPath, LocalSocketAddress.Namespace.FILESYSTEM)
            }
            sock.connect(addr)
        } catch (e: IOException) {
            sock.close()
            throw DaemonUnavailableException(e)
        }
        return sock
    }

    private fun <T> call(
        payload: ByteArray,
        timeoutMs: Int = 15_000,
        decode: (FrameReader) -> T,
    ): T = connect().use { s ->
        s.soTimeout = timeoutMs
        decode(exchange(s, payload))
    }

    private fun encodeTag(tag: Int): ByteArray = byteArrayOf(tag.toByte())

    companion object {
        fun resizeTab(sock: LocalSocket, sessionId: Int, cols: Int, rows: Int, widthPixels: Int, heightPixels: Int) {
            val w = FrameWriter().apply {
                u8(6)
                u32(sessionId)
                u16(cols)
                u16(rows)
                u16(widthPixels.coerceIn(0, 0xFFFF))
                u16(heightPixels.coerceIn(0, 0xFFFF))
            }
            val r = exchange(sock, w.toByteArray())
            when (val tag = r.u8()) {
                1 -> Unit
                255 -> throw BarebootRpcException(r.u32(), r.str())
                else -> throw IOException("unexpected response tag $tag")
            }
        }

        private fun exchange(s: LocalSocket, payload: ByteArray): FrameReader {
            val out = s.outputStream
            val header = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(payload.size).array()
            val writeError = try {
                out.write(header)
                out.write(payload)
                out.flush()
                null
            } catch (e: IOException) {
                e
            }

            val inp = DataInputStream(s.inputStream)
            val lenBuf = ByteArray(4)
            try {
                inp.readFully(lenBuf)
            } catch (e: IOException) {
                throw writeError ?: e
            }
            val respLen = ByteBuffer.wrap(lenBuf).order(ByteOrder.LITTLE_ENDIAN).int
            if (respLen < 0 || respLen > 65536) {
                throw IOException("invalid response length: $respLen")
            }
            val respBuf = ByteArray(respLen)
            inp.readFully(respBuf)
            return FrameReader(respBuf)
        }
    }
}

class AttachedTab(val socket: LocalSocket, val master: FileDescriptor, val replay: ByteArray)

private class FrameWriter {
    private val out = ByteArrayOutputStream(128)

    fun u8(v: Int) {
        out.write(v and 0xFF)
    }

    fun u16(v: Int) {
        out.write(v and 0xFF)
        out.write((v shr 8) and 0xFF)
    }

    fun u32(v: Int) {
        val b = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(v).array()
        out.write(b)
    }

    fun i32(v: Int) = u32(v)

    fun str(s: String) {
        val bytes = s.toByteArray(Charsets.UTF_8)
        u32(bytes.size)
        out.write(bytes)
    }

    fun strVec(items: List<String>) {
        u32(items.size)
        items.forEach(::str)
    }

    fun toByteArray(): ByteArray = out.toByteArray()
}

private class FrameReader(private val data: ByteArray) {
    private var pos = 0

    private fun ensure(n: Int) {
        if (pos + n > data.size) throw EOFException("truncated frame")
    }

    fun u8(): Int {
        ensure(1)
        return data[pos++].toInt() and 0xFF
    }

    fun i8(): Int {
        ensure(1)
        return data[pos++].toInt()
    }

    fun bool(): Boolean = u8() != 0

    fun u16(): Int {
        ensure(2)
        val lo = data[pos++].toInt() and 0xFF
        val hi = data[pos++].toInt() and 0xFF
        return lo or (hi shl 8)
    }

    fun u32(): Int {
        ensure(4)
        val v = ByteBuffer.wrap(data, pos, 4).order(ByteOrder.LITTLE_ENDIAN).int
        pos += 4
        return v
    }

    fun u64(): Long {
        ensure(8)
        val v = ByteBuffer.wrap(data, pos, 8).order(ByteOrder.LITTLE_ENDIAN).long
        pos += 8
        return v
    }

    fun i32(): Int = u32()

    fun rest(): ByteArray = data.copyOfRange(pos, data.size).also { pos = data.size }

    fun str(): String {
        val len = u32()
        if (len < 0 || len > 65536) throw IOException("invalid string len $len")
        ensure(len)
        val s = String(data, pos, len, Charsets.UTF_8)
        pos += len
        return s
    }
}

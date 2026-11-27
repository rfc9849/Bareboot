package ch.ausfuhrli.bareboot.screen

import android.annotation.SuppressLint
import android.os.Build
import androidx.annotation.RequiresApi
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Rect
import android.hardware.HardwareBuffer
import android.hardware.SyncFence
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import android.view.SurfaceControl
import java.io.File
import java.io.FileInputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.concurrent.thread
import kotlin.math.floor
import kotlin.system.exitProcess

@RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
object ScreenHost {
    private const val TAG = "BarebootScreen"
    private const val EVIOCGRAB = 0x40044590
    private const val EV_SYN = 0
    private const val EV_KEY = 1
    private const val EV_REL = 2
    private const val REL_X = 0
    private const val REL_Y = 1
    private const val REL_HWHEEL = 6
    private const val REL_WHEEL = 8
    private const val KEY_LEFTCTRL = 29
    private const val KEY_RIGHTCTRL = 97
    private const val KEY_LEFTALT = 56
    private const val KEY_RIGHTALT = 100
    private const val KEY_BACKSPACE = 14

    private lateinit var main: Handler
    private lateinit var link: DisplayLink
    private lateinit var root: SurfaceControl
    private var screenWidth = 0
    private var screenHeight = 0
    private var scale = 1
    private val windows = LinkedHashMap<Int, Window>()
    private var pointerX = 0.0
    private var pointerY = 0.0
    private var grab = 0
    private var hover = 0
    private var focused = 0
    private val held = HashSet<Int>()

    private class Layer(val control: SurfaceControl) {
        var bufferWidth = 0
        var bufferHeight = 0
    }

    private class Window(val id: Int) {
        var logicalWidth = 0
        var logicalHeight = 0
        var x = 0
        var y = 0
        val container: SurfaceControl = SurfaceControl.Builder().setName("bareboot-window-$id").setParent(root).build()
        val pixels: SurfaceControl = SurfaceControl.Builder().setName("bareboot-pixels-$id").setParent(container).setBufferSize(1, 1).setOpaque(true).build()
        var pixelSurface: Surface? = null
        var bitmap: Bitmap? = null
        val layers = HashMap<Int, Layer>()
    }

    @JvmStatic
    fun main(args: Array<String>) {
        if (args.size != 3) {
            Log.e(TAG, "usage: ScreenHost <distro> <display-id> <display-fd>")
            exitProcess(2)
        }
        val classes = System.getenv("CLASSPATH")?.let(::File)
        System.load(File(classes?.parentFile, "libbareboot_screen.so").path)
        val displayId = args[1].toInt()
        val fd = ParcelFileDescriptor.adoptFd(args[2].toInt())
        val info = displayInfo(displayId)
        if (info == null) {
            Log.e(TAG, "display $displayId is not connected")
            exitProcess(1)
        }
        val layerStack = info.javaClass.getField("layerStack").getInt(info)
        screenWidth = info.javaClass.getField("logicalWidth").getInt(info)
        screenHeight = info.javaClass.getField("logicalHeight").getInt(info)
        val dpi = info.javaClass.getField("logicalDensityDpi").getInt(info)
        scale = floor(dpi / 160.0 + 0.25).toInt().coerceIn(1, 4)
        Looper.prepareMainLooper()
        main = Handler(Looper.myLooper()!!)

        root = SurfaceControl.Builder().setName("bareboot-screen").setBufferSize(screenWidth, screenHeight).setOpaque(true).build()
        val t = SurfaceControl.Transaction()
        SurfaceControl.Transaction::class.java.getMethod("setLayerStack", SurfaceControl::class.java, Int::class.javaPrimitiveType)
            .invoke(t, root, layerStack)
        t.setLayer(root, Int.MAX_VALUE).setVisibility(root, true).apply()
        Surface(root).also { s ->
            val canvas = s.lockHardwareCanvas()
            canvas.drawColor(Color.BLACK)
            s.unlockCanvasAndPost(canvas)
            s.release()
        }

        link = DisplayLink(fd, onMessage = ::message, onClosed = { reason ->
            if (reason.isNotEmpty()) Log.i(TAG, "display closed: $reason")
            restorePanels()
            exitProcess(0)
        })
        link.start()
        pointerX = screenWidth / scale / 2.0
        pointerY = screenHeight / scale / 2.0
        link.send(DisplayLink.HELLO, DisplayLink.VERSION, scale, screenWidth / scale, screenHeight / scale, DisplayLink.CAP_LAYERS)
        grabInput()
        keepAwake(displayId)
        panelsOff(displayId)
        Runtime.getRuntime().addShutdownHook(Thread { restorePanels() })
        syncFence = runCatching {
            Class.forName("android.hardware.SyncFence").getDeclaredMethod("create", ParcelFileDescriptor::class.java)
        }.getOrNull()
        Looper.loop()
    }

    private fun displayInfo(displayId: Int): Any? = runCatching {
        val global = Class.forName("android.hardware.display.DisplayManagerGlobal").getMethod("getInstance").invoke(null)
        global.javaClass.getMethod("getDisplayInfo", Int::class.javaPrimitiveType).invoke(global, displayId)
    }.onFailure { Log.w(TAG, "could not read display $displayId", it) }.getOrNull()

    private fun message(kind: Int, body: ByteArray) {
        if (body.size < 4) return
        val words = ByteBuffer.wrap(body).order(ByteOrder.LITTLE_ENDIAN)
        val id = words.getInt(0)
        when (kind) {
            DisplayLink.LAYER -> {
                if (body.size < 68) return
                val count = words.getInt(60)
                val fds = link.takeFds(count) ?: return
                val metaLength = words.getInt(64)
                val hasFence = words.getInt(56) != 0
                val flight = words.getInt(52)
                if (metaLength <= 0 || 68 + metaLength > body.size || count < 1 + (if (hasFence) 1 else 0)) {
                    fds.forEach { runCatching { ParcelFileDescriptor.adoptFd(it).close() } }
                    link.send(DisplayLink.RELEASE, flight)
                    return
                }
                val fence = if (hasFence) fds.last() else -1
                val handles = if (hasFence) fds.copyOf(fds.size - 1) else fds
                val buffer = Native.buffer(body.copyOfRange(68, 68 + metaLength), handles)
                val layout = IntArray(13) { words.getInt(it * 4) }
                main.post { layer(layout, buffer, fence, flight) }
            }
            DisplayLink.LAYER_MOVE -> {
                if (body.size < 52) return
                val layout = IntArray(13) { words.getInt(it * 4) }
                main.post { move(layout) }
            }
            DisplayLink.LAYER_REMOVE -> {
                if (body.size < 8) return
                val layerId = words.getInt(4)
                main.post { removeLayer(id, layerId) }
            }
            DisplayLink.PIXELS -> {
                if (body.size < 20) return
                val width = words.getInt(4)
                val height = words.getInt(8)
                val y0 = words.getInt(12)
                val rows = words.getInt(16)
                if (width <= 0 || width > 16384 || height <= 0 || height > 16384 || y0 < 0 || rows <= 0 || y0 + rows > height) return
                val data = runCatching { Lz4.decompressSizePrepended(body, 20, body.size - 20) }.getOrNull() ?: return
                if (data.size != width * rows * 4) return
                val colors = IntArray(width * rows)
                ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN).asIntBuffer().get(colors)
                main.post { pixels(id, width, height, y0, rows, colors) }
            }
            DisplayLink.CURSOR -> main.post {
                cursorVisible = id != 0
                moveCursor()
            }
            DisplayLink.CURSOR_IMAGE -> {
                if (body.size < 20) return
                val width = words.getInt(0)
                val height = words.getInt(4)
                if (width <= 0 || width > 256 || height <= 0 || height > 256) return
                val data = runCatching { Lz4.decompressSizePrepended(body, 20, body.size - 20) }.getOrNull() ?: return
                if (data.size != width * height * 4) return
                val colors = IntArray(width * height)
                ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN).asIntBuffer().get(colors)
                val hotspotX = words.getInt(8)
                val hotspotY = words.getInt(12)
                main.post { cursorImage(width, height, hotspotX, hotspotY, colors) }
            }
            DisplayLink.OPEN -> main.post { open(id) }
            DisplayLink.CLOSE -> main.post { close(id) }
            DisplayLink.SIZE -> {
                if (body.size < 12) return
                val width = words.getInt(4)
                val height = words.getInt(8)
                main.post {
                    val window = windows[id] ?: return@post
                    window.logicalWidth = width
                    window.logicalHeight = height
                    place(window)
                }
            }
        }
    }

    private var syncFence: java.lang.reflect.Method? = null
    private var cursor: SurfaceControl? = null
    private var cursorSurface: Surface? = null
    private var cursorVisible = false
    private var cursorHotspotX = 0
    private var cursorHotspotY = 0

    private fun cursorImage(width: Int, height: Int, hotspotX: Int, hotspotY: Int, colors: IntArray) {
        val control = cursor ?: SurfaceControl.Builder().setName("bareboot-cursor").setParent(root).build().also {
            cursor = it
            SurfaceControl.Transaction().setLayer(it, Int.MAX_VALUE).apply()
        }
        cursorSurface?.release()
        SurfaceControl.Transaction().setBufferSize(control, width, height).apply()
        val surface = Surface(control)
        cursorSurface = surface
        val bitmap = Bitmap.createBitmap(colors, width, height, Bitmap.Config.ARGB_8888)
        val canvas = surface.lockHardwareCanvas()
        canvas.drawColor(Color.TRANSPARENT, android.graphics.PorterDuff.Mode.CLEAR)
        canvas.drawBitmap(bitmap, 0f, 0f, null)
        surface.unlockCanvasAndPost(canvas)
        bitmap.recycle()
        cursorHotspotX = hotspotX
        cursorHotspotY = hotspotY
        moveCursor()
    }

    private fun moveCursor() {
        val control = cursor ?: return
        SurfaceControl.Transaction()
            .setPosition(control, (pointerX * scale - cursorHotspotX).toFloat(), (pointerY * scale - cursorHotspotY).toFloat())
            .setVisibility(control, cursorVisible)
            .apply()
    }

    private fun layer(layout: IntArray, buffer: HardwareBuffer?, fence: Int, flight: Int) {
        val window = windows[layout[0]]
        val fenceObject = if (fence >= 0) {
            val pfd = ParcelFileDescriptor.adoptFd(fence)
            runCatching { syncFence?.invoke(null, pfd) as SyncFence? }.getOrNull() ?: run {
                pfd.close()
                null
            }
        } else {
            null
        }
        if (window == null || buffer == null) {
            buffer?.close()
            fenceObject?.close()
            link.send(DisplayLink.RELEASE, flight)
            return
        }
        val layer = window.layers.getOrPut(layout[1]) {
            Layer(SurfaceControl.Builder().setName("bareboot-layer-${layout[0]}-${layout[1]}").setParent(window.container).build())
        }
        layer.bufferWidth = buffer.width
        layer.bufferHeight = buffer.height
        val t = SurfaceControl.Transaction()
        t.setBuffer(layer.control, buffer, fenceObject) { release ->
            runCatching { release.awaitForever() }
            release.close()
            link.send(DisplayLink.RELEASE, flight)
        }
        geometry(t, layer, layout)
        t.setVisibility(layer.control, true)
        t.setVisibility(window.pixels, false)
        t.apply()
        buffer.close()
        fenceObject?.close()
    }

    private fun geometry(t: SurfaceControl.Transaction, layer: Layer, layout: IntArray) {
        val source = Rect(
            (layout[9].toLong() * layer.bufferWidth / 65536).toInt(),
            (layout[10].toLong() * layer.bufferHeight / 65536).toInt(),
            (layout[11].toLong() * layer.bufferWidth / 65536).toInt(),
            (layout[12].toLong() * layer.bufferHeight / 65536).toInt(),
        )
        val destination = Rect(layout[2], layout[3], layout[2] + layout[4], layout[3] + layout[5])
        if (source.isEmpty || destination.isEmpty) return
        t.setGeometry(layer.control, source, destination, Surface.ROTATION_0)
        t.setLayer(layer.control, layout[8])
    }

    private fun move(layout: IntArray) {
        val layer = windows[layout[0]]?.layers?.get(layout[1]) ?: return
        if (layer.bufferWidth == 0) return
        val t = SurfaceControl.Transaction()
        geometry(t, layer, layout)
        t.apply()
    }

    private fun removeLayer(id: Int, layerId: Int) {
        val layer = windows[id]?.layers?.remove(layerId) ?: return
        SurfaceControl.Transaction().reparent(layer.control, null).apply()
        layer.control.release()
    }

    private fun open(id: Int) {
        if (windows.containsKey(id)) return
        val window = Window(id)
        window.logicalWidth = screenWidth / scale
        window.logicalHeight = screenHeight / scale
        windows[id] = window
        SurfaceControl.Transaction().setVisibility(window.container, true).apply()
        place(window)
        link.send(DisplayLink.CONFIGURE, id, screenWidth / scale, screenHeight / scale)
        focus(id)
    }

    private fun close(id: Int) {
        val window = windows.remove(id) ?: return
        SurfaceControl.Transaction().reparent(window.container, null).apply()
        window.layers.values.forEach { it.control.release() }
        window.pixels.release()
        window.container.release()
        window.pixelSurface?.release()
        window.bitmap?.recycle()
        if (grab == id) grab = 0
        if (hover == id) hover = 0
        if (focused == id) {
            focused = 0
            windows.keys.lastOrNull()?.let(::focus)
        }
    }

    private fun place(window: Window) {
        val width = (window.logicalWidth * scale).coerceIn(1, screenWidth)
        val height = (window.logicalHeight * scale).coerceIn(1, screenHeight)
        window.x = ((screenWidth - width) / 2).coerceAtLeast(0)
        window.y = ((screenHeight - height) / 2).coerceAtLeast(0)
        SurfaceControl.Transaction().setPosition(window.container, window.x.toFloat(), window.y.toFloat()).apply()
    }

    private fun pixels(id: Int, width: Int, height: Int, y0: Int, rows: Int, colors: IntArray) {
        val window = windows[id] ?: return
        var bitmap = window.bitmap
        if (bitmap == null || bitmap.width != width || bitmap.height != height) {
            if (y0 != 0 || rows != height) return
            bitmap?.recycle()
            bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            window.bitmap = bitmap
            window.pixelSurface?.release()
            SurfaceControl.Transaction().setBufferSize(window.pixels, width, height).apply()
            window.pixelSurface = Surface(window.pixels)
        }
        bitmap.setPixels(colors, 0, width, 0, y0, width, rows)
        val surface = window.pixelSurface ?: return
        val canvas = surface.lockHardwareCanvas()
        canvas.drawBitmap(bitmap, null, Rect(0, 0, width, height), null)
        surface.unlockCanvasAndPost(canvas)
        val t = SurfaceControl.Transaction().setVisibility(window.pixels, true)
        window.layers.values.forEach { t.setVisibility(it.control, false) }
        t.apply()
    }

    private fun focus(id: Int) {
        if (!windows.containsKey(id)) return
        val window = windows.remove(id)!!
        windows[id] = window
        var z = 1
        val t = SurfaceControl.Transaction()
        windows.values.forEach { t.setLayer(it.container, z++) }
        t.apply()
        if (focused != id) {
            focused = id
            link.send(DisplayLink.FOCUS, id)
        }
    }

    private fun windowAt(x: Double, y: Double): Window? =
        windows.values.reversed().firstOrNull {
            val left = it.x / scale.toDouble()
            val top = it.y / scale.toDouble()
            x >= left && y >= top && x < left + it.logicalWidth && y < top + it.logicalHeight
        }

    private fun pointerMotion() {
        val target = windows[grab] ?: windowAt(pointerX, pointerY)
        if (target == null) {
            if (hover != 0) link.send(DisplayLink.LEAVE, hover)
            hover = 0
            return
        }
        if (hover != 0 && hover != target.id) link.send(DisplayLink.LEAVE, hover)
        hover = target.id
        val lx = (pointerX - target.x / scale.toDouble()).toFloat()
        val ly = (pointerY - target.y / scale.toDouble()).toFloat()
        link.send(DisplayLink.MOTION, target.id, lx.toRawBits(), ly.toRawBits())
    }

    private fun button(code: Int, pressed: Boolean) {
        val target = windows[grab] ?: windowAt(pointerX, pointerY) ?: return
        if (pressed && grab == 0) {
            grab = target.id
            focus(target.id)
        }
        link.send(DisplayLink.BUTTON, target.id, code, if (pressed) 1 else 0)
        if (!pressed && held.none { it in 272..276 }) grab = 0
    }

    private fun grabInput() {
        val events = File("/dev/input").listFiles { f -> f.name.startsWith("event") }?.sortedBy { it.name } ?: return
        val ioctlInt = runCatching {
            Class.forName("android.system.Os").getMethod("ioctlInt", java.io.FileDescriptor::class.java, Int::class.javaPrimitiveType)
        }.getOrNull()
        for (event in events) {
            val sys = File("/sys/class/input/${event.name}/device")
            val bus = runCatching { File(sys, "id/bustype").readText().trim() }.getOrNull() ?: continue
            if (bus != "0003") continue
            val keys = runCatching { File(sys, "capabilities/key").readText().trim() }.getOrDefault("0")
            val rel = runCatching { File(sys, "capabilities/rel").readText().trim() }.getOrDefault("0")
            if (keys == "0" && rel == "0") continue
            val input = runCatching { FileInputStream(event) }.getOrNull() ?: continue
            val grabbed = runCatching { ioctlInt?.invoke(null, input.fd, EVIOCGRAB) }.isSuccess && ioctlInt != null
            Log.i(TAG, "input ${event.name} ${File(sys, "name").readText().trim()} grabbed=$grabbed")
            thread(name = "input-${event.name}", isDaemon = true) { readInput(input) }
        }
    }

    private fun readInput(input: FileInputStream) {
        val record = ByteArray(24)
        val buffer = ByteBuffer.wrap(record).order(ByteOrder.LITTLE_ENDIAN)
        var dx = 0
        var dy = 0
        try {
            while (true) {
                var read = 0
                while (read < 24) {
                    val n = input.read(record, read, 24 - read)
                    if (n < 0) return
                    read += n
                }
                val type = buffer.getShort(16).toInt() and 0xFFFF
                val code = buffer.getShort(18).toInt() and 0xFFFF
                val value = buffer.getInt(20)
                when (type) {
                    EV_REL -> when (code) {
                        REL_X -> dx += value
                        REL_Y -> dy += value
                        REL_WHEEL -> main.post { scroll(0, -value * 15f) }
                        REL_HWHEEL -> main.post { scroll(1, value * 15f) }
                    }
                    EV_KEY -> if (value != 2) {
                        val pressed = value == 1
                        main.post { key(code, pressed) }
                    }
                    EV_SYN -> if (dx != 0 || dy != 0) {
                        val mx = dx
                        val my = dy
                        dx = 0
                        dy = 0
                        main.post {
                            pointerX = (pointerX + mx).coerceIn(0.0, screenWidth / scale - 1.0)
                            pointerY = (pointerY + my).coerceIn(0.0, screenHeight / scale - 1.0)
                            pointerMotion()
                            moveCursor()
                        }
                    }
                }
            }
        } catch (_: Exception) {
        }
    }

    private fun scroll(axis: Int, value: Float) {
        val target = windows[grab] ?: windowAt(pointerX, pointerY) ?: return
        link.send(DisplayLink.AXIS, target.id, axis, value.toRawBits())
    }

    private fun key(code: Int, pressed: Boolean) {
        if (pressed) held.add(code) else held.remove(code)
        if (code in 272..276) {
            button(code, pressed)
            return
        }
        if (pressed && code == KEY_BACKSPACE &&
            (KEY_LEFTCTRL in held || KEY_RIGHTCTRL in held) && (KEY_LEFTALT in held || KEY_RIGHTALT in held)
        ) {
            link.close()
            restorePanels()
            exitProcess(0)
        }
        link.send(DisplayLink.KEY, code, if (pressed) 1 else 0)
    }

    private fun keepAwake(displayId: Int) {
        val power = runCatching {
            val binder = Class.forName("android.os.ServiceManager").getMethod("getService", String::class.java).invoke(null, "power") as IBinder
            Class.forName("android.os.IPowerManager\$Stub").getMethod("asInterface", IBinder::class.java).invoke(null, binder)
        }.getOrNull()
        val method = power?.javaClass?.methods?.firstOrNull { it.name == "userActivity" && it.parameterTypes.size == 4 }
        val tick = object : Runnable {
            override fun run() {
                if (displayInfo(displayId) == null) {
                    Log.i(TAG, "display $displayId disconnected")
                    link.close()
                    restorePanels()
                    exitProcess(0)
                }
                for (id in intArrayOf(0, displayId)) {
                    runCatching { method?.invoke(power, id, SystemClock.uptimeMillis(), 0, 0) }
                }
                main.postDelayed(this, 10_000)
            }
        }
        main.post(tick)
    }

    private val panels = ArrayList<IBinder>()
    @Volatile private var panelsRestored = false

    @SuppressLint("BlockedPrivateApi")
    private fun panelsOff(displayId: Int) {
        runCatching {
            val info = Class.forName("android.hardware.display.DisplayManagerGlobal").getMethod("getInstance").invoke(null)
                .let { it.javaClass.getMethod("getDisplayInfo", Int::class.javaPrimitiveType).invoke(it, displayId) }
            val address = info.javaClass.getField("address").get(info)
            val external = address.javaClass.getMethod("getPhysicalDisplayId").invoke(address) as Long
            val factory = Class.forName("com.android.internal.os.ClassLoaderFactory")
            val create = factory.getDeclaredMethod(
                "createClassLoader", String::class.java, String::class.java, String::class.java, ClassLoader::class.java,
                Int::class.javaPrimitiveType, Boolean::class.javaPrimitiveType, String::class.java,
            )
            val loader = create.invoke(
                null, "/system/framework/services.jar", null, null, ClassLoader.getSystemClassLoader(), 0, true, null,
            ) as ClassLoader
            val control = loader.loadClass("com.android.server.display.DisplayControl")
            Runtime::class.java.getDeclaredMethod("loadLibrary0", Class::class.java, String::class.java)
                .apply { isAccessible = true }
                .invoke(Runtime.getRuntime(), control, "android_servers")
            val ids = control.getMethod("getPhysicalDisplayIds").invoke(null) as LongArray
            val token = control.getMethod("getPhysicalDisplayToken", Long::class.javaPrimitiveType)
            val setMode = SurfaceControl::class.java.getMethod("setDisplayPowerMode", IBinder::class.java, Int::class.javaPrimitiveType)
            for (id in ids) {
                if (id == external) continue
                val binder = token.invoke(null, id) as? IBinder ?: continue
                setMode.invoke(null, binder, 0)
                panels.add(binder)
            }
            Log.i(TAG, "phone panels off: ${panels.size}")
        }.onFailure { Log.w(TAG, "could not turn off the phone panel", it) }
    }

    private fun restorePanels() {
        if (panelsRestored) return
        panelsRestored = true
        val setMode = runCatching {
            SurfaceControl::class.java.getMethod("setDisplayPowerMode", IBinder::class.java, Int::class.javaPrimitiveType)
        }.getOrNull() ?: return
        panels.forEach { runCatching { setMode.invoke(null, it, 2) } }
    }
}

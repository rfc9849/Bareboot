package ch.ausfuhrli.bareboot.screen

import android.hardware.HardwareBuffer

object Native {
    @JvmStatic external fun recv(fd: Int, buffer: ByteArray, fds: IntArray): Int

    @JvmStatic external fun buffer(metadata: ByteArray, fds: IntArray): HardwareBuffer?
}

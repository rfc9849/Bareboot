package ch.ausfuhrli.bareboot.ipc

import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

data class DistroRelease(
    val release: String,
    val label: String,
    val path: String,
    val sha256: String,
    val size: Long,
)

data class Distro(
    val id: String,
    val name: String,
    val defaultUser: String,
    val releases: List<DistroRelease>,
) {
    val defaultRelease: DistroRelease get() = releases.first()

    fun osLabel(release: DistroRelease): String =
        if (release.label.isEmpty()) name else "$name ${release.label}"
}

object DistroCatalog {
    private const val BASE_URL = "https://images.linuxcontainers.org/"
    private const val INDEX_URL = "${BASE_URL}streams/v1/images.json"

    private class Spec(val os: String, val name: String, val user: String, val preferred: String?)

    private val specs = listOf(
        Spec("debian", "Debian", "debian", "trixie"),
        Spec("ubuntu", "Ubuntu", "ubuntu", "resolute"),
        Spec("alpine", "Alpine Linux", "alpine", null),
        Spec("archlinux", "Arch Linux", "arch", null),
        Spec("fedora", "Fedora", "fedora", null),
        Spec("opensuse", "openSUSE", "opensuse", "tumbleweed"),
        Spec("kali", "Kali Linux", "kali", null),
        Spec("voidlinux", "Void Linux", "void", null),
    )

    fun load(): List<Distro> {
        val products = JSONObject(fetchText(INDEX_URL)).getJSONObject("products")
        val byOs = mutableMapOf<String, MutableList<Pair<DistroRelease, String?>>>()
        for (key in products.keys()) {
            val parts = key.split(':')
            if (parts.size != 4 || parts[2] != "arm64" || parts[3] != "default") continue
            val os = parts[0]
            if (specs.none { it.os == os }) continue
            val product = products.getJSONObject(key)
            val versions = product.optJSONObject("versions") ?: continue
            val latest = versions.keys().asSequence().maxOrNull() ?: continue
            val item = versions.getJSONObject(latest).getJSONObject("items").optJSONObject("root.tar.xz") ?: continue
            val release = parts[1]
            val number = product.optString("aliases").split(',')
                .map { it.split('/') }
                .firstOrNull { it.size == 2 && it[0] == os && it[1] != release && it[1].firstOrNull()?.isDigit() == true }
                ?.get(1)
            byOs.getOrPut(os) { mutableListOf() } += DistroRelease(
                release = release,
                label = releaseLabel(release, number),
                path = item.getString("path"),
                sha256 = item.getString("sha256"),
                size = item.getLong("size"),
            ) to number
        }
        return specs.mapNotNull { spec ->
            val found = byOs[spec.os] ?: return@mapNotNull null
            val sorted = found.sortedWith(compareByDescending<Pair<DistroRelease, String?>> {
                versionKey(it.second ?: it.first.release)
            }.thenBy { it.first.release }).map { it.first }
            val preferred = sorted.firstOrNull { it.release == spec.preferred }
                ?: sorted.firstOrNull { versionKey(it.release) != null }
                ?: sorted.first()
            Distro(spec.os, spec.name, spec.user, listOf(preferred) + (sorted - preferred))
        }
    }

    fun download(
        release: DistroRelease,
        destFile: File,
        cancelled: () -> Boolean,
        onProgress: (Long, Long) -> Unit,
    ) {
        val conn = openRedirected(BASE_URL + release.path)
        val tmp = File(destFile.parentFile, "${destFile.name}.part")
        try {
            destFile.parentFile?.mkdirs()
            val digest = MessageDigest.getInstance("SHA-256")
            var readTotal = 0L
            conn.inputStream.use { inp ->
                FileOutputStream(tmp).use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        if (cancelled()) throw ch.ausfuhrli.bareboot.install.InstallCancelledException()
                        val n = inp.read(buf)
                        if (n <= 0) break
                        out.write(buf, 0, n)
                        digest.update(buf, 0, n)
                        readTotal += n
                        onProgress(readTotal, release.size)
                    }
                }
            }
            val sha = digest.digest().joinToString("") { "%02x".format(it) }
            if (readTotal != release.size || !sha.equals(release.sha256, ignoreCase = true)) {
                throw IOException("the download does not match the published checksum")
            }
            if (destFile.exists()) destFile.delete()
            if (!tmp.renameTo(destFile)) throw IOException("could not save the download")
            destFile.setReadable(true, false)
        } finally {
            tmp.delete()
            conn.disconnect()
        }
    }

    private fun releaseLabel(release: String, number: String?): String = when {
        number != null -> "$number ($release)"
        release == "current" -> ""
        else -> release.replaceFirstChar { it.uppercase() }
    }

    private fun versionKey(value: String): Long? {
        val parts = value.split('.')
        if (parts.isEmpty() || parts.size > 3) return null
        var key = 0L
        for (i in 0 until 3) {
            val part = parts.getOrNull(i) ?: "0"
            key = key * 10_000 + (part.toLongOrNull() ?: return null)
        }
        return key
    }

    private fun fetchText(url: String): String {
        val conn = openRedirected(url)
        try {
            return conn.inputStream.bufferedReader().use { it.readText() }
        } finally {
            conn.disconnect()
        }
    }

    private fun openRedirected(urlStr: String): HttpURLConnection {
        var current = urlStr
        repeat(5) {
            val conn = (URL(current).openConnection() as HttpURLConnection).apply {
                instanceFollowRedirects = false
                connectTimeout = 15_000
                readTimeout = 60_000
            }
            val code = conn.responseCode
            if (code in 301..308) {
                val loc = conn.getHeaderField("Location") ?: throw IOException("empty redirect")
                conn.disconnect()
                current = URL(URL(current), loc).toString()
            } else if (code in 200..299) {
                return conn
            } else {
                conn.disconnect()
                throw IOException("HTTP $code for $current")
            }
        }
        throw IOException("too many redirects")
    }
}

package ch.ausfuhrli.bareboot.install

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.IBinder
import android.os.Process
import android.os.StatFs
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.text.format.Formatter
import ch.ausfuhrli.bareboot.InstallProgress
import ch.ausfuhrli.bareboot.MainActivity
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.ipc.DistroCatalog
import ch.ausfuhrli.bareboot.ipc.DistroRelease
import ch.ausfuhrli.bareboot.ipc.BarebootClient
import ch.ausfuhrli.bareboot.userMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

sealed interface InstallEvent {
    data class Finished(val name: String) : InstallEvent
    data class Failed(val message: String) : InstallEvent
}

object InstallTracker {
    val progress = MutableStateFlow<InstallProgress?>(null)
    val events = MutableSharedFlow<InstallEvent>(extraBufferCapacity = 8)

    @Volatile
    var cancelRequested = false
}

class InstallCancelledException : IOException("the install was cancelled")

class InstallService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var lastNotified = 0L

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_CANCEL -> InstallTracker.cancelRequested = true
            ACTION_INSTALL -> if (InstallTracker.progress.value == null) begin(intent) else stopIfIdle()
            else -> stopIfIdle()
        }
        return START_NOT_STICKY
    }

    private fun stopIfIdle() {
        if (InstallTracker.progress.value == null) stopSelf()
    }

    private fun begin(intent: Intent) {
        val name = intent.getStringExtra(EXTRA_NAME) ?: return stopIfIdle()
        val username = intent.getStringExtra(EXTRA_USERNAME) ?: return stopIfIdle()
        val osLabel = intent.getStringExtra(EXTRA_OS_LABEL) ?: return stopIfIdle()
        val sizeGb = intent.getIntExtra(EXTRA_SIZE_GB, 16)
        val importUri = intent.getStringExtra(EXTRA_IMPORT_URI)?.let(Uri::parse)
        val release = intent.getStringExtra(EXTRA_PATH)?.let {
            DistroRelease(
                release = intent.getStringExtra(EXTRA_RELEASE).orEmpty(),
                label = "",
                path = it,
                sha256 = intent.getStringExtra(EXTRA_SHA256).orEmpty(),
                size = intent.getLongExtra(EXTRA_SIZE, 0),
            )
        }
        val distroId = intent.getStringExtra(EXTRA_DISTRO_ID).orEmpty()
        if (release == null && importUri == null) return stopIfIdle()

        InstallTracker.cancelRequested = false
        val initial = InstallProgress(osLabel, 0, release?.size ?: 0, unpacking = false)
        InstallTracker.progress.value = initial
        ensureChannel(this)
        startForeground(NOTIFICATION_ID, notification(initial), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        scope.launch {
            val event = try {
                if (release != null) {
                    installRelease(distroId, release, name, username, osLabel, sizeGb)
                } else {
                    installImport(importUri!!, name, username, osLabel, sizeGb)
                }
                InstallEvent.Finished(name)
            } catch (e: InstallCancelledException) {
                InstallEvent.Failed(getString(R.string.install_cancelled))
            } catch (e: Exception) {
                InstallEvent.Failed(getString(R.string.install_failed, e.userMessage(this@InstallService)))
            }
            InstallTracker.progress.value = null
            InstallTracker.events.tryEmit(event)
            stopForeground(STOP_FOREGROUND_REMOVE)
            if (event is InstallEvent.Failed) postResult(event.message)
            stopSelf()
        }
    }

    private fun installRelease(
        distroId: String,
        release: DistroRelease,
        name: String,
        username: String,
        osLabel: String,
        sizeGb: Int,
    ) {
        requireSpace(release.size + estimateUnpacked(release.size, ".tar.xz"))
        val tarFile = File(cacheDir, "rootfs-$distroId-${release.release}.tar.xz")
        try {
            DistroCatalog.download(release, tarFile, { InstallTracker.cancelRequested }) { got, total ->
                publish(InstallProgress(osLabel, got, total, unpacking = false))
            }
            publish(InstallProgress(osLabel, release.size, release.size, unpacking = true))
            BarebootClient().createDistro(name, sizeGb, tarFile.absolutePath, username, osLabel)
        } finally {
            tarFile.delete()
        }
    }

    private fun installImport(uri: Uri, name: String, username: String, osLabel: String, sizeGb: Int) {
        val displayName = displayName(uri)
        val suffix = archiveSuffix(displayName)
            ?: throw IOException(getString(R.string.install_err_archive_type))
        val direct = directPath(uri)?.takeIf { archiveSuffix(it) == suffix && File(it).length() > 0 }
        val size = fileSize(uri)
        if (direct != null) {
            requireSpace(estimateUnpacked(size, suffix))
            publish(InstallProgress(osLabel, size, size, unpacking = true))
            BarebootClient().createDistro(name, sizeGb, direct, username, osLabel)
            return
        }
        requireSpace(size + estimateUnpacked(size, suffix))
        val copy = File(cacheDir, "import-$name$suffix")
        try {
            contentResolver.openInputStream(uri)?.use { input ->
                FileOutputStream(copy).use { output ->
                    val buffer = ByteArray(256 * 1024)
                    var total = 0L
                    while (true) {
                        if (InstallTracker.cancelRequested) throw InstallCancelledException()
                        val n = input.read(buffer)
                        if (n <= 0) break
                        output.write(buffer, 0, n)
                        total += n
                        publish(InstallProgress(osLabel, total, size, unpacking = false))
                    }
                }
            } ?: throw IOException(getString(R.string.install_err_open_file, displayName))
            copy.setReadable(true, false)
            publish(InstallProgress(osLabel, size, size, unpacking = true))
            BarebootClient().createDistro(name, sizeGb, copy.absolutePath, username, osLabel)
        } finally {
            copy.delete()
        }
    }

    private fun requireSpace(needed: Long) {
        val available = StatFs(filesDir.path).availableBytes
        if (available < needed) {
            throw IOException(
                getString(
                    R.string.install_err_no_space,
                    Formatter.formatShortFileSize(this, needed),
                    Formatter.formatShortFileSize(this, available),
                ),
            )
        }
    }

    private fun publish(progress: InstallProgress) {
        InstallTracker.progress.value = progress
        val now = System.currentTimeMillis()
        if (progress.unpacking || now - lastNotified > 500) {
            lastNotified = now
            getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(progress))
        }
    }

    private fun notification(progress: InstallProgress): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = Notification.Builder(this, CHANNEL_INSTALL)
            .setSmallIcon(R.drawable.ic_download)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
        if (progress.unpacking) {
            builder.setContentTitle(getString(R.string.install_setting_up, progress.osLabel))
                .setContentText(getString(R.string.install_unpacking_desc))
                .setProgress(0, 0, true)
        } else {
            val cancel = PendingIntent.getService(
                this, 1, Intent(this, InstallService::class.java).setAction(ACTION_CANCEL), PendingIntent.FLAG_IMMUTABLE,
            )
            val total = progress.totalBytes
            builder.setContentTitle(getString(R.string.install_downloading, progress.osLabel))
                .setContentText(
                    if (total > 0) {
                        getString(
                            R.string.install_bytes_of,
                            Formatter.formatShortFileSize(this, progress.downloadedBytes),
                            Formatter.formatShortFileSize(this, total),
                        )
                    } else {
                        Formatter.formatShortFileSize(this, progress.downloadedBytes)
                    },
                )
                .setProgress(1000, if (total > 0) (progress.downloadedBytes * 1000 / total).toInt() else 0, total <= 0)
                .addAction(Notification.Action.Builder(null, getString(R.string.action_cancel), cancel).build())
        }
        return builder.build()
    }

    private fun postResult(message: String) {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(this, CHANNEL_INSTALL)
            .setSmallIcon(R.drawable.ic_error_outline)
            .setContentTitle(message)
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        getSystemService(NotificationManager::class.java).notify(RESULT_NOTIFICATION_ID, n)
    }

    private fun displayName(uri: Uri): String =
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        } ?: uri.lastPathSegment.orEmpty()

    private fun fileSize(uri: Uri): Long =
        contentResolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else 0L
        } ?: 0L

    private fun directPath(uri: Uri): String? {
        if (!DocumentsContract.isDocumentUri(this, uri)) return null
        val id = DocumentsContract.getDocumentId(uri)
        val user = Process.myUid() / 100_000
        return when (uri.authority) {
            "com.android.externalstorage.documents" ->
                id.removePrefix("primary:").takeIf { id.startsWith("primary:") }?.let { "/data/media/$user/$it" }
            "com.android.providers.downloads.documents" ->
                id.removePrefix("raw:").takeIf { id.startsWith("raw:/storage/emulated/$user/") }
                    ?.replaceFirst("/storage/emulated/", "/data/media/")
            else -> null
        }?.takeIf { !it.contains("/../") }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        const val ACTION_INSTALL = "ch.ausfuhrli.bareboot.action.INSTALL"
        const val ACTION_CANCEL = "ch.ausfuhrli.bareboot.action.CANCEL_INSTALL"
        private const val EXTRA_NAME = "name"
        private const val EXTRA_USERNAME = "username"
        private const val EXTRA_OS_LABEL = "os_label"
        private const val EXTRA_SIZE_GB = "size_gb"
        private const val EXTRA_DISTRO_ID = "distro_id"
        private const val EXTRA_RELEASE = "release"
        private const val EXTRA_PATH = "path"
        private const val EXTRA_SHA256 = "sha256"
        private const val EXTRA_SIZE = "size"
        private const val EXTRA_IMPORT_URI = "import_uri"
        const val CHANNEL_INSTALL = "install"
        private const val NOTIFICATION_ID = 10
        private const val RESULT_NOTIFICATION_ID = 11

        fun ensureChannel(context: Context) {
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL_INSTALL, context.getString(R.string.install_channel_name), NotificationManager.IMPORTANCE_LOW),
            )
        }

        fun installRelease(
            context: Context,
            distroId: String,
            release: DistroRelease,
            name: String,
            username: String,
            osLabel: String,
            sizeGb: Int,
        ) {
            context.startForegroundService(
                base(context, name, username, osLabel, sizeGb)
                    .putExtra(EXTRA_DISTRO_ID, distroId)
                    .putExtra(EXTRA_RELEASE, release.release)
                    .putExtra(EXTRA_PATH, release.path)
                    .putExtra(EXTRA_SHA256, release.sha256)
                    .putExtra(EXTRA_SIZE, release.size),
            )
        }

        fun importArchive(context: Context, uri: Uri, name: String, username: String, osLabel: String, sizeGb: Int) {
            context.startForegroundService(base(context, name, username, osLabel, sizeGb).putExtra(EXTRA_IMPORT_URI, uri.toString()))
        }

        private fun base(context: Context, name: String, username: String, osLabel: String, sizeGb: Int) =
            Intent(context, InstallService::class.java)
                .setAction(ACTION_INSTALL)
                .putExtra(EXTRA_NAME, name)
                .putExtra(EXTRA_USERNAME, username)
                .putExtra(EXTRA_OS_LABEL, osLabel)
                .putExtra(EXTRA_SIZE_GB, sizeGb)

        fun archiveSuffix(name: String): String? {
            val lower = name.lowercase()
            return when {
                lower.endsWith(".tar.xz") || lower.endsWith(".txz") -> ".tar.xz"
                lower.endsWith(".tar.gz") || lower.endsWith(".tgz") -> ".tar.gz"
                lower.endsWith(".tar") -> ".tar"
                else -> null
            }
        }

        private fun estimateUnpacked(archiveSize: Long, suffix: String): Long {
            val ratio = when (suffix) {
                ".tar.xz" -> 6
                ".tar.gz" -> 4
                else -> 1
            }
            return archiveSize * ratio + 256L * 1024 * 1024
        }
    }
}

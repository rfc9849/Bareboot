package ch.ausfuhrli.bareboot

import android.app.Application
import android.content.Context
import android.graphics.Typeface
import android.net.Uri
import android.system.OsConstants
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import ch.ausfuhrli.bareboot.files.SharedFolderAccess
import ch.ausfuhrli.bareboot.ipc.DaemonStatus
import ch.ausfuhrli.bareboot.ipc.DaemonUnavailableException
import ch.ausfuhrli.bareboot.ipc.Distro
import ch.ausfuhrli.bareboot.ipc.DistroCatalog
import ch.ausfuhrli.bareboot.ipc.DistroRelease
import ch.ausfuhrli.bareboot.ipc.PROTOCOL_VERSION
import ch.ausfuhrli.bareboot.ipc.ImageInfo
import ch.ausfuhrli.bareboot.ipc.KernelReport
import ch.ausfuhrli.bareboot.ipc.BarebootClient
import ch.ausfuhrli.bareboot.ipc.BarebootRpcException
import ch.ausfuhrli.bareboot.ipc.PortForward
import ch.ausfuhrli.bareboot.ipc.SessionInfo
import ch.ausfuhrli.bareboot.install.InstallEvent
import ch.ausfuhrli.bareboot.install.InstallService
import ch.ausfuhrli.bareboot.install.InstallTracker
import ch.ausfuhrli.bareboot.ipc.INIT_SYSTEM
import ch.ausfuhrli.bareboot.notify.LauncherShortcuts
import ch.ausfuhrli.bareboot.notify.RunningNotifier
import ch.ausfuhrli.bareboot.ssh.SshConfig
import ch.ausfuhrli.bareboot.ssh.SshSetup
import ch.ausfuhrli.bareboot.terminal.TabConnection
import ch.ausfuhrli.bareboot.terminal.TerminalTab
import io.github.sagernet.libghostty.GhosttyTerminalSession
import ch.ausfuhrli.bareboot.ui.TerminalTheme
import ch.ausfuhrli.bareboot.ui.TerminalThemes
import io.github.sagernet.libghostty.extras.GhosttyFontStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.net.SocketTimeoutException
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

enum class Screen { Onboarding, Home, Terminal, Settings, SystemCheck, Diagnostics }

sealed interface ServiceState {
    data object Checking : ServiceState
    data object NotRunning : ServiceState
    data object Unauthorized : ServiceState
    data class Ready(val version: Int) : ServiceState
    data class Failed(val message: String) : ServiceState
}

data class InstallProgress(
    val osLabel: String,
    val downloadedBytes: Long,
    val totalBytes: Long,
    val unpacking: Boolean,
)

class BarebootViewModel(app: Application) : AndroidViewModel(app) {
    private val client = BarebootClient()
    private val store = SettingsStore(app)
    private val folders = SharedFolderAccess(app)
    private val ipcLock = Mutex()
    private val bundledTypeface: Typeface? = runCatching {
        Typeface.createFromAsset(app.assets, "fonts/GoogleSansCode.ttf")
    }.getOrNull()

    val messages = Channel<String>(Channel.BUFFERED)

    var screen by mutableStateOf(if (store.onboardingDone) Screen.Home else Screen.Onboarding)
    private var settingsOrigin = Screen.Home
    var stopTarget by mutableStateOf<String?>(null)
        private set
    var service by mutableStateOf<ServiceState>(ServiceState.Checking)
        private set
    var status by mutableStateOf<DaemonStatus?>(null)
        private set
    var images by mutableStateOf<List<ImageInfo>>(emptyList())
        private set
    var kernelReport by mutableStateOf<KernelReport?>(null)
        private set
    var kernelReportError by mutableStateOf<String?>(null)
        private set
    var busy by mutableStateOf(false)
        private set
    var install by mutableStateOf<InstallProgress?>(null)
        private set
    var pendingJobs by mutableStateOf<Set<String>>(emptySet())
        private set
    var catalog by mutableStateOf<List<Distro>?>(null)
        private set
    var catalogError by mutableStateOf<String?>(null)
        private set
    var filesConnected by mutableStateOf(false)
        private set

    var distroName by mutableStateOf("")
        private set
    var imagePath by mutableStateOf("")
        private set
    var sharedDir by mutableStateOf("/data/media/0/Linux")
        private set
    var shareEnabled by mutableStateOf(true)
        private set
    var autoStart by mutableStateOf(true)
        private set
    var themeId by mutableStateOf("onedark")
        private set
    var useBundledFont by mutableStateOf(true)
        private set
    var fontSizeSp by mutableFloatStateOf(14f)
        private set

    var extraKeys by mutableStateOf(store.extraKeys)
        private set
    var extraChars by mutableStateOf(store.extraChars)
        private set
    val tabs = mutableStateListOf<TerminalTab>()
    val tabNames = mutableStateMapOf<Int, String>()
    var activeTabId by mutableIntStateOf(-1)
        private set

    private var themeCache: TerminalTheme? = null
    val currentTheme: TerminalTheme
        get() = themeCache?.takeIf { it.id == themeId }
            ?: TerminalThemes.byId(getApplication(), themeId).also { themeCache = it }

    fun ghosttyThemeNames(): List<String> = TerminalThemes.ghosttyThemeNames(getApplication())

    fun previewTheme(name: String): TerminalTheme =
        TerminalThemes.byId(getApplication(), TerminalThemes.GHOSTTY_PREFIX + name)
    var importedFont by mutableStateOf(store.importedFont)
        private set
    val selectedImage: ImageInfo? get() = images.firstOrNull { it.name == distroName }
    val distroRunning: Boolean get() = isRunning(distroName)
    val pidNamespaces: Boolean get() = status?.kernel?.hasPidNs == true
    val isolatedNetworking: Boolean get() = status?.kernel?.isolatedNetwork == true

    fun isRunning(name: String): Boolean = status?.distros?.any { it.name == name } == true

    fun image(name: String): ImageInfo? = images.firstOrNull { it.name == name }
    val sharedFolderName: String? get() = folders.displayName(sharedDir)

    init {
        val loaded = store.load()
        distroName = loaded.distroName
        imagePath = loaded.imagePath
        sharedDir = loaded.sharedDir
        shareEnabled = loaded.shareEnabled
        autoStart = loaded.autoStart
        fontSizeSp = loaded.fontSizeSp
        themeId = loaded.themeId
        useBundledFont = loaded.useBundledFont
        filesConnected = folders.activeTree() != null
        viewModelScope.launch {
            InstallTracker.progress.collect { install = it }
        }
        viewModelScope.launch {
            InstallTracker.events.collect { event ->
                when (event) {
                    is InstallEvent.Finished -> {
                        refresh()
                        images.firstOrNull { it.name == event.name }?.let(::selectImage)
                        if (screen != Screen.Onboarding) messages.trySend(str(R.string.msg_image_ready, event.name))
                    }
                    is InstallEvent.Failed -> messages.trySend(event.message)
                }
            }
        }
        if (store.onboardingDone) viewModelScope.launch { startup() }
    }

    private suspend fun startup() {
        refresh()
        if (service !is ServiceState.Ready) return
        if (autoStart && selectedImage != null && !distroRunning) {
            runAction { startSelected() }
        }
        if (tabs.isNotEmpty()) {
            screen = Screen.Terminal
        } else if (autoStart && distroRunning) {
            runAction {
                newTab(distroName, root = false)
                screen = Screen.Terminal
            }
        }
    }

    suspend fun refresh() {
        try {
            ipcLock.withLock {
                val pong = io { client.ping() }
                if (pong.version != PROTOCOL_VERSION) {
                    service = ServiceState.Failed(
                        str(
                            R.string.msg_update_module,
                            RootManager.name(getApplication(), RootManager.detect(getApplication())),
                        ),
                    )
                    status = null
                    return@withLock
                }
                val st = io { client.status() }
                val imgs = io { client.listImages() }
                val sessions = io { client.listTabs() }
                service = ServiceState.Ready(pong.version)
                status = st
                images = imgs
                syncSelection()
                restoreTabs(sessions)
                RunningNotifier.update(getApplication(), st.distros.map { it.name })
                LauncherShortcuts.update(getApplication(), listOfNotNull(selectedImage?.name) + imgs.map { it.name }.filter { it != distroName })
            }
        } catch (e: Exception) {
            service = e.toServiceState(getApplication())
            status = null
        }
    }

    fun refreshAsync() {
        viewModelScope.launch { refresh() }
    }

    fun loadKernelReport() {
        viewModelScope.launch {
            kernelReportError = null
            kernelReport = null
            refresh()
            if (service !is ServiceState.Ready) return@launch
            try {
                kernelReport = io { client.kernelReport() }
            } catch (e: Exception) {
                kernelReportError = e.userMessage(getApplication())
            }
        }
    }

    private fun syncSelection() {
        val match = images.firstOrNull { it.name == distroName } ?: images.firstOrNull() ?: return
        if (match.name != distroName || match.imagePath != imagePath) {
            distroName = match.name
            imagePath = match.imagePath
            persist()
        }
    }

    fun selectImage(img: ImageInfo) {
        distroName = img.name
        imagePath = img.imagePath
        persist()
    }

    fun startDistro(name: String = distroName) = launchAction {
        startImage(name)
        refresh()
    }

    private suspend fun startSelected() = startImage(distroName)

    private suspend fun startImage(name: String) {
        val path = image(name)?.imagePath ?: if (name == distroName) imagePath else return
        val shared = if (shareEnabled) sharedDir else ""
        io { client.startDistro(name = name, imagePath = path, sharedDir = shared) }
        refresh()
    }

    fun openSettings() {
        settingsOrigin = screen
        screen = Screen.Settings
    }

    fun closeSettings() {
        screen = if (settingsOrigin == Screen.Terminal && tabs.isEmpty()) Screen.Home else settingsOrigin
    }

    fun requestStopDistro(name: String) {
        val hasShells = tabs.any { it.distro == name } || status?.sessions?.any { it.distro == name } == true
        if (hasShells) stopTarget = name else stopDistro(name)
    }

    fun dismissStop() {
        stopTarget = null
    }

    fun confirmStop() {
        val name = stopTarget ?: return
        dismissStop()
        stopDistro(name)
    }

    private fun stopDistro(name: String) = launchAction {
        io { client.stopDistro(name) }
        dropDistroTabs(name)
        RunningNotifier.dismiss(getApplication(), name)
        refresh()
    }

    fun openTerminal(name: String = distroName) = launchAction {
        if (name != distroName) image(name)?.let(::selectImage)
        val existing = tabs.firstOrNull { it.id == activeTabId && it.distro == name }
            ?: tabs.lastOrNull { it.distro == name }
        if (existing != null) {
            selectTab(existing.id)
        } else {
            if (!isRunning(name)) startImage(name)
            newTab(name, root = false)
        }
        screen = Screen.Terminal
    }

    fun openNewTab(root: Boolean, name: String? = null) = launchAction {
        val target = name ?: tabs.firstOrNull { it.id == activeTabId }?.distro ?: distroName
        if (!isRunning(target)) startImage(target)
        newTab(target, root)
        screen = Screen.Terminal
    }

    fun showTab(id: Int) {
        selectTab(id)
        screen = Screen.Terminal
    }

    fun resizeImage(name: String, sizeGb: Int) = launchJob(name) {
        io { client.resizeDistro(name, sizeGb) }
        messages.trySend(str(R.string.msg_resized, name, sizeGb))
    }

    fun setKeepAwake(img: ImageInfo, keepAwake: Boolean) = configure(img, keepAwake = keepAwake)

    fun setBootInit(img: ImageInfo, enabled: Boolean) =
        configure(img, init = if (enabled) INIT_SYSTEM else ch.ausfuhrli.bareboot.ipc.INIT_NONE)

    fun updateDetails(img: ImageInfo, newName: String, osLabel: String, username: String) = launchAction {
        if (osLabel != img.osLabel || username != img.username) {
            io { client.configureImage(img.name, osLabel, username, img.keepAwake, img.init, img.isolatedNet, img.sshPort, img.portForwards) }
        }
        if (newName != img.name) {
            io { client.renameImage(img.name, newName) }
            store.renameImage(img.name, newName)
            if (distroName == img.name) {
                distroName = newName
                imagePath = img.imagePath.replace("/distros/${img.name}/", "/distros/$newName/")
                persist()
            }
        }
        refresh()
    }

    fun setIsolatedNet(img: ImageInfo, enabled: Boolean) {
        val savedSshPort = store.sshConfig(img.name)?.takeIf { it.enabled }?.port ?: 0
        val sshPort = if (enabled && img.sshPort == 0) savedSshPort else img.sshPort
        configure(img, isolatedNet = enabled, sshPort = sshPort)
    }

    fun setGpu(img: ImageInfo, enabled: Boolean) = launchAction {
        io { client.configureGpu(img.name, enabled) }
        refresh()
    }

    fun setPortForwards(img: ImageInfo, portForwards: List<PortForward>) =
        configure(img, portForwards = portForwards)

    private fun configure(
        img: ImageInfo,
        keepAwake: Boolean = img.keepAwake,
        init: String = img.init,
        isolatedNet: Boolean = img.isolatedNet,
        sshPort: Int = img.sshPort,
        portForwards: List<PortForward> = img.portForwards,
    ) = launchAction {
        io {
            client.configureImage(
                img.name,
                img.osLabel.ifEmpty { img.name },
                img.username.ifEmpty { "user" },
                keepAwake,
                init,
                isolatedNet,
                sshPort,
                portForwards,
            )
        }
        refresh()
    }

    fun setDaemonOptions(purgeOnUninstall: Boolean, relaxedSync: Boolean) = launchAction {
        io { client.setDaemonOptions(purgeOnUninstall, relaxedSync) }
        refresh()
    }

    suspend fun readLog(distro: String, name: String): String = try {
        io { client.readLog(distro, name) }.ifEmpty { str(R.string.diagnostics_log_empty) }
    } catch (e: Exception) {
        e.userMessage(getApplication())
    }

    suspend fun diagnosticsReport(logs: Map<String, String>): String {
        val report = try {
            io { client.kernelReport() }
        } catch (_: Exception) {
            null
        }
        val app = getApplication<Application>()
        val version = runCatching { app.packageManager.getPackageInfo(app.packageName, 0).versionName }.getOrNull()
        return buildString {
            appendLine("Bareboot ${version.orEmpty()}")
            appendLine("Android ${android.os.Build.VERSION.RELEASE} (API ${android.os.Build.VERSION.SDK_INT})")
            appendLine("Root manager: ${RootManager.name(app, RootManager.detect(app))}")
            appendLine("Service: $service")
            report?.let { kr ->
                appendLine("Kernel: ${kr.release}")
                appendLine("Missing: " + kr.checks.filter { it.state == ch.ausfuhrli.bareboot.ipc.CheckState.Missing }.joinToString(" ") { it.id })
            }
            for (img in images) {
                appendLine(
                    "Image ${img.name}: ${img.osLabel}, init=${img.init}, keep_awake=${img.keepAwake}, " +
                        "running=${isRunning(img.name)}, free=${img.fsFreeBytes / (1024 * 1024)} MB",
                )
            }
            for ((title, text) in logs) {
                appendLine()
                appendLine("== $title")
                appendLine(text.takeLast(8_000))
            }
        }
    }

    fun exportImage(name: String) = launchJob(name) {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmm", Locale.ROOT).format(Date())
        val user = android.os.Process.myUid() / 100_000
        val file = "bareboot-$name-$stamp.tar.gz"
        io { client.exportImage(name, "/data/media/$user/Download/$file") }
        messages.trySend(str(R.string.msg_backup_saved, file))
    }

    private fun launchJob(name: String, block: suspend () -> Unit) {
        if (name in pendingJobs) return
        pendingJobs = pendingJobs + name
        viewModelScope.launch {
            try {
                refresh()
                block()
            } catch (e: Exception) {
                messages.trySend(e.userMessage(getApplication()))
            } finally {
                pendingJobs = pendingJobs - name
                refresh()
            }
        }
    }

    fun loadCatalog() {
        if (!catalog.isNullOrEmpty()) return
        viewModelScope.launch {
            catalogError = null
            try {
                catalog = io { DistroCatalog.load() }
            } catch (e: Exception) {
                catalogError = e.userMessage(getApplication())
            }
        }
    }

    fun installDistro(distro: Distro, release: DistroRelease, name: String, username: String, sizeGb: Int) {
        if (install != null) return
        InstallService.installRelease(getApplication(), distro.id, release, name, username, distro.osLabel(release), sizeGb)
    }

    fun importArchive(uri: Uri, name: String, username: String, osLabel: String, sizeGb: Int) {
        if (install != null) return
        InstallService.importArchive(getApplication(), uri, name, username, osLabel, sizeGb)
    }

    fun cancelInstall() {
        InstallTracker.cancelRequested = true
    }

    fun deleteImage(name: String) = launchAction {
        io { client.deleteDistro(name) }
        refresh()
        messages.trySend(str(R.string.msg_deleted, name))
    }

    fun sshConfig(image: String): SshConfig {
        val saved = store.sshConfig(image)
        if (saved != null) return saved
        val taken = images.mapNotNull { store.sshConfig(it.name)?.port }.toSet()
        val port = generateSequence(SshSetup.DEFAULT_PORT) { it + 1 }.first { it !in taken }
        return SshConfig(enabled = false, port = port, keys = images.firstNotNullOfOrNull { store.sshConfig(it.name)?.keys }.orEmpty())
    }

    fun applySsh(image: String, config: SshConfig) = launchAction {
        val targetPort = if (config.enabled) config.port else 0
        image(image)?.let { img ->
            if (img.sshPort != targetPort) {
                io {
                    client.configureImage(
                        img.name,
                        img.osLabel.ifEmpty { img.name },
                        img.username.ifEmpty { "user" },
                        img.keepAwake,
                        img.init,
                        img.isolatedNet,
                        targetPort,
                        img.portForwards,
                    )
                }
            }
        }
        store.saveSshConfig(image, config)
        if (!isRunning(image)) startImage(image)
        ipcLock.withLock {
            val app = getApplication<Application>()
            val info = io { client.createTab(image, 0, SshSetup.argv(app), SshSetup.env(config)) }
            addTab(info)
            selectTab(info.sessionId)
        }
        refresh()
        screen = Screen.Terminal
    }

    fun closeTab(tab: TerminalTab) = launchAction {
        try {
            io { client.killSession(tab.id, OsConstants.SIGHUP) }
        } catch (e: BarebootRpcException) {
            if (e.code != OsConstants.ENOENT) throw e
        }
        removeTab(tab)
    }

    fun changeShareEnabled(enabled: Boolean) {
        shareEnabled = enabled
        persist()
        filesConnected = folders.activeTree() != null
        if (distroRunning) messages.trySend(str(R.string.msg_restart_to_apply, distroName))
    }

    fun folderPickerLocation(): Uri? = folders.pickerLocation(sharedDir)

    fun chooseSharedFolder(tree: Uri) {
        val path = folders.pathForTree(tree)
        if (path == null) {
            messages.trySend(str(R.string.msg_choose_internal_folder))
            return
        }
        val changed = path != sharedDir
        sharedDir = path
        shareEnabled = true
        persist()
        filesConnected = folders.connect(tree)
        if (changed && distroRunning) messages.trySend(str(R.string.msg_restart_for_folder, distroName))
    }

    fun disconnectFiles() {
        folders.disconnect()
        filesConnected = false
    }

    fun filesRootUri(): Uri? = folders.rootUri()

    fun changeAutoStart(enabled: Boolean) {
        autoStart = enabled
        persist()
    }

    fun finishOnboarding() {
        store.onboardingDone = true
        screen = Screen.Home
        selectedImage?.let { openTerminal(it.name) }
    }

    fun openFromNotification(name: String) {
        if (!store.onboardingDone) return
        viewModelScope.launch {
            refresh()
            if (image(name) != null) openTerminal(name)
        }
    }

    fun restartOnboarding() {
        screen = Screen.Onboarding
    }

    fun resolveTypeface(): Typeface = typefaceFor(importedFont, useBundledFont)

    fun typefaceFor(imported: String?, bundled: Boolean): Typeface {
        imported?.let { name ->
            val path = runCatching {
                GhosttyFontStore.listImportedFonts(getApplication()).firstOrNull { it.name == name }?.path
            }.getOrNull()
            path?.let { p -> runCatching { GhosttyFontStore.loadTypeface(p) }.getOrNull()?.let { return it } }
        }
        return if (bundled) (bundledTypeface ?: Typeface.MONOSPACE) else Typeface.MONOSPACE
    }

    fun importedFontNames(): List<String> =
        runCatching { GhosttyFontStore.listImportedFonts(getApplication()).map { it.name } }.getOrDefault(emptyList())

    fun selectImportedFont(name: String) {
        importedFont = name
        store.importedFont = name
    }

    fun importFont(uri: Uri) = launchAction {
        val font = io { GhosttyFontStore.importFont(getApplication(), uri) }
            ?: throw java.io.IOException(str(R.string.msg_invalid_font))
        importedFont = font.name
        store.importedFont = font.name
        messages.trySend(str(R.string.msg_using_font, font.name))
    }

    fun selectTheme(id: String) {
        themeId = id
        persist()
    }

    fun setFontChoice(bundled: Boolean) {
        importedFont = null
        store.importedFont = null
        useBundledFont = bundled
        persist()
    }

    fun setFontSize(sp: Float) {
        val next = sp.coerceIn(MIN_FONT_SP, MAX_FONT_SP)
        if (next == fontSizeSp) return
        fontSizeSp = next
        persist()
    }

    fun setExtraKeys(keys: List<String>, chars: String) {
        extraKeys = keys
        extraChars = chars.take(32)
        store.extraKeys = keys
        store.extraChars = extraChars
    }

    fun nextTab(step: Int) {
        if (tabs.isEmpty()) return
        val index = tabs.indexOfFirst { it.id == activeTabId }.coerceAtLeast(0)
        selectTab(tabs[(index + step).mod(tabs.size)].id)
    }

    fun closeActiveTab() {
        tabs.firstOrNull { it.id == activeTabId }?.let(::closeTab)
    }

    fun renameTab(id: Int, name: String) {
        val trimmed = name.trim().take(40)
        if (trimmed.isEmpty()) tabNames.remove(id) else tabNames[id] = trimmed
        store.setTabName(id, trimmed)
    }

    fun selectTab(id: Int) {
        activeTabId = id
        store.activeTabId = id
    }

    private suspend fun restoreTabs(sessions: List<SessionInfo>) {
        for (info in sessions) {
            if (tabs.none { it.id == info.sessionId }) {
                try {
                    addTab(info)
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                }
            }
        }
        if (tabs.none { it.id == activeTabId }) {
            val remembered = tabs.firstOrNull { it.id == store.activeTabId }
            (remembered ?: tabs.lastOrNull())?.let { selectTab(it.id) }
        }
    }

    private suspend fun newTab(distro: String, root: Boolean) {
        val uid = if (root) 0 else 1000
        ipcLock.withLock {
            val info = io { client.createTab(distro, uid) }
            addTab(info)
            selectTab(info.sessionId)
        }
    }

    private suspend fun addTab(info: SessionInfo) {
        val attached = io { client.attachTab(info.sessionId, 80, 24, 0, 0) }
        val connection = TabConnection(info.sessionId, attached)
        val snapshot = snapshotFile(info.sessionId, info.pid)
        val saved = io { runCatching { snapshot.takeIf { it.isFile }?.readBytes() }.getOrNull() }
        val session = saved?.let {
            runCatching { GhosttyTerminalSession.restore(getApplication(), it) }.getOrNull()
        } ?: GhosttyTerminalSession(getApplication())
        session.transport = connection
        val tab = TerminalTab(info.sessionId, info.distro, info.uid, info.pid, session)
        store.tabName(info.sessionId)?.let { tabNames[info.sessionId] = it }
        tabs.add(tab)
        connection.start(session) {
            if (tabs.any { it.id == tab.id }) removeTab(tab)
        }
    }

    private fun dropDistroTabs(distro: String) {
        tabs.filter { it.distro == distro }.forEach(::removeTab)
    }

    private fun snapshotDir(): File = File(getApplication<Application>().filesDir, "scrollback")

    private fun snapshotFile(id: Int, pid: Int): File = File(snapshotDir(), "$id-$pid.bin")

    fun saveScrollback() {
        val dir = snapshotDir()
        val current = tabs.map { tab ->
            val data = runCatching { tab.session.saveSnapshot() }.getOrNull()
            Triple(snapshotFile(tab.id, tab.pid), data, tab)
        }
        viewModelScope.launch(Dispatchers.IO) {
            dir.mkdirs()
            val keep = current.map { it.first.name }.toSet()
            dir.listFiles()?.filter { it.name !in keep }?.forEach { it.delete() }
            for ((target, data, _) in current) {
                if (data == null) continue
                val temporary = File(dir, target.name + ".tmp")
                runCatching {
                    temporary.writeBytes(data)
                    temporary.renameTo(target)
                }
            }
        }
    }

    private fun removeTab(tab: TerminalTab) {
        snapshotFile(tab.id, tab.pid).delete()
        tabs.removeAll { it.id == tab.id }
        tabNames.remove(tab.id)
        store.setTabName(tab.id, "")
        tab.session.close()
        if (activeTabId == tab.id) selectTab(tabs.lastOrNull()?.id ?: -1)
        if (tabs.isEmpty() && screen == Screen.Terminal) screen = Screen.Home
    }

    private fun persist() {
        store.save(
            BarebootPrefs(
                distroName = distroName,
                imagePath = imagePath,
                sharedDir = sharedDir,
                shareEnabled = shareEnabled,
                autoStart = autoStart,
                fontSizeSp = fontSizeSp,
                themeId = themeId,
                useBundledFont = useBundledFont,
            )
        )
    }

    private fun launchAction(block: suspend () -> Unit) {
        if (busy) return
        viewModelScope.launch { runAction(block) }
    }

    private suspend fun runAction(block: suspend () -> Unit) {
        busy = true
        try {
            block()
        } catch (e: Exception) {
            messages.trySend(e.userMessage(getApplication()))
            if (e is DaemonUnavailableException || (e is BarebootRpcException && e.code == OsConstants.EACCES)) {
                service = e.toServiceState(getApplication())
            }
        } finally {
            busy = false
        }
    }

    private fun str(resId: Int, vararg args: Any): String =
        if (args.isEmpty()) getApplication<Application>().getString(resId)
        else getApplication<Application>().getString(resId, *args)

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    override fun onCleared() {
        val sessions = tabs.map { it.session }
        tabs.clear()
        sessions.forEach { it.close() }
        super.onCleared()
    }

}

private fun Exception.toServiceState(context: Context): ServiceState = when {
    this is DaemonUnavailableException -> ServiceState.NotRunning
    this is BarebootRpcException && code == OsConstants.EACCES -> ServiceState.Unauthorized
    else -> ServiceState.Failed(userMessage(context))
}

fun Throwable.userMessage(context: Context): String = when {
    this is DaemonUnavailableException -> context.getString(R.string.home_service_not_running_detail)
    this is BarebootRpcException && code == OsConstants.EACCES -> context.getString(R.string.home_service_unauthorized_detail)
    this is SocketTimeoutException -> context.getString(R.string.err_service_timeout)
    this is BarebootRpcException -> explainDaemonError(context, message.orEmpty())
    else -> message?.replaceFirstChar { it.uppercase() } ?: javaClass.simpleName
}

fun explainDaemonError(context: Context, message: String): String = when {
    "still mounted by processes from an earlier start" in message -> context.getString(R.string.err_still_mounted)
    "is busy (export)" in message -> context.getString(R.string.err_busy_export)
    "is busy (resize)" in message -> context.getString(R.string.err_busy_resize)
    "is busy (create)" in message -> context.getString(R.string.err_busy_create)
    "No space left on device" in message -> context.getString(R.string.err_no_space)
    "File exists" in message -> context.getString(R.string.err_file_exists)
    "distro image already exists" in message -> context.getString(R.string.err_image_exists)
    "no root manager busybox found" in message -> context.getString(R.string.err_no_busybox)
    "distro not running" in message -> context.getString(R.string.err_distro_not_running)
    "new size must be larger" in message -> context.getString(R.string.err_resize_larger)
    else -> message.replaceFirstChar { it.uppercase() }
}

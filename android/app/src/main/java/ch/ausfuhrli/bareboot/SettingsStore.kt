package ch.ausfuhrli.bareboot

import android.content.Context
import android.net.Uri
import ch.ausfuhrli.bareboot.files.SharedFolderAccess
import ch.ausfuhrli.bareboot.ssh.SshConfig
import ch.ausfuhrli.bareboot.ssh.SshSetup

data class BarebootPrefs(
    val distroName: String,
    val imagePath: String,
    val sharedDir: String,
    val shareEnabled: Boolean,
    val autoStart: Boolean,
    val fontSizeSp: Float,
    val themeId: String,
    val useBundledFont: Boolean,
)

val DEFAULT_EXTRA_KEYS = listOf("esc", "tab", "ctrl", "alt", "left", "down", "up", "right", "home", "end", "paste")
const val DEFAULT_EXTRA_CHARS = "/-|~"
const val MIN_FONT_SP = 8f
const val MAX_FONT_SP = 28f

class SettingsStore(context: Context) {
    private val appContext = context.applicationContext
    private val prefs = appContext.getSharedPreferences("bareboot_prefs", Context.MODE_PRIVATE)

    var activeTabId: Int
        get() = prefs.getInt("active_tab", -1)
        set(id) {
            prefs.edit().putInt("active_tab", id).apply()
        }

    var onboardingDone: Boolean
        get() = prefs.getBoolean("onboarding_done", false)
        set(done) {
            prefs.edit().putBoolean("onboarding_done", done).apply()
        }

    var extraKeys: List<String>
        get() = prefs.getString("extra_keys", null)?.split(',')?.filter { it.isNotEmpty() } ?: DEFAULT_EXTRA_KEYS
        set(keys) {
            prefs.edit().putString("extra_keys", keys.joinToString(",")).apply()
        }

    var extraChars: String
        get() = prefs.getString("extra_chars", DEFAULT_EXTRA_CHARS) ?: DEFAULT_EXTRA_CHARS
        set(chars) {
            prefs.edit().putString("extra_chars", chars).apply()
        }

    var importedFont: String?
        get() = prefs.getString("imported_font", null)
        set(name) {
            prefs.edit().putString("imported_font", name).apply()
        }

    fun tabName(id: Int): String? = prefs.getString("tab_name_$id", null)

    fun setTabName(id: Int, name: String) {
        prefs.edit().apply { if (name.isEmpty()) remove("tab_name_$id") else putString("tab_name_$id", name) }.apply()
    }

    fun sshConfig(image: String): SshConfig? {
        if (!prefs.contains("ssh_port_$image")) return null
        return SshConfig(
            enabled = prefs.getBoolean("ssh_enabled_$image", false),
            port = prefs.getInt("ssh_port_$image", SshSetup.DEFAULT_PORT),
            keys = prefs.getString("ssh_keys_$image", "").orEmpty(),
        )
    }

    fun saveSshConfig(image: String, config: SshConfig) {
        prefs.edit()
            .putBoolean("ssh_enabled_$image", config.enabled)
            .putInt("ssh_port_$image", config.port)
            .putString("ssh_keys_$image", config.keys)
            .apply()
    }

    fun renameImage(old: String, new: String) {
        val saved = sshConfig(old) ?: return
        saveSshConfig(new, saved)
        prefs.edit()
            .remove("ssh_enabled_$old")
            .remove("ssh_port_$old")
            .remove("ssh_keys_$old")
            .apply()
    }

    var filesTree: Uri?
        get() = prefs.getString("files_tree", null)?.let(Uri::parse)
        set(tree) {
            prefs.edit().putString("files_tree", tree?.toString()).apply()
        }

    fun load(): BarebootPrefs = BarebootPrefs(
        distroName = prefs.getString("distro_name", "").orEmpty(),
        imagePath = prefs.getString("image_path", "").orEmpty(),
        sharedDir = prefs.getString("shared_dir", "/data/media/0/Linux") ?: "/data/media/0/Linux",
        shareEnabled = prefs.getBoolean("share_enabled", true),
        autoStart = prefs.getBoolean("auto_start", true),
        fontSizeSp = prefs.getFloat("font_size_sp", 14f).coerceIn(MIN_FONT_SP, MAX_FONT_SP),
        themeId = prefs.getString("theme_id", "onedark") ?: "onedark",
        useBundledFont = prefs.getBoolean("use_jb_font", true),
    )

    fun save(p: BarebootPrefs) {
        val refreshFiles = filesTree != null &&
            (p.shareEnabled != prefs.getBoolean("share_enabled", true) ||
                p.sharedDir != prefs.getString("shared_dir", "/data/media/0/Linux"))
        prefs.edit()
            .putString("distro_name", p.distroName)
            .putString("image_path", p.imagePath)
            .putString("shared_dir", p.sharedDir)
            .putBoolean("share_enabled", p.shareEnabled)
            .putBoolean("auto_start", p.autoStart)
            .putFloat("font_size_sp", p.fontSizeSp)
            .putString("theme_id", p.themeId)
            .putBoolean("use_jb_font", p.useBundledFont)
            .apply()
        if (refreshFiles) SharedFolderAccess.notifyRoots(appContext)
    }
}

package ch.ausfuhrli.bareboot.notify

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.os.Build
import android.service.quicksettings.TileService
import ch.ausfuhrli.bareboot.MainActivity
import ch.ausfuhrli.bareboot.R

class ShellTileService : TileService() {
    override fun onClick() {
        super.onClick()
        val distro = getSharedPreferences("bareboot_prefs", Context.MODE_PRIVATE).getString("distro_name", "").orEmpty()
        val intent = openShellIntent(this, distro)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT),
            )
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}

fun openShellIntent(context: Context, distro: String): Intent =
    Intent(context, MainActivity::class.java)
        .setAction(Intent.ACTION_VIEW)
        .putExtra(RunningNotifier.EXTRA_OPEN_DISTRO, distro)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)

object LauncherShortcuts {
    private var published: List<String>? = null

    fun update(context: Context, images: List<String>) {
        val names = images.take(4)
        if (names == published) return
        published = names
        val manager = context.getSystemService(ShortcutManager::class.java) ?: return
        val shortcuts = names.map { name ->
            ShortcutInfo.Builder(context, "shell-$name")
                .setShortLabel(name)
                .setLongLabel(context.getString(R.string.shortcut_shell_in, name))
                .setIcon(Icon.createWithResource(context, R.drawable.ic_shortcut_terminal))
                .setIntent(openShellIntent(context, name))
                .build()
        }
        runCatching { manager.dynamicShortcuts = shortcuts }
    }
}

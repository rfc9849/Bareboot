package ch.ausfuhrli.bareboot.notify

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import ch.ausfuhrli.bareboot.MainActivity
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.ipc.BarebootClient

object RunningNotifier {
    private const val CHANNEL = "running"
    private const val TAG_PREFIX = "distro:"
    private const val ID = 20
    const val EXTRA_OPEN_DISTRO = "ch.ausfuhrli.bareboot.extra.OPEN_DISTRO"

    fun update(context: Context, running: Collection<String>) {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (!manager.areNotificationsEnabled()) return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, context.getString(R.string.notif_running_channel), NotificationManager.IMPORTANCE_LOW),
        )
        val shown = manager.activeNotifications
            .mapNotNull { it.tag?.removePrefix(TAG_PREFIX)?.takeIf { _ -> it.tag.startsWith(TAG_PREFIX) } }
            .toSet()
        for (name in shown - running.toSet()) manager.cancel(TAG_PREFIX + name, ID)
        for (name in running.toSet() - shown) manager.notify(TAG_PREFIX + name, ID, build(context, name))
    }

    fun dismiss(context: Context, name: String) {
        context.getSystemService(NotificationManager::class.java).cancel(TAG_PREFIX + name, ID)
    }

    private fun build(context: Context, name: String): Notification {
        val code = name.hashCode()
        val open = PendingIntent.getActivity(
            context,
            code,
            Intent(context, MainActivity::class.java)
                .putExtra(EXTRA_OPEN_DISTRO, name)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getBroadcast(
            context,
            code,
            Intent(context, DistroActionReceiver::class.java)
                .setAction(DistroActionReceiver.ACTION_STOP)
                .putExtra(DistroActionReceiver.EXTRA_NAME, name),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_terminal)
            .setContentTitle(context.getString(R.string.notif_distro_running, name))
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setGroup("running")
            .addAction(Notification.Action.Builder(null, context.getString(R.string.notif_open_shell), open).build())
            .addAction(Notification.Action.Builder(null, context.getString(R.string.action_stop), stop).build())
            .build()
    }
}

class DistroActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_STOP) return
        val name = intent.getStringExtra(EXTRA_NAME) ?: return
        val pending = goAsync()
        val app = context.applicationContext
        Thread {
            try {
                BarebootClient().stopDistro(name)
                RunningNotifier.dismiss(app, name)
            } catch (_: Exception) {
            } finally {
                pending.finish()
            }
        }.start()
    }

    companion object {
        const val ACTION_STOP = "ch.ausfuhrli.bareboot.action.STOP_DISTRO"
        const val EXTRA_NAME = "name"
    }
}

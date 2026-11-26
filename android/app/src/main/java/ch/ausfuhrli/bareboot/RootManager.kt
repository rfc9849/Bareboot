package ch.ausfuhrli.bareboot

import android.content.Context
import android.content.Intent

enum class RootManager(val label: String, val packageName: String) {
    KernelSu("KernelSU", "me.weishu.kernelsu"),
    SukiSu("SukiSU Ultra", "com.sukisu.ultra"),
    Magisk("Magisk", "com.topjohnwu.magisk");

    fun launchIntent(context: Context): Intent? =
        context.packageManager.getLaunchIntentForPackage(packageName)?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    companion object {
        fun detect(context: Context): RootManager? = entries.firstOrNull { it.launchIntent(context) != null }

        fun name(context: Context, manager: RootManager?): String =
            manager?.label ?: context.getString(R.string.label_root_manager_generic)
    }
}

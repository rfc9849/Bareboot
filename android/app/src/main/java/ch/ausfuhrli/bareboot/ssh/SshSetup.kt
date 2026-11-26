package ch.ausfuhrli.bareboot.ssh

import android.content.Context
import ch.ausfuhrli.bareboot.R
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.net.Inet4Address

data class SshConfig(val enabled: Boolean, val port: Int, val keys: String)

object SshSetup {
    const val DEFAULT_PORT = 2222

    private val KEY_RE = Regex(
        "^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp(256|384|521)|sk-ssh-ed25519@openssh\\.com|" +
            "sk-ecdsa-sha2-nistp256@openssh\\.com) [A-Za-z0-9+/]+={0,3}( [^\\p{Cntrl}]*)?$",
    )

    fun keyLines(text: String): List<String> = text.lines().map(String::trim).filter { it.isNotEmpty() && !it.startsWith("#") }

    fun keysError(context: Context, text: String): String? {
        val lines = keyLines(text)
        return when {
            lines.isEmpty() -> context.getString(R.string.ssh_err_no_keys)
            lines.any { !KEY_RE.matches(it) } -> context.getString(R.string.ssh_err_bad_key)
            else -> null
        }
    }

    fun portError(context: Context, port: Int?): String? =
        if (port == null || port !in 1024..65535) context.getString(R.string.ssh_err_port) else null

    fun wifiAddress(context: Context): String? {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return null
        return cm.allNetworks.firstNotNullOfOrNull { network ->
            val caps = cm.getNetworkCapabilities(network)
            if (caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) != true) return@firstNotNullOfOrNull null
            cm.getLinkProperties(network)?.linkAddresses
                ?.map { it.address }
                ?.firstOrNull { it is Inet4Address && !it.isLoopbackAddress }
                ?.hostAddress
        }
    }

    fun argv(context: Context): List<String> {
        val script = context.assets.open("ssh-setup.sh").bufferedReader().use { it.readText() }
        return listOf("/bin/sh", "-c", script, "bareboot-ssh")
    }

    fun env(config: SshConfig): List<String> = listOf(
        "BAREBOOT_SSH_ENABLE=${if (config.enabled) 1 else 0}",
        "BAREBOOT_SSH_PORT=${config.port}",
        "BAREBOOT_SSH_KEYS=${keyLines(config.keys).joinToString("\n")}",
    )
}

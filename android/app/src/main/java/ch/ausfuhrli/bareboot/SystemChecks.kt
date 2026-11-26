package ch.ausfuhrli.bareboot

import android.content.Context
import android.os.Build
import android.system.Os
import android.system.OsConstants
import androidx.annotation.StringRes
import ch.ausfuhrli.bareboot.ipc.CheckState
import ch.ausfuhrli.bareboot.ipc.KernelReport

enum class CheckGroup(@param:StringRes val titleRes: Int) {
    Device(R.string.check_group_device),
    Runtime(R.string.check_group_runtime),
    Init(R.string.check_group_init),
    Containers(R.string.check_group_containers),
}

enum class CheckLevel { Required, Recommended, Optional }

data class CheckItem(
    val id: String,
    val label: String,
    val detail: String?,
    val group: CheckGroup,
    val level: CheckLevel,
    val state: CheckState,
)

object SystemChecks {
    private data class Spec(
        @param:StringRes val labelRes: Int,
        val group: CheckGroup,
        val level: CheckLevel,
        val missingDetail: String? = null,
    )

    private val specs = linkedMapOf(
        "loop_control" to Spec(R.string.check_loop_control, CheckGroup.Runtime, CheckLevel.Required),
        "fs_ext4" to Spec(R.string.check_fs_ext4, CheckGroup.Runtime, CheckLevel.Required),
        "fs_devpts" to Spec(R.string.check_fs_devpts, CheckGroup.Runtime, CheckLevel.Required),
        "ns_mnt" to Spec(R.string.check_ns_mnt, CheckGroup.Runtime, CheckLevel.Required),
        "ns_uts" to Spec(R.string.check_ns_uts, CheckGroup.Runtime, CheckLevel.Required),
        "ns_pid" to Spec(R.string.check_ns_pid, CheckGroup.Init, CheckLevel.Optional, "CONFIG_PID_NS"),
        "ns_ipc" to Spec(R.string.check_ns_ipc, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_IPC_NS"),
        "ns_user" to Spec(R.string.check_ns_user, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_USER_NS"),
        "ns_cgroup" to Spec(R.string.check_ns_cgroup, CheckGroup.Init, CheckLevel.Optional, "CONFIG_CGROUP_NS"),
        "ns_net" to Spec(R.string.check_ns_net, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_NET_NS"),
        "cgroup2" to Spec(R.string.check_cgroup2, CheckGroup.Init, CheckLevel.Optional),
        "cgroup_cpu" to Spec(R.string.check_cgroup_cpu, CheckGroup.Containers, CheckLevel.Optional),
        "cgroup_memory" to Spec(R.string.check_cgroup_memory, CheckGroup.Containers, CheckLevel.Optional),
        "cgroup_pids" to Spec(R.string.check_cgroup_pids, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_CGROUP_PIDS"),
        "config_TMPFS_XATTR" to Spec(R.string.check_tmpfs_xattr, CheckGroup.Init, CheckLevel.Optional, "CONFIG_TMPFS_XATTR"),
        "config_FHANDLE" to Spec(R.string.check_fhandle, CheckGroup.Init, CheckLevel.Optional, "CONFIG_FHANDLE"),
        "fs_overlay" to Spec(R.string.check_fs_overlay, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_OVERLAY_FS"),
        "config_SYSVIPC" to Spec(R.string.check_sysvipc, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_SYSVIPC"),
        "config_POSIX_MQUEUE" to Spec(R.string.check_posix_mqueue, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_POSIX_MQUEUE"),
        "config_VETH" to Spec(R.string.check_veth, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_VETH"),
        "config_BRIDGE" to Spec(R.string.check_bridge, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_BRIDGE"),
        "config_BRIDGE_NETFILTER" to Spec(R.string.check_bridge_netfilter, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_BRIDGE_NETFILTER"),
        "config_NETFILTER_XT_MATCH_ADDRTYPE" to Spec(
            R.string.check_xt_addrtype, CheckGroup.Containers, CheckLevel.Optional,
            "CONFIG_NETFILTER_XT_MATCH_ADDRTYPE",
        ),
        "config_NF_TABLES" to Spec(R.string.check_nf_tables, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_NF_TABLES"),
        "config_NFT_COMPAT" to Spec(R.string.check_nft_compat, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_NFT_COMPAT"),
        "config_SECCOMP_FILTER" to Spec(R.string.check_seccomp_filter, CheckGroup.Containers, CheckLevel.Optional, "CONFIG_SECCOMP_FILTER"),
    )

    fun device(context: Context): List<CheckItem> {
        val release = System.getProperty("os.version").orEmpty()
        val version = Regex("^(\\d+)\\.(\\d+)").find(release)?.destructured
            ?.let { (major, minor) -> major.toInt() to minor.toInt() }
        val kernelOk = version?.let { (major, minor) -> major > 6 || (major == 6 && minor >= 1) }
        val pageSize = Os.sysconf(OsConstants._SC_PAGESIZE)
        val kib = (pageSize / 1024).toInt()
        return listOf(
            CheckItem(
                id = "arm64",
                label = context.getString(R.string.check_arm64),
                detail = Build.SUPPORTED_ABIS.firstOrNull(),
                group = CheckGroup.Device,
                level = CheckLevel.Required,
                state = if ("arm64-v8a" in Build.SUPPORTED_64_BIT_ABIS) CheckState.Present else CheckState.Missing,
            ),
            CheckItem(
                id = "kernel_version",
                label = context.getString(R.string.check_kernel_version),
                detail = release.substringBefore('-').ifEmpty { null },
                group = CheckGroup.Device,
                level = CheckLevel.Required,
                state = when (kernelOk) {
                    true -> CheckState.Present
                    false -> CheckState.Missing
                    null -> CheckState.Unknown
                },
            ),
            CheckItem(
                id = "page_size",
                label = context.getString(R.string.check_page_size),
                detail = if (pageSize == 4096L || pageSize == 16384L) {
                    context.getString(R.string.check_page_size_kib, kib)
                } else {
                    context.getString(R.string.check_page_size_untested, kib)
                },
                group = CheckGroup.Device,
                level = CheckLevel.Recommended,
                state = if (pageSize == 4096L || pageSize == 16384L) CheckState.Present else CheckState.Unknown,
            ),
        )
    }

    fun fromReport(context: Context, report: KernelReport): List<CheckItem> {
        val byId = report.checks.associateBy { it.id }
        val known = specs.mapNotNull { (id, spec) ->
            val check = byId[id] ?: return@mapNotNull null
            CheckItem(
                id = id,
                label = context.getString(spec.labelRes),
                detail = when (check.state) {
                    CheckState.Missing -> spec.missingDetail
                    CheckState.Unknown -> context.getString(R.string.check_unreadable)
                    CheckState.Present -> null
                },
                group = spec.group,
                level = spec.level,
                state = check.state,
            )
        }
        val unknown = report.checks.filter { it.id !in specs }.map {
            CheckItem(it.id, it.id, null, CheckGroup.Containers, CheckLevel.Optional, it.state)
        }
        return known + unknown
    }

    fun blocking(items: List<CheckItem>): List<CheckItem> =
        items.filter { it.level == CheckLevel.Required && it.state == CheckState.Missing }
}

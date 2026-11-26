package ch.ausfuhrli.bareboot.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.unit.dp
import ch.ausfuhrli.bareboot.CheckGroup
import ch.ausfuhrli.bareboot.CheckItem
import ch.ausfuhrli.bareboot.CheckLevel
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.Screen
import ch.ausfuhrli.bareboot.ServiceState
import ch.ausfuhrli.bareboot.SystemChecks
import ch.ausfuhrli.bareboot.ipc.CheckState

data class CheckResults(
    val items: List<CheckItem>,
    val loading: Boolean,
    val note: String?,
    val reportAvailable: Boolean,
    val isolatedNetwork: Boolean,
) {
    val blocking: List<CheckItem> get() = SystemChecks.blocking(items)
}

@Composable
fun rememberCheckResults(vm: BarebootViewModel): CheckResults {
    val context = LocalContext.current
    val report = vm.kernelReport
    val error = vm.kernelReportError
    val ready = vm.service is ServiceState.Ready
    val device = remember(context, ready) { SystemChecks.device(context, ready) }
    val items = device + (report?.let { SystemChecks.fromReport(context, it) } ?: emptyList())
    val loading = report == null && error == null && (ready || vm.service is ServiceState.Checking)
    val note = error ?: when (vm.service) {
        ServiceState.NotRunning -> stringResource(R.string.check_note_not_running)
        ServiceState.Unauthorized -> stringResource(R.string.check_note_unauthorized)
        is ServiceState.Failed -> (vm.service as ServiceState.Failed).message
        else -> null
    }
    return CheckResults(items, loading, note, ready && report != null, vm.isolatedNetworking)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SystemCheckScreen(vm: BarebootViewModel) {
    LaunchedEffect(Unit) { vm.loadKernelReport() }
    val results = rememberCheckResults(vm)
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings_system_check)) },
                navigationIcon = {
                    IconButton(onClick = { vm.screen = Screen.Settings }) {
                        Icon(AppIcons.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                actions = {
                    IconButton(onClick = vm::loadKernelReport) {
                        Icon(AppIcons.Refresh, contentDescription = stringResource(R.string.action_check_again))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(bottom = 24.dp),
        ) {
            CheckSummary(results, Modifier.padding(horizontal = 16.dp))
            CheckGroups(results.items)
            vm.kernelReport?.release?.takeIf { it.isNotEmpty() }?.let {
                Text(
                    text = stringResource(R.string.check_kernel_release, it),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 32.dp, vertical = 16.dp),
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CheckSummary(results: CheckResults, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val blocking = results.blocking
    val unconfirmed = !results.reportAvailable ||
        results.items.any { it.level == CheckLevel.Required && it.state == CheckState.Unknown }
    val (container, content) = when {
        blocking.isNotEmpty() -> colors.errorContainer to colors.onErrorContainer
        results.loading || unconfirmed -> colors.surfaceContainerHigh to colors.onSurface
        else -> colors.primaryContainer to colors.onPrimaryContainer
    }
    Surface(
        color = container,
        contentColor = content,
        shape = MaterialTheme.shapes.extraLarge,
        modifier = modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier.padding(20.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            when {
                blocking.isNotEmpty() -> ShapeIcon(
                    MaterialShapes.Burst, AppIcons.PriorityHigh, colors.error, colors.onError, 56.dp,
                )
                results.loading -> LoadingIndicator(modifier = Modifier.size(56.dp))
                unconfirmed -> ShapeIcon(
                    MaterialShapes.Burst, AppIcons.PriorityHigh, colors.onSurface, colors.surfaceContainerHighest, 56.dp,
                )
                else -> ShapeIcon(
                    MaterialShapes.Cookie9Sided, AppIcons.Done, colors.primary, colors.onPrimary, 56.dp,
                )
            }
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(
                    text = when {
                        blocking.isNotEmpty() -> stringResource(R.string.check_summary_blocked)
                        results.loading -> stringResource(R.string.check_summary_loading)
                        unconfirmed -> stringResource(R.string.check_summary_unconfirmed)
                        else -> stringResource(R.string.check_summary_ready)
                    },
                    style = MaterialTheme.typography.titleMedium,
                )
                val detail = when {
                    blocking.isNotEmpty() -> stringResource(R.string.check_summary_missing, blocking.joinToString { it.label })
                    results.loading -> null
                    results.note != null -> results.note
                    else -> null
                }
                detail?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                if (!results.loading && blocking.isEmpty() && !unconfirmed) {
                    FeatureChecks(results)
                }
            }
        }
    }
}

@Composable
private fun FeatureChecks(results: CheckResults) {
    val states = results.items.associate { it.id to it.state }
    val initChecks = listOf("ns_pid", "ns_cgroup", "cgroup2")
    val init = when {
        initChecks.any { states[it] == CheckState.Missing } -> CheckState.Missing
        initChecks.all { states[it] == CheckState.Present } -> CheckState.Present
        else -> CheckState.Unknown
    }
    Text(
        stringResource(
            when (init) {
                CheckState.Present -> R.string.check_summary_init_ready
                CheckState.Missing -> R.string.check_summary_init_needed
                CheckState.Unknown -> R.string.check_summary_init_unknown
            },
        ),
        style = MaterialTheme.typography.bodyMedium,
    )

    val bridgeChecks = listOf("config_VETH", "config_BRIDGE", "config_BRIDGE_NETFILTER")
    val nft = states["config_NF_TABLES"]
    val nftCompat = states["config_NFT_COMPAT"]
    val legacy = states["config_NETFILTER_XT_MATCH_ADDRTYPE"]
    val nftReady = nft == CheckState.Present && nftCompat == CheckState.Present
    val docker = when {
        init == CheckState.Missing || !results.isolatedNetwork ||
            bridgeChecks.any { states[it] == CheckState.Missing } -> R.string.check_summary_docker_bridge_missing
        (nft == CheckState.Missing || nftCompat == CheckState.Missing) && legacy == CheckState.Missing ->
            R.string.check_summary_docker_firewall_missing
        init != CheckState.Present || bridgeChecks.any { states[it] != CheckState.Present } ->
            R.string.check_summary_docker_unknown
        nftReady -> R.string.check_summary_docker_ready
        legacy == CheckState.Present -> R.string.check_summary_docker_legacy
        else -> R.string.check_summary_docker_unknown
    }
    Text(stringResource(docker), style = MaterialTheme.typography.bodyMedium)
    if (docker == R.string.check_summary_docker_ready || docker == R.string.check_summary_docker_legacy) {
        if (states["ns_ipc"] == CheckState.Missing || states["config_POSIX_MQUEUE"] == CheckState.Missing) {
            Text(stringResource(R.string.check_summary_docker_ipc), style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
fun CheckGroups(items: List<CheckItem>) {
    CheckGroup.entries.forEach { group ->
        val groupItems = items.filter { it.group == group }
        if (groupItems.isEmpty()) return@forEach
        if (group == CheckGroup.Init || group == CheckGroup.Containers) {
            CollapsibleGroup(group, groupItems)
        } else {
            SectionHeader(stringResource(group.titleRes))
            CheckItems(groupItems)
        }
    }
}

@Composable
private fun CollapsibleGroup(group: CheckGroup, items: List<CheckItem>) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    val present = items.count { it.state == CheckState.Present }
    SectionHeader(stringResource(group.titleRes))
    SegmentedColumn {
        SegmentedListItem(
            onClick = { expanded = !expanded },
            shapes = ListItemDefaults.segmentedShapes(0, if (expanded) items.size + 1 else 1),
            supportingContent = {
                Text(
                    stringResource(
                        if (group == CheckGroup.Init) R.string.check_group_init_desc
                        else R.string.check_group_containers_desc,
                    )
                )
            },
            trailingContent = {
                Icon(
                    if (expanded) AppIcons.ExpandLess else AppIcons.ExpandMore,
                    contentDescription = stringResource(if (expanded) R.string.action_collapse else R.string.action_expand),
                )
            },
        ) {
            Text(stringResource(R.string.check_available_count, present, items.size))
        }
        AnimatedVisibility(expanded) {
            Column(verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                items.forEachIndexed { index, item ->
                    CheckRow(item, index + 1, items.size + 1)
                }
            }
        }
    }
}

@Composable
private fun CheckItems(items: List<CheckItem>) {
    SegmentedColumn {
        items.forEachIndexed { index, item -> CheckRow(item, index, items.size) }
    }
}

@Composable
private fun CheckRow(item: CheckItem, index: Int, count: Int) {
    val colors = MaterialTheme.colorScheme
    val (icon, tint) = when (item.state) {
        CheckState.Present -> AppIcons.CheckCircle to colors.primary
        CheckState.Unknown -> AppIcons.HelpOutline to colors.onSurfaceVariant
        CheckState.Missing -> when (item.level) {
            CheckLevel.Required -> AppIcons.Error to colors.error
            CheckLevel.Recommended -> AppIcons.Warning to colors.tertiary
            CheckLevel.Optional -> AppIcons.RemoveCircleOutline to colors.onSurfaceVariant
        }
    }
    SegmentedListItem(
        shapes = ListItemDefaults.segmentedShapes(index, count),
        leadingContent = {
            Icon(
                icon,
                contentDescription = stringResource(
                    when (item.state) {
                        CheckState.Present -> R.string.check_state_present
                        CheckState.Missing -> R.string.check_state_missing
                        CheckState.Unknown -> R.string.check_state_unknown
                    },
                ),
                tint = tint,
            )
        },
        supportingContent = item.detail?.let { { Text(it) } },
    ) {
        Text(item.label)
    }
}

package ch.ausfuhrli.bareboot.ui


import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.IconButton
import androidx.compose.ui.Alignment
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.unit.dp
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.ipc.ImageInfo
import ch.ausfuhrli.bareboot.ipc.PortForward

private val IMAGE_NAME_RE = Regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
private val USERNAME_RE = Regex("^[a-z_][a-z0-9_-]{0,31}$")

private class SheetEntry(
    val title: String,
    val supporting: String?,
    val icon: @Composable () -> Unit,
    val enabled: Boolean = true,
    val checked: Boolean? = null,
    val onClick: () -> Unit,
)

@Composable
fun ImageSettingsSheet(
    vm: BarebootViewModel,
    image: ImageInfo,
    onDismiss: () -> Unit,
    onSsh: () -> Unit,
    onPortForwards: () -> Unit,
    onDetails: () -> Unit,
    onEnlarge: () -> Unit,
    onDelete: () -> Unit,
) {
    val running = vm.isRunning(image.name)
    val idle = image.busy.isEmpty() && image.name !in vm.pendingJobs && !vm.busy
    val stoppedNote = stringResource(R.string.image_stop_first, image.name)
    fun close(action: () -> Unit): () -> Unit = {
        onDismiss()
        action()
    }
    val shells = listOfNotNull(
        SheetEntry(
            stringResource(R.string.image_new_root_shell),
            stringResource(R.string.image_new_root_shell_desc, image.name),
            { Icon(AppIcons.AdminPanelSettings, null) },
            idle,
            onClick = close { vm.openNewTab(root = true, name = image.name) },
        ),
        SheetEntry(
            stringResource(R.string.image_ssh_access),
            stringResource(R.string.image_ssh_access_desc),
            { Icon(AppIcons.Key, null) },
            idle,
            onClick = close(onSsh),
        ),
        if (running) {
            SheetEntry(
                stringResource(R.string.action_stop),
                stringResource(R.string.image_stop_desc, image.name),
                { Icon(AppIcons.StopCircle, null) },
                idle,
                onClick = close { vm.stopDistro(image.name) },
            )
        } else {
            null
        },
    )
    val behavior = listOfNotNull(
        SheetEntry(
            stringResource(R.string.image_keep_awake),
            stringResource(R.string.image_keep_awake_desc, image.name),
            { Icon(AppIcons.Coffee, null) },
            idle,
            checked = image.keepAwake,
            onClick = { vm.setKeepAwake(image, !image.keepAwake) },
        ),
        SheetEntry(
            stringResource(R.string.image_boot_init),
            when {
                !vm.pidNamespaces -> stringResource(R.string.image_boot_init_missing)
                running -> stringResource(R.string.image_boot_init_running)
                else -> stringResource(R.string.image_boot_init_desc)
            },
            { Icon(AppIcons.PowerSettingsNew, null) },
            idle && (vm.pidNamespaces || image.bootsInit),
            checked = image.bootsInit,
            onClick = { vm.setBootInit(image, !image.bootsInit) },
        ),
        SheetEntry(
            stringResource(R.string.image_isolated_net),
            when {
                running -> stoppedNote
                !vm.isolatedNetworking && !image.isolatedNet -> stringResource(R.string.image_isolated_net_missing)
                else -> stringResource(R.string.image_isolated_net_desc)
            },
            { Icon(AppIcons.Extension, null) },
            idle && !running && (vm.isolatedNetworking || image.isolatedNet),
            checked = image.isolatedNet,
            onClick = { vm.setIsolatedNet(image, !image.isolatedNet) },
        ),
        SheetEntry(
            stringResource(R.string.image_port_forwarding),
            when {
                !image.isolatedNet -> stringResource(R.string.image_port_forwarding_needs_isolated)
                image.portForwards.isEmpty() -> stringResource(R.string.image_port_forwarding_empty)
                else -> image.portForwards.joinToString(", ") { "${it.hostPort} -> ${it.guestPort}" }
            },
            { Icon(AppIcons.OpenInNew, null) },
            idle && image.isolatedNet,
            onClick = close(onPortForwards),
        ),
        if (image.gpuAvailable || image.gpuEnabled) {
            SheetEntry(
                stringResource(R.string.image_gpu),
                when {
                    running && image.gpuRunning -> stringResource(R.string.image_gpu_start_command)
                    running && image.gpuEnabled -> stringResource(R.string.image_gpu_failed)
                    running -> stoppedNote
                    !image.gpuAvailable -> stringResource(R.string.image_gpu_missing)
                    else -> stringResource(R.string.image_gpu_desc)
                },
                { Icon(AppIcons.Memory, null) },
                idle && !running,
                checked = image.gpuEnabled,
                onClick = { vm.setGpu(image, !image.gpuEnabled) },
            )
        } else {
            null
        },
    )
    val storage = listOf(
        SheetEntry(
            stringResource(R.string.image_details),
            stringResource(R.string.image_details_desc),
            { Icon(AppIcons.Edit, null) },
            idle,
            onClick = close(onDetails),
        ),
        SheetEntry(
            stringResource(R.string.image_backup),
            if (running) stoppedNote else stringResource(R.string.image_backup_desc),
            { Icon(AppIcons.Backup, null) },
            idle && !running,
            onClick = close { vm.exportImage(image.name) },
        ),
        SheetEntry(
            stringResource(R.string.image_enlarge),
            if (running) stoppedNote else stringResource(R.string.image_enlarge_desc),
            { Icon(AppIcons.OpenInFull, null) },
            idle && !running,
            onClick = close(onEnlarge),
        ),
        SheetEntry(
            stringResource(R.string.action_delete),
            if (running) stoppedNote else stringResource(R.string.image_delete_desc),
            { Icon(AppIcons.Delete, null, tint = MaterialTheme.colorScheme.error) },
            idle && !running,
            onClick = close(onDelete),
        ),
    )
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        sheetGesturesEnabled = false,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(bottom = 24.dp),
        ) {
            Text(
                image.name,
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            val userPart = image.username.ifEmpty { null }?.let { stringResource(R.string.image_user_label, it) }
            Text(
                listOfNotNull(image.osLabel.ifEmpty { null }, userPart).joinToString(" · "),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            SheetGroup(stringResource(R.string.image_group_shells), shells)
            SheetGroup(stringResource(R.string.image_group_running), behavior)
            SheetGroup(stringResource(R.string.image_group_storage), storage)
        }
    }
}

@Composable
private fun SheetGroup(title: String, entries: List<SheetEntry>) {
    SectionHeader(title)
    val colors = switchRowColors()
    SegmentedColumn {
        entries.forEachIndexed { index, entry ->
            val shapes = switchRowShapes(index, entries.size)
            if (entry.checked != null) {
                SegmentedListItem(
                    checked = entry.checked,
                    onCheckedChange = { entry.onClick() },
                    enabled = entry.enabled,
                    shapes = shapes,
                    colors = colors,
                    leadingContent = entry.icon,
                    supportingContent = entry.supporting?.let { { Text(it) } },
                    trailingContent = { Switch(checked = entry.checked, onCheckedChange = null, enabled = entry.enabled) },
                ) { Text(entry.title) }
            } else {
                SegmentedListItem(
                    onClick = entry.onClick,
                    enabled = entry.enabled,
                    shapes = shapes,
                    colors = colors,
                    leadingContent = entry.icon,
                    supportingContent = entry.supporting?.let { { Text(it) } },
                ) { Text(entry.title) }
            }
        }
    }
}

@Composable
fun DetailsDialog(
    image: ImageInfo,
    running: Boolean,
    existing: List<String>,
    onDismiss: () -> Unit,
    onConfirm: (String, String, String) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf(image.name) }
    var label by rememberSaveable { mutableStateOf(image.osLabel) }
    var user by rememberSaveable { mutableStateOf(image.username) }
    val nameError = when {
        !IMAGE_NAME_RE.matches(name) -> stringResource(R.string.err_image_name_chars)
        name != image.name && name in existing -> stringResource(R.string.err_image_name_exists)
        name != image.name && running -> stringResource(R.string.err_image_rename_running, image.name)
        else -> null
    }
    val labelError = if (label.isBlank() || label.length > 64) stringResource(R.string.err_label_length) else null
    val userError = if (!USERNAME_RE.matches(user)) stringResource(R.string.err_username_chars) else null
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(AppIcons.Edit, contentDescription = null) },
        title = { Text(stringResource(R.string.image_details)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.trim() },
                    label = { Text(stringResource(R.string.field_image_name)) },
                    singleLine = true,
                    isError = nameError != null,
                    supportingText = nameError?.let { { Text(it) } },
                )
                OutlinedTextField(
                    value = label,
                    onValueChange = { label = it },
                    label = { Text(stringResource(R.string.field_distro_label)) },
                    singleLine = true,
                    isError = labelError != null,
                    supportingText = labelError?.let { { Text(it) } },
                )
                OutlinedTextField(
                    value = user,
                    onValueChange = { user = it.trim() },
                    label = { Text(stringResource(R.string.field_user_name)) },
                    singleLine = true,
                    isError = userError != null,
                    supportingText = { Text(userError ?: stringResource(R.string.details_user_hint)) },
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onConfirm(name, label.trim(), user) },
                enabled = nameError == null && labelError == null && userError == null,
            ) { Text(stringResource(R.string.action_save)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

@Composable
fun PortForwardsDialog(
    image: ImageInfo,
    onDismiss: () -> Unit,
    onConfirm: (List<PortForward>) -> Unit,
) {
    var forwards by rememberSaveable { mutableStateOf(image.portForwards.map { "${it.hostPort}:${it.guestPort}" }) }
    var hostInput by rememberSaveable { mutableStateOf("") }
    var guestInput by rememberSaveable { mutableStateOf("") }
    val parsed = forwards.mapNotNull { entry ->
        val parts = entry.split(":")
        val h = parts.getOrNull(0)?.toIntOrNull()
        val g = parts.getOrNull(1)?.toIntOrNull()
        if (h != null && g != null) PortForward(h, g) else null
    }
    val hostPort = hostInput.toIntOrNull()
    val guestPort = if (guestInput.isBlank()) hostPort else guestInput.toIntOrNull()
    val addError = when {
        hostInput.isBlank() -> null
        hostPort == null || hostPort !in 1024..65535 -> stringResource(R.string.image_port_err_host)
        guestPort == null || guestPort !in 1..65535 -> stringResource(R.string.image_port_err_guest)
        image.sshPort != 0 && hostPort == image.sshPort -> stringResource(R.string.image_port_err_ssh_conflict, image.sshPort)
        parsed.any { it.hostPort == hostPort } -> stringResource(R.string.image_port_err_duplicate, hostPort)
        parsed.size >= 32 -> stringResource(R.string.image_port_err_limit)
        else -> null
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(AppIcons.OpenInNew, contentDescription = null) },
        title = { Text(stringResource(R.string.image_port_forwarding)) },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    stringResource(R.string.image_port_forwarding_desc, image.name),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    OutlinedTextField(
                        value = hostInput,
                        onValueChange = { hostInput = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.image_port_host)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.weight(1f),
                    )
                    OutlinedTextField(
                        value = guestInput,
                        onValueChange = { guestInput = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.image_port_guest)) },
                        placeholder = { Text(hostInput.ifEmpty { "80" }) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(
                        onClick = {
                            if (hostPort != null && guestPort != null && addError == null) {
                                forwards = forwards + "$hostPort:$guestPort"
                                hostInput = ""
                                guestInput = ""
                            }
                        },
                        enabled = hostInput.isNotBlank() && addError == null,
                    ) {
                        Icon(AppIcons.Add, contentDescription = stringResource(R.string.image_port_add))
                    }
                }
                if (addError != null) {
                    Text(addError, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
                if (parsed.isEmpty()) {
                    Text(
                        stringResource(R.string.image_port_none),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    parsed.forEachIndexed { idx, fwd ->
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text("TCP ${fwd.hostPort} -> ${fwd.guestPort}", style = MaterialTheme.typography.bodyMedium)
                            IconButton(onClick = { forwards = forwards.filterIndexed { i, _ -> i != idx } }) {
                                Icon(AppIcons.Delete, contentDescription = stringResource(R.string.image_port_remove))
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(parsed) }) { Text(stringResource(R.string.action_save)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

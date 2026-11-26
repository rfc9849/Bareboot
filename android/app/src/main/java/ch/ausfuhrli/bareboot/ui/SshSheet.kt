package ch.ausfuhrli.bareboot.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.ipc.ImageInfo
import ch.ausfuhrli.bareboot.ssh.SshConfig
import ch.ausfuhrli.bareboot.ssh.SshSetup

@Composable
fun SshSheet(vm: BarebootViewModel, image: ImageInfo, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val saved = remember(image.name) { vm.sshConfig(image.name) }
    var enabled by rememberSaveable { mutableStateOf(saved.enabled) }
    var portText by rememberSaveable { mutableStateOf(saved.port.toString()) }
    var keys by rememberSaveable { mutableStateOf(saved.keys) }
    val importKey = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            val text = runCatching {
                context.contentResolver.openInputStream(uri)?.use { it.readBytes().take(64 * 1024).toByteArray().decodeToString() }
            }.getOrNull()
            if (text != null) keys = listOf(keys.trim(), text.trim()).filter { it.isNotEmpty() }.joinToString("\n")
        }
    }
    val port = portText.toIntOrNull()
    val portProblem = SshSetup.portError(context, port)
    val keysProblem = if (enabled) SshSetup.keysError(context, keys) else null
    val user = image.username.ifEmpty { "your-user" }
    val wifi = remember { SshSetup.wifiAddress(context) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        sheetGesturesEnabled = false,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(bottom = 16.dp),
        ) {
            Text(
                stringResource(R.string.ssh_title, image.name),
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            SegmentedColumn(Modifier.padding(top = 16.dp)) {
                SegmentedListItem(
                    checked = enabled,
                    onCheckedChange = { enabled = it },
                    shapes = switchRowShapes(0, 1),
                    colors = switchRowColors(),
                    supportingContent = {
                        Text(stringResource(R.string.ssh_server_desc))
                    },
                    trailingContent = { Switch(checked = enabled, onCheckedChange = null) },
                ) { Text(stringResource(R.string.ssh_server)) }
            }
            if (enabled) {
                Column(
                    Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    OutlinedTextField(
                        value = portText,
                        onValueChange = { portText = it.filter(Char::isDigit).take(5) },
                        label = { Text(stringResource(R.string.ssh_port)) },
                        singleLine = true,
                        isError = portProblem != null,
                        supportingText = portProblem?.let { { Text(it) } },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = keys,
                        onValueChange = { keys = it },
                        label = { Text(stringResource(R.string.ssh_authorized_keys)) },
                        placeholder = { Text(stringResource(R.string.ssh_keys_placeholder)) },
                        isError = keysProblem != null,
                        supportingText = { Text(keysProblem ?: stringResource(R.string.ssh_keys_hint)) },
                        textStyle = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(min = 120.dp),
                    )
                    OutlinedButton(onClick = { importKey.launch(arrayOf("*/*")) }) {
                        Text(stringResource(R.string.ssh_import_pub))
                    }
                }
                if (port != null && portProblem == null) {
                    SectionHeader(stringResource(R.string.ssh_section_connect))
                    val wifiLabel = stringResource(R.string.ssh_connect_wifi)
                    val usbLabel = stringResource(R.string.ssh_connect_usb, port)
                    val commands = buildList {
                        wifi?.let { add(wifiLabel to "ssh -p $port $user@$it") }
                        add(usbLabel to "ssh -p $port $user@127.0.0.1")
                    }
                    SegmentedColumn {
                        commands.forEachIndexed { index, (label, command) ->
                            SegmentedListItem(
                                shapes = ListItemDefaults.segmentedShapes(index, commands.size),
                                colors = listRowColors(),
                                overlineContent = { Text(label) },
                                trailingContent = {
                                    IconButton(onClick = { copy(context, command) }) {
                                        Icon(AppIcons.ContentPaste, contentDescription = stringResource(R.string.action_copy))
                                    }
                                },
                            ) {
                                Text(command, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
                            }
                        }
                    }
                    Text(
                        if (wifi == null) {
                            stringResource(R.string.ssh_wifi_needed)
                        } else {
                            stringResource(R.string.ssh_wifi_note)
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 32.dp, vertical = 8.dp),
                    )
                }
            }
            Text(
                stringResource(R.string.ssh_save_note),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 8.dp),
            )
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End),
            ) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
                Button(
                    onClick = {
                        vm.applySsh(image.name, SshConfig(enabled, port ?: SshSetup.DEFAULT_PORT, keys))
                        onDismiss()
                    },
                    enabled = portProblem == null && keysProblem == null && !(enabled == saved.enabled && !enabled),
                ) { Text(stringResource(if (enabled) R.string.ssh_save_and_start else R.string.action_save)) }
            }
        }
    }
}

private fun copy(context: Context, text: String) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    cm.setPrimaryClip(ClipData.newPlainText(context.getString(R.string.ssh_clipboard_label), text))
}

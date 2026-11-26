package ch.ausfuhrli.bareboot.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.Screen
import kotlinx.coroutines.launch

private data class LogSource(val title: String, val distro: String, val name: String)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DiagnosticsScreen(vm: BarebootViewModel) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val running = vm.status?.distros?.map { it.name }.orEmpty()
    val sources = listOf(LogSource(stringResource(R.string.diagnostics_source_service), "", "daemon")) + running.flatMap { name ->
        listOf(
            LogSource(stringResource(R.string.diagnostics_source_autostart, name), name, "autostart"),
            LogSource(stringResource(R.string.diagnostics_source_ssh, name), name, "sshd"),
        )
    }
    var selected by rememberSaveable { mutableIntStateOf(0) }
    var reload by rememberSaveable { mutableIntStateOf(0) }
    var text by rememberSaveable { mutableStateOf("") }
    val source = sources.getOrElse(selected) { sources.first() }
    val loadingText = stringResource(R.string.diagnostics_loading)
    LaunchedEffect(source, reload) {
        text = loadingText
        text = vm.readLog(source.distro, source.name)
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.diagnostics_title)) },
                navigationIcon = {
                    IconButton(onClick = { vm.screen = Screen.Settings }) {
                        Icon(AppIcons.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                actions = {
                    IconButton(onClick = { reload++ }) {
                        Icon(AppIcons.Refresh, contentDescription = stringResource(R.string.action_reload))
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            LazyRow(
                contentPadding = PaddingValues(horizontal = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(sources.size) { index ->
                    FilterChip(
                        selected = index == selected,
                        onClick = { selected = index },
                        label = { Text(sources[index].title) },
                    )
                }
            }
            if (running.isEmpty()) {
                Text(
                    stringResource(R.string.diagnostics_start_hint),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
            Surface(
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                shape = MaterialTheme.shapes.large,
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp),
            ) {
                SelectionContainer {
                    Text(
                        text,
                        fontFamily = FontFamily.Monospace,
                        fontSize = 11.sp,
                        lineHeight = 14.sp,
                        modifier = Modifier
                            .verticalScroll(rememberScrollState(Int.MAX_VALUE))
                            .horizontalScroll(rememberScrollState())
                            .padding(12.dp),
                    )
                }
            }
            Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = {
                        scope.launch {
                            val logs = sources.associate { it.title to vm.readLog(it.distro, it.name) }
                            copy(context, vm.diagnosticsReport(logs))
                            vm.messages.trySend(context.getString(R.string.diagnostics_report_copied))
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.diagnostics_copy_report)) }
                OutlinedButton(
                    onClick = { copy(context, text) },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(stringResource(R.string.diagnostics_copy_log)) }
                Text(
                    stringResource(R.string.diagnostics_privacy_note),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

private fun copy(context: Context, text: String) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    cm.setPrimaryClip(ClipData.newPlainText("Bareboot", text))
}

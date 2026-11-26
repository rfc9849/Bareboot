package ch.ausfuhrli.bareboot.ui

import android.content.ActivityNotFoundException
import android.content.Intent
import android.provider.DocumentsContract
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.ListItemColors
import androidx.compose.material3.RadioButton
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeFlexibleTopAppBar
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberSliderState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import ch.ausfuhrli.bareboot.DEFAULT_EXTRA_CHARS
import ch.ausfuhrli.bareboot.DEFAULT_EXTRA_KEYS
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.MAX_FONT_SP
import ch.ausfuhrli.bareboot.MIN_FONT_SP
import ch.ausfuhrli.bareboot.Screen
import ch.ausfuhrli.bareboot.ServiceState

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsScreen(vm: BarebootViewModel) {
    val context = LocalContext.current
    val scrollBehavior = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        if (tree != null) vm.chooseSharedFolder(tree)
    }
    val version = remember {
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull()
    }
    var showExtraKeys by rememberSaveable { mutableStateOf(false) }
    var showThemes by rememberSaveable { mutableStateOf(false) }
    var showFonts by rememberSaveable { mutableStateOf(false) }
    val fontPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) vm.importFont(uri)
    }
    if (showThemes) {
        ThemeListDialog(vm, onDismiss = { showThemes = false })
    }
    if (showExtraKeys) {
        ExtraKeysDialog(vm, onDismiss = { showExtraKeys = false })
    }
    if (showFonts) {
        FontDialog(
            vm,
            onImport = {
                showFonts = false
                fontPicker.launch(arrayOf("font/*", "application/x-font-ttf", "application/octet-stream"))
            },
            onDismiss = { showFonts = false },
        )
    }
    val colors = switchRowColors()
    val fontFamily = remember(vm.importedFont, vm.useBundledFont) { FontFamily(vm.resolveTypeface()) }

    Scaffold(
        modifier = Modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            LargeFlexibleTopAppBar(
                title = { Text(stringResource(R.string.label_settings)) },
                navigationIcon = {
                    IconButton(onClick = vm::closeSettings) {
                        Icon(AppIcons.ArrowBack, contentDescription = stringResource(R.string.action_back))
                    }
                },
                scrollBehavior = scrollBehavior,
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.readableWidth(),
            contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = padding.calculateBottomPadding() + 24.dp),
        ) {
            item { SectionHeader(stringResource(R.string.settings_section_terminal)) }
            item { TerminalPreview(vm, fontFamily) }
            item { ThemePicker(vm, fontFamily, onMore = { showThemes = true }) }
            item {
                SegmentedColumn(Modifier.padding(top = 12.dp)) {
                    SegmentedListItem(
                        onClick = { showFonts = true },
                        colors = colors,
                        shapes = switchRowShapes(0, 3),
                        leadingContent = { Icon(AppIcons.TextFields, contentDescription = null) },
                        supportingContent = { Text(fontLabel(vm.importedFont, vm.useBundledFont)) },
                    ) { Text(stringResource(R.string.settings_font)) }
                    TextSizeItem(vm, colors)
                    SegmentedListItem(
                        onClick = { showExtraKeys = true },
                        colors = colors,
                        shapes = switchRowShapes(2, 3),
                        leadingContent = { Icon(AppIcons.Edit, contentDescription = null) },
                        supportingContent = {
                            Text((vm.extraKeys.map { extraKeyLabel(context, it) } + vm.extraChars.map { it.toString() }).joinToString(" "))
                        },
                    ) { Text(stringResource(R.string.settings_extra_keys)) }
                }
            }

            item { SectionHeader(stringResource(R.string.settings_section_startup)) }
            item {
                SegmentedColumn {
                    SegmentedListItem(
                        checked = vm.autoStart,
                        onCheckedChange = vm::changeAutoStart,
                        colors = colors,
                        shapes = switchRowShapes(0, 1),
                        leadingContent = { Icon(AppIcons.PlayCircle, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.settings_autostart_desc)) },
                        trailingContent = { Switch(checked = vm.autoStart, onCheckedChange = null) },
                    ) { Text(stringResource(R.string.settings_autostart)) }
                }
            }

            item { SectionHeader(stringResource(R.string.settings_section_shared_folder)) }
            item {
                val count = if (vm.filesConnected) 4 else 3
                SegmentedColumn {
                    SegmentedListItem(
                        checked = vm.shareEnabled,
                        onCheckedChange = vm::changeShareEnabled,
                        colors = colors,
                        shapes = switchRowShapes(0, count),
                        leadingContent = { Icon(AppIcons.FolderShared, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.settings_share_folder_desc)) },
                        trailingContent = { Switch(checked = vm.shareEnabled, onCheckedChange = null) },
                    ) { Text(stringResource(R.string.settings_share_folder)) }
                    SegmentedListItem(
                        onClick = { picker.launch(vm.folderPickerLocation()) },
                        enabled = vm.shareEnabled,
                        colors = colors,
                        shapes = switchRowShapes(1, count),
                        leadingContent = { Icon(AppIcons.Folder, contentDescription = null) },
                        supportingContent = {
                            Text(vm.sharedFolderName?.let { stringResource(R.string.settings_internal_storage_path, it) } ?: vm.sharedDir)
                        },
                    ) { Text(stringResource(R.string.settings_folder)) }
                    SegmentedListItem(
                        checked = vm.filesConnected,
                        onCheckedChange = { on ->
                            if (on) picker.launch(vm.folderPickerLocation()) else vm.disconnectFiles()
                        },
                        enabled = vm.shareEnabled,
                        colors = colors,
                        shapes = switchRowShapes(2, count),
                        leadingContent = { Icon(AppIcons.Visibility, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.settings_show_in_files_desc)) },
                        trailingContent = {
                            Switch(checked = vm.filesConnected, onCheckedChange = null, enabled = vm.shareEnabled)
                        },
                    ) { Text(stringResource(R.string.settings_show_in_files)) }
                    if (vm.filesConnected) {
                        SegmentedListItem(
                            onClick = {
                                val root = vm.filesRootUri() ?: return@SegmentedListItem
                                try {
                                    context.startActivity(
                                        Intent(Intent.ACTION_VIEW)
                                            .setDataAndType(root, DocumentsContract.Root.MIME_TYPE_ITEM)
                                    )
                                } catch (_: ActivityNotFoundException) {
                                    vm.messages.trySend(context.getString(R.string.settings_files_open_failed))
                                }
                            },
                            colors = colors,
                            shapes = switchRowShapes(3, count),
                            leadingContent = { Icon(AppIcons.OpenInNew, contentDescription = null) },
                        ) { Text(stringResource(R.string.settings_open_in_files)) }
                    }
                }
            }

            item { SectionHeader(stringResource(R.string.settings_section_service)) }
            item {
                val status = vm.status
                val ready = status != null
                SegmentedColumn {
                    SegmentedListItem(
                        checked = status?.relaxedSync == true,
                        onCheckedChange = { on ->
                            status?.let { vm.setDaemonOptions(it.purgeOnUninstall, on) }
                        },
                        enabled = ready && !vm.busy,
                        colors = colors,
                        shapes = switchRowShapes(0, 3),
                        leadingContent = { Icon(AppIcons.Storage, contentDescription = null) },
                        supportingContent = {
                            Text(stringResource(R.string.settings_relaxed_sync_desc))
                        },
                        trailingContent = {
                            Switch(checked = status?.relaxedSync == true, onCheckedChange = null, enabled = ready)
                        },
                    ) { Text(stringResource(R.string.settings_relaxed_sync)) }
                    SegmentedListItem(
                        checked = status?.purgeOnUninstall == true,
                        onCheckedChange = { on ->
                            status?.let { vm.setDaemonOptions(on, it.relaxedSync) }
                        },
                        enabled = ready && !vm.busy,
                        colors = colors,
                        shapes = switchRowShapes(1, 3),
                        leadingContent = { Icon(AppIcons.Delete, contentDescription = null) },
                        supportingContent = {
                            Text(stringResource(R.string.settings_purge_on_uninstall_desc))
                        },
                        trailingContent = {
                            Switch(checked = status?.purgeOnUninstall == true, onCheckedChange = null, enabled = ready)
                        },
                    ) { Text(stringResource(R.string.settings_purge_on_uninstall)) }
                    SegmentedListItem(
                        onClick = { vm.screen = Screen.Diagnostics },
                        colors = colors,
                        shapes = switchRowShapes(2, 3),
                        leadingContent = { Icon(AppIcons.HelpOutline, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.settings_diagnostics_desc)) },
                    ) { Text(stringResource(R.string.settings_diagnostics)) }
                }
            }

            item { SectionHeader(stringResource(R.string.settings_section_system)) }
            item {
                SegmentedColumn {
                    SegmentedListItem(
                        onClick = { vm.screen = Screen.SystemCheck },
                        colors = colors,
                        shapes = switchRowShapes(0, 3),
                        leadingContent = { Icon(AppIcons.Memory, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.settings_system_check_desc)) },
                    ) { Text(stringResource(R.string.settings_system_check)) }
                    SegmentedListItem(
                        onClick = vm::restartOnboarding,
                        colors = colors,
                        shapes = switchRowShapes(1, 3),
                        leadingContent = { Icon(AppIcons.Restore, contentDescription = null) },
                    ) { Text(stringResource(R.string.settings_run_setup_again)) }
                    SegmentedListItem(
                        colors = colors,
                        shapes = switchRowShapes(2, 3),
                        leadingContent = { Icon(AppIcons.Info, contentDescription = null) },
                        supportingContent = {
                            val service = when (val s = vm.service) {
                                is ServiceState.Ready -> stringResource(R.string.settings_service_protocol, s.version)
                                ServiceState.NotRunning -> stringResource(R.string.home_service_not_running)
                                ServiceState.Unauthorized -> stringResource(R.string.home_service_unauthorized)
                                is ServiceState.Failed -> stringResource(R.string.home_service_error)
                                ServiceState.Checking -> stringResource(R.string.settings_service_connecting)
                            }
                            Text(service)
                        },
                    ) { Text(stringResource(R.string.settings_version, version.orEmpty())) }
                }
            }
        }
    }
}

@Composable
private fun ThemePicker(vm: BarebootViewModel, fontFamily: FontFamily, onMore: () -> Unit) {
    LazyRow(
        modifier = Modifier.padding(top = 12.dp),
        contentPadding = PaddingValues(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val current = vm.currentTheme
        val shown = if (TerminalThemes.all.any { it.id == current.id }) TerminalThemes.all else TerminalThemes.all + current
        items(shown, key = { it.id }) { theme ->
            ThemeCard(theme, fontFamily, selected = theme.id == vm.themeId, onClick = { vm.selectTheme(theme.id) })
        }
        item(key = "more") {
            Surface(
                onClick = onMore,
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                shape = MaterialTheme.shapes.large,
                modifier = Modifier.width(148.dp),
            ) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("800+", style = MaterialTheme.typography.titleLarge)
                    Text(stringResource(R.string.settings_more_themes), style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

@Composable
private fun ThemeCard(theme: TerminalTheme, fontFamily: FontFamily, selected: Boolean, onClick: () -> Unit) {
    Surface(
        onClick = onClick,
        color = theme.bgColor,
        shape = MaterialTheme.shapes.large,
        border = if (selected) BorderStroke(3.dp, MaterialTheme.colorScheme.primary) else null,
        modifier = Modifier.width(148.dp),
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            val fg = Color(theme.foreground)
            Text(
                buildAnnotatedString {
                    withStyle(SpanStyle(color = Color(theme.ansi16[2]))) { append("user") }
                    withStyle(SpanStyle(color = fg)) { append(":") }
                    withStyle(SpanStyle(color = Color(theme.ansi16[4]))) { append("~") }
                    withStyle(SpanStyle(color = fg)) { append("$ ls\n") }
                    withStyle(SpanStyle(color = Color(theme.ansi16[4]))) { append("bin ") }
                    withStyle(SpanStyle(color = Color(theme.ansi16[5]))) { append("etc ") }
                    withStyle(SpanStyle(color = Color(theme.ansi16[1]))) { append("tmp") }
                },
                fontFamily = fontFamily,
                fontSize = 12.sp,
                lineHeight = 16.sp,
                maxLines = 2,
                softWrap = false,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    theme.label,
                    color = fg,
                    style = MaterialTheme.typography.labelLarge,
                    maxLines = 1,
                    modifier = Modifier.weight(1f),
                )
                if (selected) {
                    Icon(
                        AppIcons.CheckCircle,
                        contentDescription = stringResource(R.string.label_selected),
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.size(18.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun ThemeListDialog(vm: BarebootViewModel, onDismiss: () -> Unit) {
    val names = remember { vm.ghosttyThemeNames() }
    var query by rememberSaveable { mutableStateOf("") }
    val filtered = names.filter { it.contains(query, ignoreCase = true) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_ghostty_themes)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.settings_search_themes, names.size)) },
                )
                LazyColumn(Modifier.height(360.dp)) {
                    items(filtered, key = { it }) { name ->
                        val theme = remember(name) { vm.previewTheme(name) }
                        Surface(
                            onClick = {
                                vm.selectTheme(theme.id)
                                onDismiss()
                            },
                            color = theme.bgColor,
                            shape = MaterialTheme.shapes.small,
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(vertical = 2.dp),
                        ) {
                            Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(name, color = Color(theme.foreground), modifier = Modifier.weight(1f), maxLines = 1)
                                (1..6).forEach { i ->
                                    Box(
                                        Modifier
                                            .padding(start = 3.dp)
                                            .size(10.dp)
                                            .background(Color(theme.ansi16[i]), MaterialTheme.shapes.extraSmall),
                                    )
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close)) } },
    )
}

@Composable
private fun TextSizeItem(vm: BarebootViewModel, colors: ListItemColors) {
    val sliderState = rememberSliderState(
        value = vm.fontSizeSp,
        trackRange = MIN_FONT_SP..MAX_FONT_SP,
    )
    LaunchedEffect(vm.fontSizeSp) {
        if (sliderState.value.roundToInt().toFloat() != vm.fontSizeSp) sliderState.value = vm.fontSizeSp
    }
    SegmentedListItem(
        colors = colors,
        shapes = switchRowShapes(1, 3),
        leadingContent = { Icon(AppIcons.FormatSize, contentDescription = null) },
        supportingContent = {
            Slider(
                state = sliderState,
                onValueChange = {
                    sliderState.value = it
                    vm.setFontSize(it.roundToInt().toFloat())
                },
                modifier = Modifier.fillMaxWidth(),
            )
        },
    ) { Text(stringResource(R.string.settings_text_size, vm.fontSizeSp.roundToInt())) }
}

@Composable
private fun fontLabel(imported: String?, bundled: Boolean): String =
    imported ?: if (bundled) "Google Sans Code" else stringResource(R.string.settings_font_system)

@Composable
private fun TerminalPreview(vm: BarebootViewModel, fontFamily: FontFamily) {
    val theme = vm.currentTheme
    val fg = Color(theme.foreground)
    val green = Color(theme.ansi16[2])
    val blue = Color(theme.ansi16[4])
    val text = buildAnnotatedString {
        fun prompt() {
            withStyle(SpanStyle(color = green)) { append("user@bareboot") }
            withStyle(SpanStyle(color = fg)) { append(":") }
            withStyle(SpanStyle(color = blue)) { append("~") }
            withStyle(SpanStyle(color = fg)) { append("$ ") }
        }
        prompt()
        withStyle(SpanStyle(color = fg)) { append("ls\n") }
        withStyle(SpanStyle(color = blue)) { append("bin  etc  home  usr") }
        withStyle(SpanStyle(color = fg)) { append("  notes.txt\n") }
        prompt()
        withStyle(SpanStyle(background = Color(theme.cursor))) { append(" ") }
    }
    val lineHeight = vm.fontSizeSp * 1.25f
    val height = with(LocalDensity.current) { (MAX_FONT_SP * 1.25f * 3).sp.toDp() } + 32.dp
    Surface(
        color = theme.bgColor,
        shape = MaterialTheme.shapes.large,
        modifier = Modifier
            .padding(horizontal = 16.dp)
            .fillMaxWidth()
            .height(height),
    ) {
        Text(
            text,
            fontFamily = fontFamily,
            fontSize = vm.fontSizeSp.sp,
            lineHeight = lineHeight.sp,
            softWrap = false,
            maxLines = 3,
            overflow = TextOverflow.Clip,
            modifier = Modifier.padding(16.dp),
        )
    }
}

@Composable
private fun FontDialog(vm: BarebootViewModel, onImport: () -> Unit, onDismiss: () -> Unit) {
    val imported = remember { vm.importedFontNames() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_font)) },
        text = {
            Column(Modifier.selectableGroup()) {
                FontOption(
                    label = fontLabel(null, true),
                    family = remember { FontFamily(vm.typefaceFor(null, true)) },
                    selected = vm.importedFont == null && vm.useBundledFont,
                ) {
                    vm.setFontChoice(true)
                    onDismiss()
                }
                FontOption(
                    label = fontLabel(null, false),
                    family = FontFamily.Monospace,
                    selected = vm.importedFont == null && !vm.useBundledFont,
                ) {
                    vm.setFontChoice(false)
                    onDismiss()
                }
                imported.forEach { name ->
                    FontOption(
                        label = name,
                        family = remember(name) { FontFamily(vm.typefaceFor(name, false)) },
                        selected = vm.importedFont == name,
                    ) {
                        vm.selectImportedFont(name)
                        onDismiss()
                    }
                }
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 56.dp)
                        .clickable(role = Role.Button, onClick = onImport)
                        .padding(horizontal = 12.dp),
                ) {
                    Icon(AppIcons.UploadFile, contentDescription = null)
                    Spacer(Modifier.width(16.dp))
                    Text(stringResource(R.string.settings_font_import), style = MaterialTheme.typography.bodyLarge)
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close)) } },
    )
}

@Composable
private fun FontOption(label: String, family: FontFamily, selected: Boolean, onClick: () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = 12.dp),
    ) {
        RadioButton(selected = selected, onClick = null)
        Spacer(Modifier.width(16.dp))
        Text(label, fontFamily = family, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun ExtraKeysDialog(vm: BarebootViewModel, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var selected by rememberSaveable { mutableStateOf(vm.extraKeys) }
    var chars by rememberSaveable { mutableStateOf(vm.extraChars) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.settings_extra_keys)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    EXTRA_KEY_IDS.forEach { id ->
                        FilterChip(
                            selected = id in selected,
                            onClick = {
                                selected = if (id in selected) {
                                    selected - id
                                } else {
                                    EXTRA_KEY_IDS.filter { it in selected || it == id }
                                }
                            },
                            label = { Text(extraKeyLabel(context, id)) },
                        )
                    }
                }
                OutlinedTextField(
                    value = chars,
                    onValueChange = { chars = it.filterNot(Char::isWhitespace).take(32) },
                    label = { Text(stringResource(R.string.settings_extra_chars)) },
                    supportingText = { Text(stringResource(R.string.settings_extra_chars_hint)) },
                    singleLine = true,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = {
                vm.setExtraKeys(selected, chars)
                onDismiss()
            }) { Text(stringResource(R.string.action_save)) }
        },
        dismissButton = {
            TextButton(onClick = {
                selected = DEFAULT_EXTRA_KEYS
                chars = DEFAULT_EXTRA_CHARS
            }) { Text(stringResource(R.string.action_reset)) }
        },
    )
}

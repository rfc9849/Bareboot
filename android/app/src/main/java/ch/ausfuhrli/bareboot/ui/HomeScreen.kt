package ch.ausfuhrli.bareboot.ui

import android.hardware.display.DisplayManager
import android.view.Display
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LargeFlexibleTopAppBar
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ProgressIndicatorDefaults
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.rememberSliderState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import android.content.Context
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.Screen
import ch.ausfuhrli.bareboot.ServiceState
import ch.ausfuhrli.bareboot.install.InstallService
import ch.ausfuhrli.bareboot.ipc.ImageInfo
import ch.ausfuhrli.bareboot.terminal.TerminalTab
import kotlinx.coroutines.delay

private val NAME_RE = Regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun HomeScreen(vm: BarebootViewModel) {
    var showAddImage by rememberSaveable { mutableStateOf(false) }
    var settingsTarget by rememberSaveable { mutableStateOf<String?>(null) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                vm.refresh()
                delay(3000)
            }
        }
    }
    val scrollBehavior = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    val selected = vm.selectedImage
    val ready = vm.service is ServiceState.Ready
    val cards = listOfNotNull(selected) + vm.images.filter { it.name != selected?.name && vm.isRunning(it.name) }
    val otherImages = vm.images.filter { image -> cards.none { it.name == image.name } }

    Scaffold(
        modifier = Modifier.nestedScroll(scrollBehavior.nestedScrollConnection),
        topBar = {
            LargeFlexibleTopAppBar(
                title = { Text(stringResource(R.string.app_name)) },
                subtitle = { Text(serviceSummary(LocalContext.current, vm)) },
                actions = {
                    IconButton(onClick = vm::openSettings) {
                        Icon(AppIcons.Settings, contentDescription = stringResource(R.string.label_settings))
                    }
                },
                scrollBehavior = scrollBehavior,
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.readableWidth(),
            contentPadding = PaddingValues(
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + 24.dp,
            ),
        ) {
            if (!ready && vm.service != ServiceState.Checking) {
                item { ServiceProblemCard(vm) }
            }
            vm.install?.let { progress ->
                item {
                    InstallProgressCard(
                        progress,
                        Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                        onCancel = if (progress.unpacking) null else vm::cancelInstall,
                    )
                }
            }
            items(cards, key = { "card-${it.name}" }) { image ->
                DistroCard(
                    vm = vm,
                    image = image,
                    onSettings = { settingsTarget = image.name },
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
            if (cards.isEmpty() && ready && vm.install == null) {
                item { EmptyCard(onInstall = { showAddImage = true }) }
            }
            if (vm.tabs.isNotEmpty()) {
                item { SectionHeader(stringResource(R.string.home_section_open_shells)) }
                itemsIndexed(vm.tabs, key = { _, tab -> tab.id }) { index, tab ->
                    TabRow(vm, tab, index)
                }
            }
            if (vm.images.isNotEmpty()) {
                if (otherImages.isNotEmpty()) item { SectionHeader(stringResource(R.string.home_section_images)) }
                val count = otherImages.size + 1
                itemsIndexed(otherImages, key = { _, img -> img.name }) { index, img ->
                    ImageRow(vm, img, index, count)
                }
                item {
                    SegmentedColumn(Modifier.padding(top = ListItemDefaults.SegmentedGap)) {
                        SegmentedListItem(
                            onClick = { showAddImage = true },
                            enabled = ready && vm.install == null,
                            shapes = ListItemDefaults.segmentedShapes(count - 1, count),
                            colors = listRowColors(),
                            leadingContent = { Icon(AppIcons.Add, contentDescription = null) },
                        ) {
                            Text(stringResource(R.string.home_add_distribution))
                        }
                    }
                }
            }
        }
    }

    if (showAddImage) {
        AddDistroSheet(vm, onDismiss = { showAddImage = false })
    }
    ImageSettingsHost(vm, settingsTarget, onDismiss = { settingsTarget = null })
}


@Composable
fun ImageSettingsHost(vm: BarebootViewModel, target: String?, onDismiss: () -> Unit) {
    var enlargeTarget by rememberSaveable { mutableStateOf<String?>(null) }
    var deleteTarget by rememberSaveable { mutableStateOf<String?>(null) }
    var sshTarget by rememberSaveable { mutableStateOf<String?>(null) }
    var forwardsTarget by rememberSaveable { mutableStateOf<String?>(null) }
    var detailsTarget by rememberSaveable { mutableStateOf<String?>(null) }
    vm.image(target.orEmpty())?.let { image ->
        ImageSettingsSheet(
            vm = vm,
            image = image,
            onDismiss = onDismiss,
            onSsh = { sshTarget = image.name },
            onPortForwards = { forwardsTarget = image.name },
            onDetails = { detailsTarget = image.name },
            onEnlarge = { enlargeTarget = image.name },
            onDelete = { deleteTarget = image.name },
        )
    }
    vm.image(deleteTarget.orEmpty())?.let { image ->
        DeleteDialog(
            image = image,
            onDismiss = { deleteTarget = null },
            onConfirm = {
                deleteTarget = null
                vm.deleteImage(image.name)
            },
        )
    }
    vm.image(sshTarget.orEmpty())?.let { image ->
        SshSheet(vm, image, onDismiss = { sshTarget = null })
    }
    vm.image(forwardsTarget.orEmpty())?.let { image ->
        PortForwardsDialog(
            image = image,
            onDismiss = { forwardsTarget = null },
            onConfirm = { forwards ->
                forwardsTarget = null
                vm.setPortForwards(image, forwards)
            },
        )
    }
    vm.image(detailsTarget.orEmpty())?.let { image ->
        DetailsDialog(
            image = image,
            running = vm.isRunning(image.name),
            existing = vm.images.map { it.name },
            onDismiss = { detailsTarget = null },
            onConfirm = { name, label, user ->
                detailsTarget = null
                vm.updateDetails(image, name, label, user)
            },
        )
    }
    vm.image(enlargeTarget.orEmpty())?.let { image ->
        EnlargeDialog(
            image = image,
            onDismiss = { enlargeTarget = null },
            onConfirm = { size ->
                enlargeTarget = null
                vm.resizeImage(image.name, size)
            },
        )
    }
}

private fun serviceSummary(context: Context, vm: BarebootViewModel): String = when (vm.service) {
    ServiceState.Checking -> context.getString(R.string.home_connecting)
    ServiceState.NotRunning -> context.getString(R.string.home_service_not_running)
    ServiceState.Unauthorized -> context.getString(R.string.home_service_unauthorized)
    is ServiceState.Failed -> context.getString(R.string.home_service_error)
    is ServiceState.Ready -> {
        val running = vm.status?.distros?.size ?: 0
        if (running == 0) context.getString(R.string.home_no_distro_running)
        else context.getString(R.string.home_distros_running, running)
    }
}

@Composable
private fun ServiceProblemCard(vm: BarebootViewModel) {
    val colors = MaterialTheme.colorScheme
    Surface(
        color = colors.errorContainer,
        contentColor = colors.onErrorContainer,
        shape = MaterialTheme.shapes.extraLarge,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(AppIcons.ErrorOutline, contentDescription = null)
                Text(
                    when (val s = vm.service) {
                        ServiceState.NotRunning -> stringResource(R.string.home_service_not_running_detail)
                        ServiceState.Unauthorized -> stringResource(R.string.home_service_unauthorized_detail)
                        is ServiceState.Failed -> s.message
                        else -> ""
                    },
                    style = MaterialTheme.typography.titleMedium,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End), modifier = Modifier.fillMaxWidth()) {
                TextButton(onClick = vm::restartOnboarding) { Text(stringResource(R.string.home_setup_help)) }
                Button(onClick = vm::refreshAsync) { Text(stringResource(R.string.action_check_again)) }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun EmptyCard(onInstall: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(
        color = colors.surfaceContainerHigh,
        shape = MaterialTheme.shapes.extraLarge,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Column(
            Modifier.padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            ShapeIcon(MaterialShapes.Sunny, AppIcons.Storage, colors.primaryContainer, colors.onPrimaryContainer, 72.dp)
            Text(stringResource(R.string.home_empty_title), style = MaterialTheme.typography.titleLarge)
            Button(onClick = onInstall) { Text(stringResource(R.string.home_choose_distribution)) }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun DistroCard(
    vm: BarebootViewModel,
    image: ImageInfo,
    onSettings: () -> Unit,
    modifier: Modifier,
) {
    val colors = MaterialTheme.colorScheme
    val running = vm.isRunning(image.name)
    val shells = vm.tabs.count { it.distro == image.name }
    val context = LocalContext.current
    val busyLabel = busyText(context, image)
    val working = busyLabel != null || image.name in vm.pendingJobs
    Surface(
        color = if (running) colors.primaryContainer else colors.surfaceContainerHigh,
        contentColor = if (running) colors.onPrimaryContainer else colors.onSurface,
        shape = MaterialTheme.shapes.extraLarge,
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                ShapeIcon(
                    shape = if (running) MaterialShapes.Cookie9Sided else MaterialShapes.Square,
                    icon = AppIcons.Terminal,
                    container = if (running) colors.primary else colors.secondaryContainer,
                    content = if (running) colors.onPrimary else colors.onSecondaryContainer,
                    size = 56.dp,
                )
                Column(Modifier.weight(1f)) {
                    Text(image.name, style = MaterialTheme.typography.headlineSmall)
                    val state = when {
                        busyLabel != null -> busyLabel
                        !running -> stringResource(R.string.home_state_stopped)
                        shells == 1 -> stringResource(R.string.home_state_running_one_shell)
                        shells > 1 -> stringResource(R.string.home_state_running_shells, shells)
                        else -> stringResource(R.string.home_state_running)
                    }
                    Text(
                        listOfNotNull(image.osLabel.ifEmpty { null }, state).joinToString(" · "),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                IconButton(onClick = onSettings) {
                    Icon(AppIcons.Settings, contentDescription = stringResource(R.string.home_settings_for_image, image.name))
                }
            }
            if (working) {
                LinearWavyProgressIndicator(modifier = Modifier.fillMaxWidth())
            } else {
                StorageUsage(image)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = { vm.openTerminal(image.name) },
                    enabled = !vm.busy && !working && vm.service is ServiceState.Ready,
                    contentPadding = ButtonDefaults.MediumContentPadding,
                    modifier = Modifier.weight(1f),
                ) {
                    Icon(
                        if (running) AppIcons.Terminal else AppIcons.PlayArrow,
                        contentDescription = null,
                        modifier = Modifier.size(ButtonDefaults.MediumIconSize),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(if (running) R.string.home_open_terminal else R.string.home_start_and_open_terminal))
                }
                FilledTonalButton(
                    onClick = { vm.openScreen(image.name, externalDisplayId(context)) },
                    enabled = !vm.busy && !working && vm.service is ServiceState.Ready,
                    contentPadding = ButtonDefaults.MediumContentPadding,
                ) {
                    Icon(
                        AppIcons.Desktop,
                        contentDescription = stringResource(R.string.home_open_desktop, image.name),
                        modifier = Modifier.size(ButtonDefaults.MediumIconSize),
                    )
                }
                if (running) {
                    FilledTonalButton(
                        onClick = { vm.requestStopDistro(image.name) },
                        enabled = !vm.busy,
                        contentPadding = ButtonDefaults.MediumContentPadding,
                    ) {
                        Text(stringResource(R.string.action_stop))
                    }
                }
            }
        }
    }
}

private fun externalDisplayId(context: Context): Int? =
    context.getSystemService(DisplayManager::class.java).displays.firstOrNull {
        it.displayId != Display.DEFAULT_DISPLAY && it.flags and Display.FLAG_PRIVATE == 0
    }?.displayId

fun busyText(context: Context, image: ImageInfo): String? = when (image.busy) {
    "" -> null
    "export" -> context.getString(R.string.home_busy_export)
    "resize" -> context.getString(R.string.home_busy_resize)
    "create" -> context.getString(R.string.home_busy_create)
    else -> context.getString(R.string.home_busy_other)
}

@Composable
private fun StorageUsage(image: ImageInfo) {
    val colors = MaterialTheme.colorScheme
    val known = image.fsSizeBytes > 0
    val total = if (known) image.fsSizeBytes else image.sizeBytes
    val used = if (known) (image.fsSizeBytes - image.fsFreeBytes).coerceAtLeast(0) else image.usedBytes
    val free = if (known) image.fsFreeBytes else (image.sizeBytes - image.usedBytes).coerceAtLeast(0)
    val full = known && free < 64L * 1024 * 1024
    val low = known && (free < total / 10 || free < 1024L * 1024 * 1024)
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        LinearProgressIndicator(
            progress = { if (total > 0) used.toFloat() / total else 0f },
            color = if (low) colors.error else ProgressIndicatorDefaults.linearColor,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            stringResource(R.string.home_storage_used, formatBytes(used), formatBytes(total)),
            style = MaterialTheme.typography.bodySmall,
        )
        if (full) {
            Text(
                stringResource(R.string.home_storage_full, image.name),
                style = MaterialTheme.typography.bodySmall,
                color = colors.error,
            )
        } else if (low) {
            Text(
                stringResource(R.string.home_storage_low, formatBytes(free), image.name),
                style = MaterialTheme.typography.bodySmall,
                color = colors.error,
            )
        }
    }
}

@Composable
private fun TabRow(vm: BarebootViewModel, tab: TerminalTab, index: Int) {
    SegmentedColumn(Modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = { vm.showTab(tab.id) },
            shapes = ListItemDefaults.segmentedShapes(index, vm.tabs.size),
            colors = listRowColors(),
            leadingContent = {
                Icon(
                    if (tab.uid == 0) AppIcons.AdminPanelSettings else AppIcons.Terminal,
                    contentDescription = null,
                )
            },
            supportingContent = { Text(if (tab.uid == 0) stringResource(R.string.home_tab_root_subtitle, tab.distro) else tab.distro) },
            trailingContent = {
                IconButton(onClick = { vm.closeTab(tab) }) {
                    Icon(AppIcons.Close, contentDescription = stringResource(R.string.terminal_close_shell))
                }
            },
        ) {
            Text(tabLabel(tab, index, vm.tabNames[tab.id]), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun ImageRow(vm: BarebootViewModel, img: ImageInfo, index: Int, count: Int) {
    val selected = img.name == vm.distroName
    val running = vm.status?.distros?.any { it.name == img.name } == true
    SegmentedColumn(Modifier.padding(bottom = if (index < count - 2) ListItemDefaults.SegmentedGap else 0.dp)) {
        SegmentedListItem(
            selected = selected,
            onClick = { vm.selectImage(img) },
            shapes = switchRowShapes(index, count),
            colors = listRowColors(),
            leadingContent = { Icon(AppIcons.Storage, contentDescription = null) },
            supportingContent = { Text(imageSummary(LocalContext.current, img, running)) },
            trailingContent = {
                if (selected) {
                    Icon(AppIcons.CheckCircle, contentDescription = stringResource(R.string.label_selected), tint = MaterialTheme.colorScheme.primary)
                }
            },
        ) {
            Text(img.name)
        }
    }
}

@Composable
fun tabLabel(tab: TerminalTab, index: Int, customName: String? = null, showDistro: Boolean = false): String {
    val sessionTitle by tab.session.title.collectAsState()
    val title = sessionTitle?.trim().orEmpty()
    val label = when {
        !customName.isNullOrBlank() -> customName
        title.isNotEmpty() -> title
        tab.uid == 0 -> stringResource(R.string.home_tab_root_label)
        else -> stringResource(R.string.home_tab_shell_label, index + 1)
    }
    return if (showDistro && !label.contains(tab.distro)) "$label · ${tab.distro}" else label
}

@Composable
private fun AddDistroSheet(vm: BarebootViewModel, onDismiss: () -> Unit) {
    val form = rememberInstallFormState()
    val choice = form.choice(vm.catalog, vm.images.map { it.name })
    var importUri by rememberSaveable { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) importUri = uri.toString()
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        sheetGesturesEnabled = false,
    ) {
        Column(Modifier.fillMaxWidth()) {
            Column(
                Modifier
                    .weight(1f, fill = false)
                    .verticalScroll(rememberScrollState()),
            ) {
                Text(
                    stringResource(R.string.home_add_distribution),
                    style = MaterialTheme.typography.headlineSmall,
                    modifier = Modifier.padding(horizontal = 24.dp),
                )
                InstallForm(vm, form)
                SectionHeader(stringResource(R.string.home_from_file))
                SegmentedColumn {
                    SegmentedListItem(
                        onClick = { picker.launch(arrayOf("*/*")) },
                        enabled = vm.install == null,
                        shapes = ListItemDefaults.segmentedShapes(0, 1),
                        colors = listRowColors(),
                        leadingContent = { Icon(AppIcons.UploadFile, contentDescription = null) },
                        supportingContent = { Text(stringResource(R.string.home_import_file_desc)) },
                    ) { Text(stringResource(R.string.home_import_file)) }
                }
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 24.dp, end = 24.dp, top = 16.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End),
            ) {
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
                Button(
                    onClick = {
                        choice?.let {
                            vm.installDistro(it.distro, it.release, it.name, it.username, it.sizeGb)
                            onDismiss()
                        }
                    },
                    enabled = choice != null && vm.install == null,
                ) { Text(stringResource(R.string.action_install)) }
            }
        }
    }
    importUri?.let { uri ->
        val fileName = remember(uri) { displayName(context, android.net.Uri.parse(uri)) }
        ImportDialog(
            fileName = fileName,
            existing = vm.images.map { it.name },
            onDismiss = { importUri = null },
            onConfirm = { name, label, user, size ->
                importUri = null
                vm.importArchive(android.net.Uri.parse(uri), name, user, label, size)
                onDismiss()
            },
        )
    }
}

private fun displayName(context: android.content.Context, uri: android.net.Uri): String =
    runCatching {
        context.contentResolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
    }.getOrNull() ?: uri.lastPathSegment.orEmpty()

@Composable
private fun ImportDialog(
    fileName: String,
    existing: List<String>,
    onDismiss: () -> Unit,
    onConfirm: (String, String, String, Int) -> Unit,
) {
    val suggested = remember(fileName) {
        fileName.substringBefore('.').removePrefix("bareboot-").replace(Regex("[^A-Za-z0-9._-]"), "-")
            .trim('-', '.', '_').ifEmpty { "imported" }.take(40)
    }
    val defaultLabel = stringResource(R.string.home_import_default_label, fileName).take(64)
    var name by rememberSaveable { mutableStateOf(if (suggested in existing) "$suggested-2" else suggested) }
    var label by rememberSaveable { mutableStateOf(defaultLabel) }
    var user by rememberSaveable { mutableStateOf("user") }
    var size by rememberSaveable { mutableIntStateOf(16) }
    val supported = InstallService.archiveSuffix(fileName) != null
    val nameError = when {
        !NAME_RE.matches(name) -> stringResource(R.string.err_image_name_chars)
        name in existing -> stringResource(R.string.err_image_name_exists)
        else -> null
    }
    val userError = if (!Regex("^[a-z_][a-z0-9_-]{0,31}$").matches(user) || user == "root") {
        stringResource(R.string.err_username_chars)
    } else {
        null
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(AppIcons.UploadFile, contentDescription = null) },
        title = { Text(stringResource(R.string.home_import_title, fileName)) },
        text = {
            Column(
                Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (!supported) {
                    Text(stringResource(R.string.home_import_unsupported), color = MaterialTheme.colorScheme.error)
                }
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
                    onValueChange = { label = it.take(64) },
                    label = { Text(stringResource(R.string.field_distro_label)) },
                    singleLine = true,
                )
                OutlinedTextField(
                    value = user,
                    onValueChange = { user = it.trim() },
                    label = { Text(stringResource(R.string.field_user_name)) },
                    singleLine = true,
                    isError = userError != null,
                    supportingText = {
                        Text(userError ?: stringResource(R.string.home_import_user_hint))
                    },
                )
                ImageSizePicker(size, { size = it })
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onConfirm(name, label.trim().ifEmpty { name }, user, size) },
                enabled = supported && nameError == null && userError == null,
            ) { Text(stringResource(R.string.action_import)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

fun imageSummary(context: Context, img: ImageInfo, running: Boolean = false): String {
    val usage = if (img.fsSizeBytes > 0) {
        context.getString(
            R.string.home_image_usage,
            formatBytes(context, (img.fsSizeBytes - img.fsFreeBytes).coerceAtLeast(0)),
            formatBytes(context, img.fsSizeBytes),
        )
    } else {
        context.getString(R.string.home_image_usage, formatBytes(context, img.usedBytes), formatBytes(context, img.sizeBytes))
    }
    return listOfNotNull(
        img.osLabel.ifEmpty { null },
        if (running) context.getString(R.string.home_state_running) else null,
        usage,
    ).joinToString(" · ")
}

@Composable
private fun DeleteDialog(image: ImageInfo, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val summary = listOfNotNull(
        image.osLabel.ifEmpty { null },
        stringResource(R.string.home_delete_disk_usage, formatBytes(image.usedBytes)),
    ).joinToString(" · ")
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { Icon(AppIcons.Delete, contentDescription = null) },
        title = { Text(stringResource(R.string.home_delete_title, image.name)) },
        text = { Text(stringResource(R.string.home_delete_body, summary)) },
        confirmButton = {
            TextButton(
                onClick = onConfirm,
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
            ) { Text(stringResource(R.string.action_delete)) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

@Composable
private fun EnlargeDialog(image: ImageInfo, onDismiss: () -> Unit, onConfirm: (Int) -> Unit) {
    val current = ((image.sizeBytes + (1L shl 30) - 1) shr 30).toInt().coerceAtLeast(1)
    val max = maxOf(current * 4, 64).coerceAtMost(512)
    val slider = rememberSliderState(
        value = (current + 8).coerceAtMost(max).toFloat(),
        steps = (max - current - 2).coerceAtLeast(0),
        trackRange = (current + 1).toFloat()..max.toFloat(),
    )
    val target = slider.value
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.home_enlarge_title, image.name)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.home_enlarge_new_size, target.toInt()), style = MaterialTheme.typography.titleMedium)
                Slider(state = slider, onValueChange = { slider.value = it })
                Text(
                    stringResource(R.string.home_enlarge_current_size, current),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(4.dp))
            }
        },
        confirmButton = { TextButton(onClick = { onConfirm(target.toInt()) }) { Text(stringResource(R.string.home_enlarge_action)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

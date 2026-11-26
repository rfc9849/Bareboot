package ch.ausfuhrli.bareboot.ui

import android.content.ClipData
import android.content.ClipboardManager
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.Screen
import ch.ausfuhrli.bareboot.terminal.TerminalTab
import ch.ausfuhrli.bareboot.ui.theme.TerminalChromeTheme
import io.github.sagernet.libghostty.compose.GhosttyDialogs
import io.github.sagernet.libghostty.compose.GhosttyTerminal
import io.github.sagernet.libghostty.compose.rememberGhosttyTerminalState

@Composable
fun TerminalScreen(vm: BarebootViewModel) {
    val activeTab = vm.tabs.firstOrNull { it.id == vm.activeTabId } ?: vm.tabs.lastOrNull()
    val terminal = rememberGhosttyTerminalState()
    val theme = vm.currentTheme
    val ghosttyTheme = remember(theme) { theme.toGhostty() }
    val typeface = remember(vm.useBundledFont, vm.importedFont) { vm.resolveTypeface() }
    val keyFont = remember(typeface) { FontFamily(typeface) }

    TerminalChromeTheme(theme) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.surface)
                .windowInsetsPadding(WindowInsets.safeDrawing),
        ) {
            TerminalTopBar(vm, activeTab)
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.background),
            ) {
                if (activeTab != null) {
                    GhosttyTerminal(
                        session = activeTab.session,
                        state = terminal,
                        theme = ghosttyTheme,
                        darkColorScheme = true,
                        typeface = typeface,
                        fontSizeSp = vm.fontSizeSp,
                        onFontSizeChanged = vm::setFontSize,
                        configure = { view -> view.keepScreenOn = true },
                        modifier = Modifier
                            .fillMaxSize()
                            .padding(horizontal = 4.dp)
                            .clipToBounds(),
                    )
                }
            }
            if (activeTab != null) ExtraKeysBar(terminal, vm.extraKeys, vm.extraChars, keyFont)
        }
        GhosttyDialogs(terminal)
    }
}

@Composable
private fun TerminalTopBar(vm: BarebootViewModel, activeTab: TerminalTab?) {
    var menu by remember { mutableStateOf(false) }
    var renameTarget by remember { mutableStateOf<Int?>(null) }
    var searching by remember { mutableStateOf(false) }
    var imageSettings by rememberSaveable { mutableStateOf<String?>(null) }
    val distro = activeTab?.distro ?: vm.distroName
    val listState = rememberLazyListState()
    val activeIndex = vm.tabs.indexOfFirst { it.id == activeTab?.id }
    LaunchedEffect(activeIndex) {
        if (activeIndex >= 0) listState.animateScrollToItem(activeIndex)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(56.dp)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = { vm.screen = Screen.Home }) {
            Icon(AppIcons.Menu, contentDescription = stringResource(R.string.terminal_distributions))
        }
        LazyRow(
            state = listState,
            modifier = Modifier.weight(1f),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val severalDistros = vm.tabs.map { it.distro }.distinct().size > 1
            itemsIndexed(vm.tabs, key = { _, tab -> tab.id }) { index, tab ->
                TabPill(
                    label = tabLabel(tab, index, vm.tabNames[tab.id], severalDistros),
                    root = tab.uid == 0,
                    selected = tab.id == activeTab?.id,
                    onClick = { vm.selectTab(tab.id) },
                    onLongClick = { renameTarget = tab.id },
                    onClose = { vm.closeTab(tab) },
                )
            }
        }
        IconButton(onClick = { vm.openNewTab(root = false) }, enabled = !vm.busy) {
            Icon(AppIcons.Add, contentDescription = stringResource(R.string.terminal_new_shell))
        }
        Box {
            IconButton(onClick = { menu = true }) {
                Icon(AppIcons.MoreVert, contentDescription = stringResource(R.string.action_more))
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                if (vm.image(distro) != null) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.home_settings_for_image, distro)) },
                        leadingIcon = { Icon(AppIcons.Storage, contentDescription = null) },
                        onClick = {
                            menu = false
                            imageSettings = distro
                        },
                    )
                }
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.terminal_new_root_shell)) },
                    leadingIcon = { Icon(AppIcons.AdminPanelSettings, contentDescription = null) },
                    enabled = !vm.busy,
                    onClick = {
                        menu = false
                        vm.openNewTab(root = true)
                    },
                )
                if (activeTab != null) {
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.terminal_search_scrollback)) },
                        leadingIcon = { Icon(AppIcons.Visibility, contentDescription = null) },
                        onClick = {
                            menu = false
                            searching = true
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.terminal_rename_tab)) },
                        leadingIcon = { Icon(AppIcons.Edit, contentDescription = null) },
                        onClick = {
                            menu = false
                            renameTarget = activeTab.id
                        },
                    )
                    DropdownMenuItem(
                        text = { Text(stringResource(R.string.terminal_close_shell)) },
                        leadingIcon = { Icon(AppIcons.Close, contentDescription = null) },
                        onClick = {
                            menu = false
                            vm.closeTab(activeTab)
                        },
                    )
                }
                DropdownMenuItem(
                    text = { Text(stringResource(R.string.label_settings)) },
                    leadingIcon = { Icon(AppIcons.Settings, contentDescription = null) },
                    onClick = {
                        menu = false
                        vm.screen = Screen.Settings
                    },
                )
            }
        }
    }
    ImageSettingsHost(vm, imageSettings, onDismiss = { imageSettings = null })
    if (searching && activeTab != null) {
        SearchDialog(activeTab, onDismiss = { searching = false })
    }
    renameTarget?.let { id ->
        RenameTabDialog(
            current = vm.tabNames[id].orEmpty(),
            onDismiss = { renameTarget = null },
            onConfirm = {
                vm.renameTab(id, it)
                renameTarget = null
            },
        )
    }
}

@Composable
private fun SearchDialog(tab: TerminalTab, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val lines = remember(tab.id) { tab.session.transcriptText().orEmpty().lines() }
    var query by remember { mutableStateOf("") }
    val matches = if (query.isBlank()) {
        emptyList()
    } else {
        lines.withIndex().filter { it.value.contains(query, ignoreCase = true) }.takeLast(200)
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.terminal_search_scrollback)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    singleLine = true,
                    label = { Text(stringResource(R.string.terminal_search_label)) },
                    supportingText = {
                        Text(
                            if (query.isBlank()) stringResource(R.string.terminal_search_lines, lines.size)
                            else stringResource(R.string.terminal_search_matches, matches.size),
                        )
                    },
                )
                LazyColumn(Modifier.heightIn(max = 360.dp)) {
                    items(matches, key = { it.index }) { (index, line) ->
                        Text(
                            "${index + 1}: $line",
                            fontFamily = FontFamily.Monospace,
                            fontSize = 12.sp,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    val cm = context.getSystemService(ClipboardManager::class.java)
                                    cm.setPrimaryClip(ClipData.newPlainText("Bareboot", line))
                                }
                                .padding(vertical = 6.dp),
                        )
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_close)) } },
    )
}

@Composable
private fun RenameTabDialog(current: String, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.terminal_rename_tab)) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it.take(40) },
                singleLine = true,
                label = { Text(stringResource(R.string.field_name)) },
                supportingText = { Text(stringResource(R.string.terminal_rename_hint)) },
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(name) }) { Text(stringResource(R.string.action_save)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) } },
    )
}

@Composable
private fun TabPill(
    label: String,
    root: Boolean,
    selected: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    onClose: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        shape = CircleShape,
        color = if (selected) colors.secondaryContainer else Color.Transparent,
        contentColor = if (selected) colors.onSecondaryContainer else colors.onSurfaceVariant,
        modifier = Modifier
            .height(36.dp)
            .clip(CircleShape)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick, onLongClickLabel = stringResource(R.string.terminal_rename_tab)),
    ) {
        Row(
            modifier = Modifier.padding(start = 14.dp, end = if (selected) 4.dp else 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (root) {
                Icon(
                    AppIcons.AdminPanelSettings,
                    contentDescription = stringResource(R.string.label_root),
                    tint = colors.error,
                    modifier = Modifier.size(16.dp),
                )
            }
            Text(
                label,
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 168.dp),
            )
            if (selected) {
                IconButton(onClick = onClose, modifier = Modifier.size(28.dp)) {
                    Icon(AppIcons.Close, contentDescription = stringResource(R.string.terminal_close_shell), modifier = Modifier.size(16.dp))
                }
            }
        }
    }
}

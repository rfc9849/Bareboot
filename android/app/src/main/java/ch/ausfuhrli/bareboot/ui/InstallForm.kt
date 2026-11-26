package ch.ausfuhrli.bareboot.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import android.content.Context
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import ch.ausfuhrli.bareboot.InstallProgress
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.ipc.Distro
import ch.ausfuhrli.bareboot.ipc.DistroRelease

val IMAGE_SIZES_GB = listOf(8, 16, 32, 64)
private val IMAGE_NAME_RE = Regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
private val USERNAME_RE = Regex("^[a-z_][a-z0-9_-]{0,31}$")

class InstallFormState(
    distroId: MutableState<String?>,
    releaseName: MutableState<String?>,
    name: MutableState<String?>,
    username: MutableState<String?>,
    sizeGb: MutableState<Int>,
) {
    var distroId by distroId
    var releaseName by releaseName
    var nameOverride by name
    var usernameOverride by username
    var sizeGb by sizeGb
}

@Composable
fun rememberInstallFormState(): InstallFormState = InstallFormState(
    rememberSaveable { mutableStateOf(null) },
    rememberSaveable { mutableStateOf(null) },
    rememberSaveable { mutableStateOf(null) },
    rememberSaveable { mutableStateOf(null) },
    rememberSaveable { mutableIntStateOf(16) },
)

class InstallChoice(
    val distro: Distro,
    val release: DistroRelease,
    val name: String,
    val username: String,
    val sizeGb: Int,
)

fun InstallFormState.choice(catalog: List<Distro>?, existing: List<String>): InstallChoice? {
    val distro = catalog?.firstOrNull { it.id == distroId } ?: catalog?.firstOrNull() ?: return null
    val release = distro.releases.firstOrNull { it.release == releaseName } ?: distro.defaultRelease
    val name = nameOverride ?: defaultImageName(distro, existing)
    val username = usernameOverride ?: distro.defaultUser
    if (nameErrorRes(name, existing) != null || usernameErrorRes(username) != null) return null
    return InstallChoice(distro, release, name, username, sizeGb)
}

private fun defaultImageName(distro: Distro, existing: List<String>): String {
    val base = distro.defaultUser
    return if (base !in existing) base else (2..99).map { "$base-$it" }.first { it !in existing }
}

private fun nameErrorRes(name: String, existing: List<String>): Int? = when {
    !IMAGE_NAME_RE.matches(name) -> R.string.err_image_name_chars
    name in existing -> R.string.err_image_name_exists
    else -> null
}

private fun usernameErrorRes(username: String): Int? = when {
    !USERNAME_RE.matches(username) -> R.string.err_username_chars
    username == "root" -> R.string.err_username_not_root
    else -> null
}

@Composable
fun InstallForm(vm: BarebootViewModel, state: InstallFormState, modifier: Modifier = Modifier) {
    LaunchedEffect(Unit) { vm.loadCatalog() }
    val catalog = vm.catalog
    val existing = vm.images.map { it.name }
    Column(modifier) {
        when {
            catalog == null && vm.catalogError != null -> Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    stringResource(R.string.install_catalog_error, vm.catalogError.orEmpty()),
                    textAlign = TextAlign.Center,
                )
                Button(onClick = vm::loadCatalog) { Text(stringResource(R.string.action_try_again)) }
            }
            catalog == null -> Box(
                Modifier
                    .fillMaxWidth()
                    .padding(32.dp),
                contentAlignment = Alignment.Center,
            ) {
                LoadingIndicator(Modifier.size(64.dp))
            }
            else -> CatalogForm(state, catalog, existing)
        }
    }
}

@Composable
private fun CatalogForm(state: InstallFormState, catalog: List<Distro>, existing: List<String>) {
    val distro = catalog.firstOrNull { it.id == state.distroId } ?: catalog.first()
    val release = distro.releases.firstOrNull { it.release == state.releaseName } ?: distro.defaultRelease
    val colors = MaterialTheme.colorScheme

    SectionHeader(stringResource(R.string.install_section_distro))
    SegmentedColumn {
        catalog.forEachIndexed { index, option ->
            val selected = option.id == distro.id
            SegmentedListItem(
                selected = selected,
                onClick = {
                    state.distroId = option.id
                    state.releaseName = null
                },
                shapes = switchRowShapes(index, catalog.size),
                colors = listRowColors(),
                leadingContent = {
                    Surface(
                        shape = CircleShape,
                        color = if (selected) colors.primary else colors.secondaryContainer,
                        contentColor = if (selected) colors.onPrimary else colors.onSecondaryContainer,
                        modifier = Modifier.size(40.dp),
                    ) {
                        Box(contentAlignment = Alignment.Center) {
                            Text(option.name.take(1), style = MaterialTheme.typography.titleMedium)
                        }
                    }
                },
                supportingContent = {
                    val shown = if (selected) release else option.defaultRelease
                    Text(shown.label.ifEmpty { stringResource(R.string.install_rolling_release) })
                },
                trailingContent = {
                    if (selected) Icon(AppIcons.CheckCircle, contentDescription = stringResource(R.string.label_selected), tint = colors.primary)
                },
            ) {
                Text(option.name)
            }
        }
    }

    if (distro.releases.size > 1) {
        SectionHeader(stringResource(R.string.install_section_version))
        LazyRow(
            contentPadding = PaddingValues(horizontal = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(distro.releases, key = { it.release }) { option ->
                FilterChip(
                    selected = option.release == release.release,
                    onClick = { state.releaseName = option.release },
                    label = { Text(option.label.ifEmpty { option.release }) },
                )
            }
        }
    }

    SectionHeader(stringResource(R.string.install_section_image))
    Column(
        modifier = Modifier.padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val name = state.nameOverride ?: defaultImageName(distro, existing)
        val nameProblem = nameErrorRes(name, existing)?.let { stringResource(it) }
        OutlinedTextField(
            value = name,
            onValueChange = { state.nameOverride = it.trim() },
            label = { Text(stringResource(R.string.field_name)) },
            singleLine = true,
            isError = nameProblem != null,
            supportingText = nameProblem?.let { { Text(it) } },
            modifier = Modifier.fillMaxWidth(),
        )
        val username = state.usernameOverride ?: distro.defaultUser
        val userProblem = usernameErrorRes(username)?.let { stringResource(it) }
        OutlinedTextField(
            value = username,
            onValueChange = { state.usernameOverride = it.trim() },
            label = { Text(stringResource(R.string.field_username)) },
            singleLine = true,
            isError = userProblem != null,
            supportingText = {
                Text(userProblem ?: stringResource(R.string.install_username_hint))
            },
            modifier = Modifier.fillMaxWidth(),
        )
        ImageSizePicker(state.sizeGb, { state.sizeGb = it })
        Text(
            stringResource(R.string.install_download_note, formatBytes(release.size)),
            style = MaterialTheme.typography.bodySmall,
            color = colors.onSurfaceVariant,
        )
    }
}

@Composable
fun ImageSizePicker(sizeGb: Int, onSizeChange: (Int) -> Unit, modifier: Modifier = Modifier) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.install_max_size), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
        Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
            IMAGE_SIZES_GB.forEachIndexed { index, gb ->
                ToggleButton(
                    checked = sizeGb == gb,
                    onCheckedChange = { onSizeChange(gb) },
                    shapes = when (index) {
                        0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                        IMAGE_SIZES_GB.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                        else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                    },
                    modifier = Modifier
                        .weight(1f)
                        .semantics { role = Role.RadioButton },
                ) {
                    Text(stringResource(R.string.install_size_gb, gb))
                }
            }
        }
        Text(
            stringResource(R.string.install_sparse_hint),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
fun InstallProgressCard(progress: InstallProgress, modifier: Modifier = Modifier, onCancel: (() -> Unit)? = null) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        shape = MaterialTheme.shapes.extraLarge,
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(
                stringResource(
                    if (progress.unpacking) R.string.install_setting_up else R.string.install_downloading,
                    progress.osLabel,
                ),
                style = MaterialTheme.typography.titleMedium,
            )
            val context = LocalContext.current
            Text(
                when {
                    progress.unpacking -> stringResource(R.string.install_unpacking_desc)
                    progress.totalBytes > 0 -> stringResource(
                        R.string.install_bytes_of,
                        android.text.format.Formatter.formatShortFileSize(context, progress.downloadedBytes),
                        android.text.format.Formatter.formatShortFileSize(context, progress.totalBytes),
                    )
                    else -> android.text.format.Formatter.formatShortFileSize(context, progress.downloadedBytes)
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (!progress.unpacking && progress.totalBytes > 0) {
                LinearWavyProgressIndicator(
                    progress = { progress.downloadedBytes.toFloat() / progress.totalBytes },
                    modifier = Modifier.fillMaxWidth(),
                )
            } else {
                LinearWavyProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
            if (onCancel != null) {
                TextButton(onClick = onCancel, modifier = Modifier.align(Alignment.End)) { Text(stringResource(R.string.action_cancel)) }
            }
        }
    }
}

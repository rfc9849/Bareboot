package ch.ausfuhrli.bareboot.ui

import android.content.ActivityNotFoundException
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import ch.ausfuhrli.bareboot.InstallProgress
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.RootManager
import ch.ausfuhrli.bareboot.ServiceState
import kotlinx.coroutines.delay

private const val STEP_WELCOME = 0
private const val STEP_SERVICE = 1
private const val STEP_CHECK = 2
private const val STEP_FOLDER = 3
private const val STEP_DISTRO = 4

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun OnboardingScreen(vm: BarebootViewModel) {
    var step by rememberSaveable { mutableIntStateOf(STEP_WELCOME) }
    BackHandler(enabled = step > STEP_WELCOME && vm.install == null) { step-- }

    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(step) {
        when (step) {
            STEP_SERVICE -> lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (true) {
                    vm.refresh()
                    if (vm.service is ServiceState.Ready) break
                    delay(2000)
                }
            }
            STEP_CHECK -> vm.loadKernelReport()
            STEP_DISTRO -> vm.refresh()
        }
    }

    Column(
        modifier = Modifier
            .windowInsetsPadding(WindowInsets.safeDrawing)
            .readableWidth(),
    ) {
        if (step > STEP_WELCOME) {
            val progress by animateFloatAsState(step / STEP_DISTRO.toFloat(), label = "steps")
            Row(
                modifier = Modifier.padding(horizontal = 4.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = { step-- }, enabled = vm.install == null) {
                    Icon(AppIcons.ArrowBack, contentDescription = stringResource(R.string.action_back))
                }
                LinearWavyProgressIndicator(
                    progress = { progress },
                    modifier = Modifier
                        .weight(1f)
                        .padding(horizontal = 8.dp),
                )
                Spacer(Modifier.width(48.dp))
            }
        }
        AnimatedContent(
            targetState = step,
            transitionSpec = {
                val forward = targetState > initialState
                (slideInHorizontally { if (forward) it / 3 else -it / 3 } + fadeIn()) togetherWith
                    (slideOutHorizontally { if (forward) -it / 3 else it / 3 } + fadeOut())
            },
            label = "onboarding",
            modifier = Modifier.weight(1f),
        ) { current ->
            when (current) {
                STEP_WELCOME -> WelcomeStep(onNext = { step = STEP_SERVICE })
                STEP_SERVICE -> ServiceStep(vm, onNext = { step = STEP_CHECK })
                STEP_CHECK -> CheckStep(vm, onNext = { step = STEP_FOLDER })
                STEP_FOLDER -> FolderStep(vm, onNext = { step = STEP_DISTRO })
                else -> DistroStep(vm)
            }
        }
    }
}

@Composable
private fun StepLayout(
    title: String,
    body: String?,
    hero: (@Composable () -> Unit)?,
    actions: @Composable RowScope.() -> Unit,
    content: @Composable ColumnScope.() -> Unit = {},
) {
    Column(Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(vertical = 16.dp),
        ) {
            Column(Modifier.padding(horizontal = 24.dp)) {
                if (hero != null) {
                    hero()
                    Spacer(Modifier.height(24.dp))
                }
                Text(title, style = MaterialTheme.typography.headlineMedium)
                if (body != null) {
                    Spacer(Modifier.height(12.dp))
                    Text(
                        body,
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.height(24.dp))
            }
            Column(Modifier.padding(horizontal = 8.dp), content = content)
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End),
            verticalAlignment = Alignment.CenterVertically,
            content = actions,
        )
    }
}

@Composable
private fun WelcomeStep(onNext: () -> Unit) {
    StepLayout(
        title = stringResource(R.string.onboarding_welcome_title),
        body = stringResource(R.string.onboarding_welcome_body),
        hero = {
            Box(Modifier.size(96.dp), contentAlignment = Alignment.Center) {
                Image(
                    painter = painterResource(R.drawable.ic_launcher_foreground),
                    contentDescription = null,
                    modifier = Modifier.requiredSize(196.dp),
                )
            }
        },
        actions = {
            Button(onClick = onNext, contentPadding = ButtonDefaults.MediumContentPadding) {
                Text(stringResource(R.string.onboarding_get_started))
            }
        },
    ) {
        Requirement(stringResource(R.string.onboarding_req_root))
        Requirement(stringResource(R.string.onboarding_req_internet))
    }
}

@Composable
private fun Requirement(text: String) {
    Row(
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(AppIcons.CheckCircle, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
        Text(text, style = MaterialTheme.typography.bodyLarge)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ServiceStep(vm: BarebootViewModel, onNext: () -> Unit) {
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    val manager = remember { RootManager.detect(context) }
    val managerName = RootManager.name(context, manager)
    val openManager: () -> Unit = {
        manager?.launchIntent(context)?.let {
            try {
                context.startActivity(it)
            } catch (_: ActivityNotFoundException) {
            }
        }
    }
    when (val state = vm.service) {
        ServiceState.Checking -> StepLayout(
            title = stringResource(R.string.onboarding_service_checking),
            body = null,
            hero = { LoadingIndicator(modifier = Modifier.size(96.dp)) },
            actions = {},
        )
        ServiceState.NotRunning -> StepLayout(
            title = stringResource(R.string.onboarding_service_install_title),
            body = stringResource(R.string.onboarding_service_install_body),
            hero = {
                ShapeIcon(MaterialShapes.Cookie12Sided, AppIcons.Extension, colors.secondaryContainer,
                    colors.onSecondaryContainer, 96.dp)
            },
            actions = {
                if (manager != null) {
                    OutlinedButton(onClick = vm::refreshAsync) { Text(stringResource(R.string.action_check_again)) }
                    Button(onClick = openManager) { Text(stringResource(R.string.label_open_manager, manager.label)) }
                } else {
                    Button(onClick = vm::refreshAsync) { Text(stringResource(R.string.action_check_again)) }
                }
            },
        ) {
            NumberedSteps(
                listOf(
                    stringResource(R.string.onboarding_step_install_module, managerName),
                    stringResource(
                        when (manager) {
                            RootManager.KernelSu, RootManager.SukiSu -> R.string.onboarding_step_reboot_ksu
                            else -> R.string.onboarding_step_reboot
                        },
                        managerName,
                    ),
                    stringResource(R.string.onboarding_step_return),
                )
            )
            SelectionContainer {
                Text(
                    stringResource(R.string.diagnostics_startup_help, stringResource(R.string.diagnostics_startup_command)),
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 16.dp),
                )
            }
        }
        ServiceState.Unauthorized -> StepLayout(
            title = stringResource(R.string.onboarding_service_unauth_title),
            body = stringResource(R.string.onboarding_service_unauth_body, managerName),
            hero = {
                ShapeIcon(MaterialShapes.Clover4Leaf, AppIcons.Key, colors.tertiaryContainer,
                    colors.onTertiaryContainer, 96.dp)
            },
            actions = {
                if (manager != null) Button(onClick = openManager) { Text(stringResource(R.string.label_open_manager, manager.label)) }
            },
        )
        is ServiceState.Failed -> StepLayout(
            title = stringResource(R.string.onboarding_service_failed_title),
            body = state.message,
            hero = {
                ShapeIcon(MaterialShapes.Burst, AppIcons.ErrorOutline, colors.errorContainer,
                    colors.onErrorContainer, 96.dp)
            },
            actions = { Button(onClick = vm::refreshAsync) { Text(stringResource(R.string.action_check_again)) } },
        )
        is ServiceState.Ready -> StepLayout(
            title = stringResource(R.string.onboarding_service_ready_title),
            body = stringResource(R.string.onboarding_service_ready_body),
            hero = {
                ShapeIcon(MaterialShapes.Cookie9Sided, AppIcons.Done, colors.primaryContainer,
                    colors.onPrimaryContainer, 96.dp)
            },
            actions = { Button(onClick = onNext) { Text(stringResource(R.string.action_continue)) } },
        )
    }
}

@Composable
private fun NumberedSteps(steps: List<String>) {
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        steps.forEachIndexed { index, text ->
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
                Surface(
                    shape = MaterialTheme.shapes.extraLarge,
                    color = MaterialTheme.colorScheme.secondaryContainer,
                    modifier = Modifier.size(28.dp),
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Text("${index + 1}", style = MaterialTheme.typography.labelLarge)
                    }
                }
                Text(text, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.padding(top = 3.dp))
            }
        }
    }
}

@Composable
private fun CheckStep(vm: BarebootViewModel, onNext: () -> Unit) {
    val results = rememberCheckResults(vm)
    StepLayout(
        title = stringResource(R.string.settings_system_check),
        body = stringResource(R.string.onboarding_check_body),
        hero = null,
        actions = {
            if (results.blocking.isNotEmpty()) {
                OutlinedButton(onClick = vm::loadKernelReport) { Text(stringResource(R.string.action_check_again)) }
            } else {
                Button(onClick = onNext, enabled = !results.loading) { Text(stringResource(R.string.action_continue)) }
            }
        },
    ) {
        CheckSummary(results, Modifier.padding(horizontal = 16.dp))
        CheckGroups(results.items)
    }
}

@Composable
private fun FolderStep(vm: BarebootViewModel, onNext: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        if (tree != null) vm.chooseSharedFolder(tree)
    }
    StepLayout(
        title = stringResource(R.string.onboarding_folder_title),
        body = stringResource(R.string.onboarding_folder_body),
        hero = {
            ShapeIcon(MaterialShapes.Pill, AppIcons.FolderShared, colors.tertiaryContainer,
                colors.onTertiaryContainer, 96.dp)
        },
        actions = {
            if (vm.filesConnected) {
                OutlinedButton(onClick = { picker.launch(vm.folderPickerLocation()) }) { Text(stringResource(R.string.action_change)) }
                Button(onClick = onNext) { Text(stringResource(R.string.action_continue)) }
            } else {
                TextButton(onClick = onNext) { Text(stringResource(R.string.action_not_now)) }
                Button(onClick = { picker.launch(vm.folderPickerLocation()) }) { Text(stringResource(R.string.onboarding_choose_folder)) }
            }
        },
    ) {
        if (vm.filesConnected) {
            SegmentedColumn {
                SegmentedListItem(
                    shapes = ListItemDefaults.segmentedShapes(0, 1),
                    colors = listRowColors(),
                    leadingContent = { Icon(AppIcons.Folder, contentDescription = null) },
                    supportingContent = { Text(stringResource(R.string.settings_internal_storage)) },
                    trailingContent = {
                        Icon(AppIcons.CheckCircle, contentDescription = stringResource(R.string.label_connected), tint = colors.primary)
                    },
                ) {
                    Text(vm.sharedFolderName ?: vm.sharedDir)
                }
            }
        }
    }
}

@Composable
private fun DistroStep(vm: BarebootViewModel) {
    val form = rememberInstallFormState()
    val colors = MaterialTheme.colorScheme
    val progress = vm.install
    val installed = vm.images
    val choice = form.choice(vm.catalog, installed.map { it.name })
    val context = LocalContext.current
    StepLayout(
        title = stringResource(if (installed.isEmpty()) R.string.home_choose_distribution else R.string.onboarding_distro_ready_title),
        body = if (installed.isEmpty()) null else stringResource(R.string.onboarding_distro_ready_body),
        hero = if (installed.isEmpty()) {
            null
        } else {
            {
                ShapeIcon(MaterialShapes.Sunny, AppIcons.Storage, colors.primaryContainer,
                    colors.onPrimaryContainer, 96.dp)
            }
        },
        actions = {
            when {
                progress != null -> {}
                installed.isEmpty() -> Button(
                    onClick = {
                        choice?.let { vm.installDistro(it.distro, it.release, it.name, it.username, it.sizeGb) }
                    },
                    enabled = choice != null && vm.service is ServiceState.Ready,
                ) { Text(stringResource(R.string.action_install)) }
                else -> Button(onClick = vm::finishOnboarding) { Text(stringResource(R.string.home_open_terminal)) }
            }
        },
    ) {
        when {
            progress != null -> InstallProgressCard(
                progress,
                Modifier.padding(horizontal = 16.dp),
                onCancel = if (progress.unpacking) null else vm::cancelInstall,
            )
            installed.isEmpty() -> InstallForm(vm, form)
            else -> SegmentedColumn {
                installed.forEachIndexed { index, img ->
                    SegmentedListItem(
                        selected = img.name == vm.distroName,
                        onClick = { vm.selectImage(img) },
                        shapes = switchRowShapes(index, installed.size),
                        colors = listRowColors(),
                        leadingContent = { Icon(AppIcons.Storage, contentDescription = null) },
                        supportingContent = { Text(imageSummary(context, img)) },
                        trailingContent = {
                            if (img.name == vm.distroName) {
                                Icon(AppIcons.CheckCircle, contentDescription = stringResource(R.string.label_selected), tint = colors.primary)
                            }
                        },
                    ) {
                        Text(img.name)
                    }
                }
            }
        }
    }
}


package ch.ausfuhrli.bareboot.ui

import android.view.inputmethod.InputMethodManager
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.BarebootViewModel
import ch.ausfuhrli.bareboot.R
import ch.ausfuhrli.bareboot.Screen

@Composable
fun BarebootApp(vm: BarebootViewModel) {
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(Unit) {
        for (message in vm.messages) snackbar.showSnackbar(message)
    }
    BackHandler(enabled = vm.screen in setOf(Screen.Settings, Screen.SystemCheck, Screen.Diagnostics)) {
        if (vm.screen == Screen.Settings) vm.closeSettings() else vm.screen = Screen.Settings
    }
    vm.stopTarget?.let { name ->
        AlertDialog(
            onDismissRequest = vm::dismissStop,
            title = { Text(stringResource(R.string.image_stop_confirm_title, name)) },
            text = { Text(stringResource(R.string.image_stop_desc, name)) },
            confirmButton = { TextButton(onClick = vm::confirmStop) { Text(stringResource(R.string.action_stop)) } },
            dismissButton = { TextButton(onClick = vm::dismissStop) { Text(stringResource(R.string.action_cancel)) } },
        )
    }
    val view = LocalView.current
    LaunchedEffect(vm.screen) {
        if (vm.screen != Screen.Terminal) {
            view.context.getSystemService(InputMethodManager::class.java)
                ?.hideSoftInputFromWindow(view.windowToken, 0)
        }
    }
    val motion = MaterialTheme.motionScheme
    Surface(color = MaterialTheme.colorScheme.surface, modifier = Modifier.fillMaxSize()) {
        Box(Modifier.fillMaxSize()) {
            AnimatedContent(
                targetState = vm.screen,
                transitionSpec = {
                    (fadeIn(motion.defaultEffectsSpec()) +
                        scaleIn(motion.defaultSpatialSpec(), initialScale = 0.96f)) togetherWith
                        fadeOut(motion.fastEffectsSpec())
                },
                label = "screen",
            ) { screen ->
                when (screen) {
                    Screen.Onboarding -> OnboardingScreen(vm)
                    Screen.Home -> HomeScreen(vm)
                    Screen.Terminal -> TerminalScreen(vm)
                    Screen.Settings -> SettingsScreen(vm)
                    Screen.SystemCheck -> SystemCheckScreen(vm)
                    Screen.Diagnostics -> DiagnosticsScreen(vm)
                }
            }
            SnackbarHost(
                hostState = snackbar,
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .windowInsetsPadding(WindowInsets.safeDrawing),
            )
        }
    }
}

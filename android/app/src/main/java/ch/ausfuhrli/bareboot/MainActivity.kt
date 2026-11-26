package ch.ausfuhrli.bareboot

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import ch.ausfuhrli.bareboot.notify.RunningNotifier
import ch.ausfuhrli.bareboot.ui.BarebootApp
import ch.ausfuhrli.bareboot.ui.theme.BarebootTheme

class MainActivity : ComponentActivity() {
    private val vm: BarebootViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            BarebootTheme {
                BarebootApp(vm)
            }
        }
        if (savedInstanceState == null) handleIntent(intent)
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN && event.isCtrlPressed && vm.screen == Screen.Terminal) {
            when {
                event.keyCode == KeyEvent.KEYCODE_TAB -> {
                    vm.nextTab(if (event.isShiftPressed) -1 else 1)
                    return true
                }
                event.isShiftPressed && event.keyCode == KeyEvent.KEYCODE_T -> {
                    vm.openNewTab(root = false)
                    return true
                }
                event.isShiftPressed && event.keyCode == KeyEvent.KEYCODE_W -> {
                    vm.closeActiveTab()
                    return true
                }
            }
        }
        return super.dispatchKeyEvent(event)
    }

    override fun onStop() {
        vm.saveScrollback()
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    private fun handleIntent(intent: Intent?) {
        val distro = intent?.getStringExtra(RunningNotifier.EXTRA_OPEN_DISTRO) ?: return
        intent.removeExtra(RunningNotifier.EXTRA_OPEN_DISTRO)
        vm.openFromNotification(distro)
    }
}

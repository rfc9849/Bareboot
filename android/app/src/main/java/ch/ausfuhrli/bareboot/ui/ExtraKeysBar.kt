package ch.ausfuhrli.bareboot.ui

import android.view.KeyEvent
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import android.content.Context
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.sagernet.libghostty.GhosttyExtraKeysState.StickyState
import io.github.sagernet.libghostty.compose.GhosttyTerminalState
import kotlinx.coroutines.delay

val EXTRA_KEY_IDS = listOf(
    "esc", "tab", "ctrl", "alt", "left", "down", "up", "right", "home", "end", "pgup", "pgdn", "paste",
)

fun extraKeyLabel(context: Context, id: String): String = when (id) {
    "esc" -> "Esc"
    "tab" -> "Tab"
    "ctrl" -> "Ctrl"
    "alt" -> "Alt"
    "left" -> context.getString(R.string.extra_key_left)
    "down" -> context.getString(R.string.extra_key_down)
    "up" -> context.getString(R.string.extra_key_up)
    "right" -> context.getString(R.string.extra_key_right)
    "home" -> "Home"
    "end" -> "End"
    "pgup" -> "PgUp"
    "pgdn" -> "PgDn"
    "paste" -> context.getString(R.string.extra_key_paste)
    else -> id
}

@Composable
fun ExtraKeysBar(state: GhosttyTerminalState, keys: List<String>, chars: String, fontFamily: FontFamily) {
    val view = state.view
    val ctrl by state.extraKeys.ctrl.collectAsState()
    val alt by state.extraKeys.alt.collectAsState()
    val key: (Int) -> Unit = { code -> view?.sendKey(code) }
    val text: (String) -> Unit = { value -> view?.sendText(value) }
    val characters = remember(chars) {
        chars.codePoints().toArray().map { String(Character.toChars(it)) }.distinct().filter { it.isNotBlank() }
    }
    if (keys.isEmpty() && characters.isEmpty()) return
    var expanded by rememberSaveable { mutableStateOf(false) }
    val keyContent: @Composable (String) -> Unit = { id ->
        when (id) {
            "esc" -> Key("Esc", fontFamily = fontFamily) { key(KeyEvent.KEYCODE_ESCAPE) }
            "tab" -> Key(icon = AppIcons.KeyboardTab, description = "Tab") { key(KeyEvent.KEYCODE_TAB) }
            "ctrl" -> Key("Ctrl", fontFamily = fontFamily, sticky = ctrl) { state.extraKeys.toggleCtrl() }
            "alt" -> Key("Alt", fontFamily = fontFamily, sticky = alt) { state.extraKeys.toggleAlt() }
            "left" -> Key(icon = AppIcons.KeyboardArrowLeft, description = stringResource(R.string.extra_key_left), repeat = true) {
                key(KeyEvent.KEYCODE_DPAD_LEFT)
            }
            "down" -> Key(icon = AppIcons.KeyboardArrowDown, description = stringResource(R.string.extra_key_down), repeat = true) {
                key(KeyEvent.KEYCODE_DPAD_DOWN)
            }
            "up" -> Key(icon = AppIcons.KeyboardArrowUp, description = stringResource(R.string.extra_key_up), repeat = true) {
                key(KeyEvent.KEYCODE_DPAD_UP)
            }
            "right" -> Key(icon = AppIcons.KeyboardArrowRight, description = stringResource(R.string.extra_key_right), repeat = true) {
                key(KeyEvent.KEYCODE_DPAD_RIGHT)
            }
            "home" -> Key("Home", fontFamily = fontFamily) { key(KeyEvent.KEYCODE_MOVE_HOME) }
            "end" -> Key("End", fontFamily = fontFamily) { key(KeyEvent.KEYCODE_MOVE_END) }
            "pgup" -> Key("PgUp", fontFamily = fontFamily, repeat = true) { key(KeyEvent.KEYCODE_PAGE_UP) }
            "pgdn" -> Key("PgDn", fontFamily = fontFamily, repeat = true) { key(KeyEvent.KEYCODE_PAGE_DOWN) }
            "paste" -> Key(icon = AppIcons.ContentPaste, description = stringResource(R.string.extra_key_paste)) { view?.pasteFromClipboard() }
            else -> Key(id, fontFamily = fontFamily) { text(id) }
        }
    }
    val keyWidth = maxOf(48.dp, with(LocalDensity.current) { 13.sp.toDp() } * 3 + 8.dp)
    val allKeys = keys + characters
    Surface(color = MaterialTheme.colorScheme.surface) {
        BoxWithConstraints(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp)) {
            val slots = (maxWidth / keyWidth).toInt().coerceAtLeast(1)
            val hasMore = allKeys.size > slots
            val visible = if (hasMore) slots - 1 else allKeys.size
            Column {
                if (hasMore && expanded) {
                    FlowRow {
                        allKeys.drop(visible).forEach { id ->
                            Box(Modifier.width(keyWidth).padding(horizontal = 2.dp, vertical = 4.dp)) {
                                keyContent(id)
                            }
                        }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    allKeys.take(visible).forEach { id ->
                        Box(Modifier.width(keyWidth).padding(horizontal = 2.dp)) { keyContent(id) }
                    }
                    if (hasMore) {
                        IconButton(onClick = { expanded = !expanded }) {
                            Icon(
                                if (expanded) AppIcons.ExpandMore else AppIcons.ExpandLess,
                                contentDescription = stringResource(
                                    if (expanded) R.string.extra_keys_hide else R.string.extra_keys_show,
                                ),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Key(
    label: String? = null,
    fontFamily: FontFamily = FontFamily.Default,
    icon: ImageVector? = null,
    description: String? = null,
    sticky: StickyState = StickyState.INACTIVE,
    repeat: Boolean = false,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val currentOnClick by rememberUpdatedState(onClick)
    if (repeat) {
        LaunchedEffect(pressed) {
            if (!pressed) return@LaunchedEffect
            delay(400)
            while (true) {
                currentOnClick()
                delay(60)
            }
        }
    }
    val (container, content) = when (sticky) {
        StickyState.INACTIVE -> colors.surfaceContainerHigh to colors.onSurface
        StickyState.ARMED -> colors.primaryContainer to colors.onPrimaryContainer
        StickyState.LOCKED -> colors.primary to colors.onPrimary
    }
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(if (sticky == StickyState.INACTIVE) 10.dp else 18.dp),
        color = container,
        contentColor = content,
        interactionSource = interaction,
        modifier = Modifier.fillMaxWidth().height(48.dp),
    ) {
        Box(contentAlignment = Alignment.Center) {
            if (icon != null) {
                Icon(icon, contentDescription = description, modifier = Modifier.size(20.dp))
            } else {
                Text(
                    text = label.orEmpty(),
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Medium,
                    fontFamily = fontFamily,
                )
            }
        }
    }
}

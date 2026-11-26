package ch.ausfuhrli.bareboot.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.platform.LocalContext
import ch.ausfuhrli.bareboot.ui.TerminalTheme

private val LightColors = lightColorScheme(
    primary = Color(0xFF006D43),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFF8DF7BE),
    onPrimaryContainer = Color(0xFF002111),
    secondary = Color(0xFF4E6355),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFD0E8D6),
    onSecondaryContainer = Color(0xFF0B1F14),
    tertiary = Color(0xFF3B6470),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFBFE9F8),
    onTertiaryContainer = Color(0xFF001F27),
    background = Color(0xFFF6FBF4),
    onBackground = Color(0xFF171D19),
    surface = Color(0xFFF6FBF4),
    onSurface = Color(0xFF171D19),
    surfaceVariant = Color(0xFFDCE5DC),
    onSurfaceVariant = Color(0xFF404943),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFF0F5EE),
    surfaceContainer = Color(0xFFEAEFE8),
    surfaceContainerHigh = Color(0xFFE4EAE3),
    surfaceContainerHighest = Color(0xFFDFE4DD),
    outline = Color(0xFF707972),
    outlineVariant = Color(0xFFC0C9C0),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF4BE39A),
    onPrimary = Color(0xFF003920),
    primaryContainer = Color(0xFF005231),
    onPrimaryContainer = Color(0xFF8DF7BE),
    secondary = Color(0xFFB4CCBA),
    onSecondary = Color(0xFF203528),
    secondaryContainer = Color(0xFF364B3E),
    onSecondaryContainer = Color(0xFFD0E8D6),
    tertiary = Color(0xFFA3CDDB),
    onTertiary = Color(0xFF033541),
    tertiaryContainer = Color(0xFF214C58),
    onTertiaryContainer = Color(0xFFBFE9F8),
    background = Color(0xFF0F1511),
    onBackground = Color(0xFFDFE4DD),
    surface = Color(0xFF0F1511),
    onSurface = Color(0xFFDFE4DD),
    surfaceVariant = Color(0xFF404943),
    onSurfaceVariant = Color(0xFFC0C9C0),
    surfaceContainerLowest = Color(0xFF0A0F0C),
    surfaceContainerLow = Color(0xFF171D19),
    surfaceContainer = Color(0xFF1B211D),
    surfaceContainerHigh = Color(0xFF252B27),
    surfaceContainerHighest = Color(0xFF303632),
    outline = Color(0xFF8A938B),
    outlineVariant = Color(0xFF404943),
)

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun BarebootTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> DarkColors
        else -> LightColors
    }
    MaterialExpressiveTheme(
        colorScheme = colors,
        motionScheme = MotionScheme.expressive(),
        content = content,
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TerminalChromeTheme(terminal: TerminalTheme, content: @Composable () -> Unit) {
    MaterialExpressiveTheme(
        colorScheme = terminalColorScheme(terminal),
        motionScheme = MotionScheme.expressive(),
        content = content,
    )
}

private fun terminalColorScheme(t: TerminalTheme): ColorScheme {
    val bg = Color(t.background)
    val fg = Color(t.foreground)
    val accent = Color(t.ansi16[4])
    val chrome = t.chromeColor
    return darkColorScheme(
        primary = accent,
        onPrimary = bg,
        primaryContainer = accent.copy(alpha = 0.28f).compositeOver(chrome),
        onPrimaryContainer = fg,
        secondaryContainer = fg.copy(alpha = 0.12f).compositeOver(chrome),
        onSecondaryContainer = fg,
        tertiary = Color(t.ansi16[2]),
        error = Color(t.ansi16[1]),
        background = bg,
        onBackground = fg,
        surface = chrome,
        onSurface = fg,
        onSurfaceVariant = fg.copy(alpha = 0.72f).compositeOver(chrome),
        surfaceContainerLow = chrome,
        surfaceContainer = fg.copy(alpha = 0.06f).compositeOver(chrome),
        surfaceContainerHigh = fg.copy(alpha = 0.10f).compositeOver(chrome),
        surfaceContainerHighest = fg.copy(alpha = 0.14f).compositeOver(chrome),
        outline = fg.copy(alpha = 0.4f).compositeOver(chrome),
        outlineVariant = fg.copy(alpha = 0.16f).compositeOver(chrome),
    )
}

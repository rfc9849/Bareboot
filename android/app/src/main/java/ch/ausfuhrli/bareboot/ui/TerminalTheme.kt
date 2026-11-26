package ch.ausfuhrli.bareboot.ui

import androidx.compose.ui.graphics.Color
import android.content.Context
import io.github.sagernet.libghostty.GhosttyTheme
import io.github.sagernet.libghostty.extras.GhosttyThemeStore

data class TerminalTheme(
    val id: String,
    val label: String,
    val background: Int,
    val foreground: Int,
    val cursor: Int,
    val chromeBg: Long,
    val ansi16: IntArray,
) {
    val bgColor: Color get() = Color(background)
    val chromeColor: Color get() = Color(chromeBg)

    fun toGhostty(): GhosttyTheme = GhosttyTheme(
        foreground = foreground,
        background = background,
        cursorColor = cursor,
        selectionBackground = null,
        selectionForeground = null,
        palette = List(256) { ansi16.getOrNull(it) },
    )
}

object TerminalThemes {
    val all: List<TerminalTheme> = listOf(
        TerminalTheme(
            id = "onedark",
            label = "One Dark",
            background = 0xFF1E2127.toInt(),
            foreground = 0xFFABB2BF.toInt(),
            cursor = 0xFF528BFF.toInt(),
            chromeBg = 0xFF16191E,
            ansi16 = intArrayOf(
                0xFF282C34.toInt(), 0xFFE06C75.toInt(), 0xFF98C379.toInt(), 0xFFE5C07B.toInt(),
                0xFF61AFEF.toInt(), 0xFFC678DD.toInt(), 0xFF56B6C2.toInt(), 0xFFABB2BF.toInt(),
                0xFF5C6370.toInt(), 0xFFE06C75.toInt(), 0xFF98C379.toInt(), 0xFFE5C07B.toInt(),
                0xFF61AFEF.toInt(), 0xFFC678DD.toInt(), 0xFF56B6C2.toInt(), 0xFFFFFFFF.toInt(),
            ),
        ),
        TerminalTheme(
            id = "tokyonight",
            label = "Tokyo Night",
            background = 0xFF1A1B26.toInt(),
            foreground = 0xFFC0CAF5.toInt(),
            cursor = 0xFFC0CAF5.toInt(),
            chromeBg = 0xFF13141D,
            ansi16 = intArrayOf(
                0xFF15161E.toInt(), 0xFFF7768E.toInt(), 0xFF9ECE6A.toInt(), 0xFFE0AF68.toInt(),
                0xFF7AA2F7.toInt(), 0xFFBB9AF7.toInt(), 0xFF7DCFFF.toInt(), 0xFFA9B1D6.toInt(),
                0xFF414868.toInt(), 0xFFF7768E.toInt(), 0xFF9ECE6A.toInt(), 0xFFE0AF68.toInt(),
                0xFF7AA2F7.toInt(), 0xFFBB9AF7.toInt(), 0xFF7DCFFF.toInt(), 0xFFC0CAF5.toInt(),
            ),
        ),
        TerminalTheme(
            id = "gruvbox",
            label = "Gruvbox",
            background = 0xFF1D2021.toInt(),
            foreground = 0xFFEBDBB2.toInt(),
            cursor = 0xFFFE8019.toInt(),
            chromeBg = 0xFF161819,
            ansi16 = intArrayOf(
                0xFF282828.toInt(), 0xFFCC241D.toInt(), 0xFF98971A.toInt(), 0xFFD79921.toInt(),
                0xFF458588.toInt(), 0xFFB16286.toInt(), 0xFF689D6A.toInt(), 0xFFA89984.toInt(),
                0xFF928374.toInt(), 0xFFFB4934.toInt(), 0xFFB8BB26.toInt(), 0xFFFABD2F.toInt(),
                0xFF83A598.toInt(), 0xFFD3869B.toInt(), 0xFF8EC07C.toInt(), 0xFFEBDBB2.toInt(),
            ),
        ),
        TerminalTheme(
            id = "amoled",
            label = "AMOLED",
            background = 0xFF000000.toInt(),
            foreground = 0xFFE5E7EB.toInt(),
            cursor = 0xFF3B82F6.toInt(),
            chromeBg = 0xFF0B0D10,
            ansi16 = intArrayOf(
                0xFF111318.toInt(), 0xFFF87171.toInt(), 0xFF4ADE80.toInt(), 0xFFFACC15.toInt(),
                0xFF60A5FA.toInt(), 0xFFC084FC.toInt(), 0xFF22D3EE.toInt(), 0xFFD1D5DB.toInt(),
                0xFF6B7280.toInt(), 0xFFFCA5A5.toInt(), 0xFF86EFAC.toInt(), 0xFFFDE047.toInt(),
                0xFF93C5FD.toInt(), 0xFFD8B4FE.toInt(), 0xFF67E8F9.toInt(), 0xFFFFFFFF.toInt(),
            ),
        ),
    )

    const val GHOSTTY_PREFIX = "ghostty:"

    fun byId(context: Context, id: String): TerminalTheme {
        all.firstOrNull { it.id == id }?.let { return it }
        val name = id.removePrefix(GHOSTTY_PREFIX)
        if (id.startsWith(GHOSTTY_PREFIX)) {
            runCatching { GhosttyThemeStore.loadTheme(context, name) }.getOrNull()?.let { return fromGhostty(id, name, it) }
        }
        return all.first()
    }

    fun ghosttyThemeNames(context: Context): List<String> =
        runCatching { (GhosttyThemeStore.listThemes(context, true) + GhosttyThemeStore.listThemes(context, false)).distinct().sorted() }
            .getOrDefault(emptyList())

    private fun fromGhostty(id: String, name: String, theme: GhosttyTheme): TerminalTheme {
        val background = theme.background ?: 0xFF000000.toInt()
        val foreground = theme.foreground ?: 0xFFFFFFFF.toInt()
        val palette = IntArray(16) { theme.palette.getOrNull(it) ?: foreground }
        return TerminalTheme(
            id = id,
            label = name,
            background = background,
            foreground = foreground,
            cursor = theme.cursorColor ?: foreground,
            chromeBg = darken(background),
            ansi16 = palette,
        )
    }

    private fun darken(argb: Int): Long {
        fun channel(shift: Int) = ((argb shr shift) and 0xFF) * 4 / 5
        return 0xFF000000L or (channel(16).toLong() shl 16) or (channel(8).toLong() shl 8) or channel(0).toLong()
    }
}

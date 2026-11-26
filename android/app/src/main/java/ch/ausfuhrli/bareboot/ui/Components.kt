package ch.ausfuhrli.bareboot.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.ListItemColors
import androidx.compose.material3.ListItemShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import android.content.Context
import ch.ausfuhrli.bareboot.R
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.RoundedPolygon

@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier) {
    Text(
        text = text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 8.dp),
    )
}

@Composable
fun SegmentedColumn(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier = modifier.padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap),
        content = content,
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ShapeIcon(
    shape: RoundedPolygon,
    icon: ImageVector,
    container: Color,
    content: Color,
    size: Dp = 48.dp,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .size(size)
            .clip(shape.toShape())
            .background(container),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = content, modifier = Modifier.size(size * 0.5f))
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun switchRowShapes(index: Int, count: Int): ListItemShapes =
    ListItemDefaults.segmentedShapes(index, count).let { it.copy(selectedShape = it.shape) }

fun Modifier.readableWidth(): Modifier =
    fillMaxSize().wrapContentWidth(Alignment.CenterHorizontally).widthIn(max = 720.dp).fillMaxWidth()

fun formatBytes(context: Context, bytes: Long): String {
    val mb = bytes / (1024.0 * 1024)
    if (mb < 1000) return context.getString(R.string.unit_mb, mb.toInt().toString())
    val gb = mb / 1024
    return context.getString(R.string.unit_gb, if (gb >= 10) gb.toInt().toString() else "%.1f".format(gb))
}

@Composable
fun formatBytes(bytes: Long): String = formatBytes(LocalContext.current, bytes)

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun listRowColors(): ListItemColors = ListItemDefaults.segmentedColors(
    containerColor = MaterialTheme.colorScheme.surfaceContainer,
    disabledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
)

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun switchRowColors(): ListItemColors {
    val scheme = MaterialTheme.colorScheme
    return ListItemDefaults.segmentedColors(
        containerColor = scheme.surfaceContainer,
        disabledContainerColor = scheme.surfaceContainer,
        selectedContainerColor = scheme.surfaceContainer,
        selectedContentColor = scheme.onSurface,
        selectedLeadingContentColor = scheme.onSurfaceVariant,
        selectedTrailingContentColor = scheme.onSurfaceVariant,
        selectedOverlineContentColor = scheme.onSurfaceVariant,
        selectedSupportingContentColor = scheme.onSurfaceVariant,
    )
}

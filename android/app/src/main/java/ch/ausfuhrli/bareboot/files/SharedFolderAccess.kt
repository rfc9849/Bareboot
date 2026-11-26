package ch.ausfuhrli.bareboot.files

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import ch.ausfuhrli.bareboot.SettingsStore

internal class SharedFolderAccess(private val context: Context) {
    private val store = SettingsStore(context)
    private val resolver get() = context.contentResolver

    fun pickerLocation(sharedDir: String): Uri? =
        documentId(sharedDir)?.let { DocumentsContract.buildDocumentUri(STORAGE_AUTHORITY, it) }

    fun activeTree(): Uri? {
        val settings = store.load()
        val tree = store.filesTree ?: return null
        return tree.takeIf {
            settings.shareEnabled && matches(it, settings.sharedDir) &&
                resolver.persistedUriPermissions.any { grant ->
                    grant.uri == it && grant.isReadPermission && grant.isWritePermission
                }
        }
    }

    fun pathForTree(tree: Uri): String? {
        if (tree.authority != STORAGE_AUTHORITY || !DocumentsContract.isTreeUri(tree)) return null
        val id = DocumentsContract.getTreeDocumentId(tree)
        val relative = id.removePrefix(PRIMARY_PREFIX)
        if (!id.startsWith(PRIMARY_PREFIX) || !validRelativePath(relative)) return null
        val user = context.getSystemService(StorageManager::class.java)
            ?.primaryStorageVolume?.directory?.name?.toIntOrNull() ?: return null
        return "/data/media/$user/$relative"
    }

    fun displayName(sharedDir: String): String? = documentId(sharedDir)?.removePrefix(PRIMARY_PREFIX)

    fun rootUri(): Uri? = activeTree()?.let {
        DocumentsContract.buildRootUri(authority(context), DocumentsContract.getTreeDocumentId(it))
    }

    fun connect(tree: Uri): Boolean {
        val settings = store.load()
        if (!settings.shareEnabled || !matches(tree, settings.sharedDir)) return false
        try {
            resolver.takePersistableUriPermission(tree, GRANT_FLAGS)
        } catch (_: SecurityException) {
            return false
        }
        val previous = store.filesTree
        store.filesTree = tree
        if (previous != tree) release(previous)
        notifyRoots(context)
        return true
    }

    fun disconnect() {
        val previous = store.filesTree
        store.filesTree = null
        release(previous)
        notifyRoots(context)
    }

    private fun release(tree: Uri?) {
        if (tree != null) {
            try {
                resolver.releasePersistableUriPermission(tree, GRANT_FLAGS)
            } catch (_: SecurityException) {
            }
        }
    }

    private fun matches(tree: Uri, path: String): Boolean =
        tree.scheme == "content" && tree.authority == STORAGE_AUTHORITY &&
            tree.pathSegments.size == 2 && DocumentsContract.isTreeUri(tree) &&
            DocumentsContract.getTreeDocumentId(tree) == documentId(path)

    private fun documentId(sharedDir: String): String? {
        val directory = context.getSystemService(StorageManager::class.java)
            ?.primaryStorageVolume?.directory ?: return null
        val path = sharedDir.trim().trimEnd('/')
        val devicePath = directory.name.toIntOrNull()?.let { "/data/media/$it/" }
        val visiblePath = "${directory.absolutePath}/"
        val relative = when {
            path.startsWith(visiblePath) -> path.removePrefix(visiblePath)
            devicePath != null && path.startsWith(devicePath) -> path.removePrefix(devicePath)
            else -> return null
        }
        return relative.takeIf(::validRelativePath)?.let { "$PRIMARY_PREFIX$it" }
    }

    companion object {
        private const val STORAGE_AUTHORITY = "com.android.externalstorage.documents"
        private const val PRIMARY_PREFIX = "primary:"
        private const val GRANT_FLAGS =
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION

        fun authority(context: Context): String = "${context.packageName}.files"

        fun notifyRoots(context: Context) {
            context.contentResolver.notifyChange(DocumentsContract.buildRootsUri(authority(context)), null)
        }

        fun validRelativePath(path: String): Boolean =
            '\u0000' !in path && path.split('/').all { it.isNotEmpty() && it != "." && it != ".." }
    }
}

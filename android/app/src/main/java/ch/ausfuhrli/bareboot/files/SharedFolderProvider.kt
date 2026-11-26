package ch.ausfuhrli.bareboot.files

import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Binder
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import java.io.FileNotFoundException

class SharedFolderProvider : DocumentsProvider() {
    private val appContext get() = checkNotNull(context)
    private val resolver get() = appContext.contentResolver
    private val access by lazy { SharedFolderAccess(appContext) }
    private val authority get() = SharedFolderAccess.authority(appContext)

    override fun onCreate(): Boolean = true

    override fun queryRoots(projection: Array<out String>?): Cursor = asApp {
        val result = MatrixCursor(projection ?: ROOT_COLUMNS)
        val tree = access.activeTree() ?: return@asApp result
        val id = DocumentsContract.getTreeDocumentId(tree)
        val flags = try {
            resolver.query(uri(tree, id), arrayOf(Document.COLUMN_FLAGS), null, null, null)?.use {
                if (it.moveToFirst()) it.getInt(0) else null
            }
        } catch (_: SecurityException) {
            null
        } ?: return@asApp result
        val rootFlags = Root.FLAG_LOCAL_ONLY or Root.FLAG_SUPPORTS_IS_CHILD or
            if (flags and Document.FLAG_DIR_SUPPORTS_CREATE != 0) Root.FLAG_SUPPORTS_CREATE else 0
        result.addRow(result.columnNames.map {
            when (it) {
                Root.COLUMN_ROOT_ID, Root.COLUMN_DOCUMENT_ID -> id
                Root.COLUMN_TITLE -> "Bareboot"
                Root.COLUMN_SUMMARY -> id.substringAfterLast('/').substringAfter(':')
                Root.COLUMN_FLAGS -> rootFlags
                Root.COLUMN_MIME_TYPES -> "*/*"
                else -> null
            }
        }.toTypedArray<Any?>())
        result
    }

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor =
        withTree { tree ->
            documents(tree, uri(tree, documentId), projection, null).also {
                if (it.count == 0) throw FileNotFoundException(documentId)
            }
        }

    override fun queryChildDocuments(
        parentDocumentId: String,
        projection: Array<out String>?,
        sortOrder: String?,
    ): Cursor = withTree { tree ->
        checkId(tree, parentDocumentId)
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentDocumentId)
        documents(tree, children, projection, sortOrder).apply {
            setNotificationUri(resolver, DocumentsContract.buildChildDocumentsUri(authority, parentDocumentId))
        }
    }

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?,
    ): ParcelFileDescriptor = withTree { tree ->
        resolver.openFileDescriptor(uri(tree, documentId), mode, signal)
            ?: throw FileNotFoundException(documentId)
    }

    override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String =
        withTree { tree ->
            val created = DocumentsContract.createDocument(resolver, uri(tree, parentDocumentId), mimeType, displayName)
                ?: throw FileNotFoundException(parentDocumentId)
            notifyChildren(parentDocumentId)
            DocumentsContract.getDocumentId(created)
        }

    override fun renameDocument(documentId: String, displayName: String): String = withTree { tree ->
        val renamed = DocumentsContract.renameDocument(resolver, uri(tree, documentId, allowRoot = false), displayName)
            ?: throw FileNotFoundException(documentId)
        notifyChildren(documentId.substringBeforeLast('/'))
        DocumentsContract.getDocumentId(renamed)
    }

    override fun deleteDocument(documentId: String) {
        withTree { tree ->
            if (!DocumentsContract.deleteDocument(resolver, uri(tree, documentId, allowRoot = false))) {
                throw FileNotFoundException(documentId)
            }
            notifyChildren(documentId.substringBeforeLast('/'))
        }
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean = try {
        withTree { tree ->
            DocumentsContract.isChildDocument(resolver, uri(tree, parentDocumentId), uri(tree, documentId))
        }
    } catch (_: FileNotFoundException) {
        false
    } catch (_: SecurityException) {
        false
    } catch (_: IllegalArgumentException) {
        false
    }

    private fun documents(
        tree: Uri,
        source: Uri,
        projection: Array<out String>?,
        sortOrder: String?,
    ): MatrixCursor {
        val result = MatrixCursor(projection ?: DOCUMENT_COLUMNS)
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        resolver.query(source, DOCUMENT_COLUMNS, null, null, sortOrder)?.use { entries ->
            while (entries.moveToNext()) {
                val id = entries.getString(entries.getColumnIndexOrThrow(Document.COLUMN_DOCUMENT_ID))
                if (!contains(rootId, id)) continue
                result.addRow(result.columnNames.map { column ->
                    val index = entries.getColumnIndex(column)
                    if (index < 0 || entries.isNull(index)) null
                    else when (column) {
                        Document.COLUMN_FLAGS -> entries.getInt(index) and
                            if (id == rootId) ROOT_DOCUMENT_FLAGS else DOCUMENT_FLAGS
                        Document.COLUMN_LAST_MODIFIED, Document.COLUMN_SIZE -> entries.getLong(index)
                        else -> entries.getString(index)
                    }
                }.toTypedArray<Any?>())
            }
        }
        return result
    }

    private fun notifyChildren(id: String) {
        resolver.notifyChange(DocumentsContract.buildChildDocumentsUri(authority, id), null)
    }

    private fun checkId(tree: Uri, id: String, allowRoot: Boolean = true) {
        val rootId = DocumentsContract.getTreeDocumentId(tree)
        if ((!allowRoot && id == rootId) || !contains(rootId, id)) throw FileNotFoundException(id)
    }

    private fun uri(tree: Uri, id: String, allowRoot: Boolean = true): Uri {
        checkId(tree, id, allowRoot)
        return DocumentsContract.buildDocumentUriUsingTree(tree, id)
    }

    private fun contains(rootId: String, id: String): Boolean =
        id == rootId || (id.startsWith("$rootId/") &&
            SharedFolderAccess.validRelativePath(id.substring(rootId.length + 1)))

    private inline fun <T> withTree(block: (Uri) -> T): T = asApp {
        block(access.activeTree() ?: throw FileNotFoundException("Shared folder is not connected"))
    }

    private inline fun <T> asApp(block: () -> T): T {
        val identity = Binder.clearCallingIdentity()
        try {
            return block()
        } finally {
            Binder.restoreCallingIdentity(identity)
        }
    }

    private companion object {
        val ROOT_COLUMNS = arrayOf(
            Root.COLUMN_ROOT_ID, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_TITLE, Root.COLUMN_SUMMARY,
            Root.COLUMN_FLAGS, Root.COLUMN_MIME_TYPES,
        )
        val DOCUMENT_COLUMNS = arrayOf(
            Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
            Document.COLUMN_FLAGS, Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED,
        )
        const val ROOT_DOCUMENT_FLAGS = Document.FLAG_DIR_SUPPORTS_CREATE
        const val DOCUMENT_FLAGS = ROOT_DOCUMENT_FLAGS or Document.FLAG_SUPPORTS_WRITE or
            Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME
    }
}

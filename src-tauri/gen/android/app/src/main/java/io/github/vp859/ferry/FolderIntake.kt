package io.github.vp859.ferry

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import java.io.File
import java.lang.ref.WeakReference

/**
 * Sending a folder from Android: the user picks it in the system folder
 * picker (Storage Access Framework, `ACTION_OPEN_DOCUMENT_TREE`), and its
 * files are copied with their structure into `cacheDir/share-in/<folder>`,
 * because the transfer engine in Rust reads regular files, not `content://`
 * documents. The path of the copy is kept in `pending-folder.txt` for the
 * frontend (`take_picked_folder`); copies older than a day are removed at
 * startup like shared files.
 *
 * Only the platform's DocumentsContract is used, no extra library.
 */
object FolderIntake {
  private const val TAG = "FerryFolder"
  private const val PENDING = "pending-folder.txt"
  /** Deeper folders are not copied (a loop of links must not run forever). */
  private const val MAX_DEPTH = 64

  /** The activity that shows the picker (set by [MainActivity]). */
  @Volatile
  var activity: WeakReference<MainActivity>? = null

  /** Shows the system folder picker; the result arrives in [MainActivity.onFolderPicked]. */
  @JvmStatic
  fun pick(@Suppress("UNUSED_PARAMETER") context: Context) {
    val current = activity?.get()
    if (current == null) {
      Log.w(TAG, "No activity to show the folder picker")
      return
    }
    current.runOnUiThread { current.pickFolder() }
  }

  /**
   * Copies the folder [tree] into the share cache and records the copy as the
   * picked folder; with `null` (cancelled) forgets an earlier one.
   *
   * Performs blocking I/O and must not be called on the UI thread.
   *
   * @return whether something was copied.
   */
  fun copy(context: Context, tree: Uri?): Boolean {
    val base = File(context.cacheDir, "share-in").apply { mkdirs() }
    val pending = File(base, PENDING)
    pending.delete()
    if (tree == null) return false
    return try {
      val rootId = DocumentsContract.getTreeDocumentId(tree)
      val name = displayName(context, DocumentsContract.buildDocumentUriUsingTree(tree, rootId))
      val target = ShareIntake.uniqueFile(base, ShareIntake.sanitize(name ?: "Folder"))
      target.mkdirs()
      copyChildren(context, tree, rootId, target, 0)
      pending.writeText(target.absolutePath)
      true
    } catch (e: Exception) {
      Log.w(TAG, "Could not copy the folder $tree", e)
      false
    }
  }

  private fun copyChildren(context: Context, tree: Uri, parentId: String, dir: File, depth: Int) {
    if (depth > MAX_DEPTH) return
    val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
    val columns = arrayOf(
      DocumentsContract.Document.COLUMN_DOCUMENT_ID,
      DocumentsContract.Document.COLUMN_DISPLAY_NAME,
      DocumentsContract.Document.COLUMN_MIME_TYPE,
    )
    context.contentResolver.query(children, columns, null, null, null)?.use { c ->
      while (c.moveToNext()) {
        val id = c.getString(0) ?: continue
        val name = ShareIntake.sanitize(c.getString(1) ?: "file")
        if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
          val sub = ShareIntake.uniqueFile(dir, name).apply { mkdirs() }
          copyChildren(context, tree, id, sub, depth + 1)
        } else {
          try {
            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)
            context.contentResolver.openInputStream(uri)?.use { input ->
              ShareIntake.uniqueFile(dir, name).outputStream().use { input.copyTo(it) }
            }
          } catch (e: Exception) {
            // One unreadable file does not stop the rest of the folder.
            Log.w(TAG, "Could not copy $name", e)
          }
        }
      }
    }
  }

  private fun displayName(context: Context, uri: Uri): String? =
    context.contentResolver.query(
      uri, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null,
    )?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
}

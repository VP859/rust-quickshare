package io.github.vp859.ferry

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File

/**
 * Opens a received file in the app the user picks for its type (called from
 * Rust through JNI, see `platform.rs`; Rust allows only files received in this
 * run of the app).
 *
 * Received files are written to `Download/Ferry` and `Pictures/Ferry`, which
 * the app's FileProvider shares (`res/xml/file_paths.xml`): the other app gets
 * a `content://` URI with a read permission for this one file only, never the
 * path or access to the folder. A FileProvider URI is used rather than the
 * media store entry, which Android adds asynchronously after the scan.
 */
object FileOpener {
  private const val TAG = "FileOpener"

  /** Nothing went wrong. */
  private const val OK = ""
  /** No installed app opens this type of file. */
  private const val NO_APP = "no_app"
  /** The file is outside the folders the FileProvider shares, or gone. */
  private const val UNAVAILABLE = "unavailable"

  /** Opens [path]; returns [OK], [NO_APP] or [UNAVAILABLE] (also when Android refuses). */
  @JvmStatic
  fun open(context: Context, path: String): String {
    val file = File(path)
    if (!file.isFile) return UNAVAILABLE
    val uri = try {
      FileProvider.getUriForFile(context, context.packageName + ".fileprovider", file)
    } catch (e: IllegalArgumentException) {
      Log.w(TAG, "Not in a shared folder: $path", e)
      return UNAVAILABLE
    }
    val type = MimeTypeMap.getSingleton()
      .getMimeTypeFromExtension(file.extension.lowercase())
      ?: "application/octet-stream"
    val intent = Intent(Intent.ACTION_VIEW)
      .setDataAndType(uri, type)
      .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    // From the visible activity when there is one, so the other app opens on
    // top of Ferry and "back" returns to it. startActivity may be called from
    // any thread; it throws right away when no app handles the type (asking
    // the package manager first would need <queries> on Android 11+).
    val activity = FerryPermissions.activity?.get()
    return try {
      if (activity != null) {
        activity.startActivity(intent)
      } else {
        context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      }
      OK
    } catch (e: ActivityNotFoundException) {
      Log.w(TAG, "No app for $type", e)
      NO_APP
    } catch (e: RuntimeException) {
      // E.g. a SecurityException when Android refuses to start the other app.
      Log.w(TAG, "Could not open $path", e)
      UNAVAILABLE
    }
  }
}

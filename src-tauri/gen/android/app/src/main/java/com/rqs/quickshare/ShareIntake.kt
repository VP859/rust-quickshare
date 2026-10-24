package com.rqs.quickshare

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import androidx.core.content.IntentCompat
import java.io.File

/**
 * Receives files, texts and links shared to the app through the system "Share" menu.
 *
 * `content://` URIs have no path on disk, so their contents are copied into
 * `cacheDir/share-in/` and the resulting paths are appended to `pending.txt`,
 * which the Rust backend reads with the `take_shared_files` command. A shared
 * text is stored in `pending-text.txt` and read with `take_shared_text`.
 */
object ShareIntake {
  private const val TAG = "RqsShare"
  private const val FALLBACK_NAME = "file"
  private val lock = Any()

  /** Returns the URIs carried by an `ACTION_SEND` / `ACTION_SEND_MULTIPLE` intent. */
  fun extractUris(intent: Intent?): List<Uri> {
    if (intent == null) return emptyList()
    val uris = mutableListOf<Uri>()
    when (intent.action) {
      Intent.ACTION_SEND ->
        IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::add)
      Intent.ACTION_SEND_MULTIPLE ->
        IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::addAll)
      else -> return emptyList()
    }
    intent.clipData?.let { clip ->
      for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let(uris::add)
    }
    return uris.distinct()
  }

  /**
   * Returns the text of an `ACTION_SEND` intent that shares text (e.g. a link from a browser).
   *
   * Browsers such as Chrome attach a page thumbnail or favicon in `clipData` as a preview for
   * the share sheet, so for a text share (a `text` MIME type) the text wins over any attached image.
   */
  fun extractText(intent: Intent?): String? {
    if (intent?.action != Intent.ACTION_SEND) return null
    val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.takeIf { it.isNotBlank() } ?: return null
    val isTextShare = intent.type?.startsWith("text/") == true
    val hasStream = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java) != null
    return text.takeIf { isTextShare || !hasStream }
  }

  /** Stores a shared text for the frontend, replacing a previous one that was not picked up. */
  fun storeText(context: Context, text: String) {
    val dir = File(context.cacheDir, "share-in").apply { mkdirs() }
    synchronized(lock) { File(dir, "pending-text.txt").writeText(text) }
  }

  /**
   * Copies the given URIs into the share cache and records them in `pending.txt`.
   *
   * Performs blocking I/O and must not be called on the UI thread.
   *
   * @return the number of files copied successfully.
   */
  fun copyToCache(context: Context, uris: List<Uri>): Int {
    val dir = File(context.cacheDir, "share-in").apply { mkdirs() }
    val copied = uris.mapNotNull { uri ->
      try {
        val target = uniqueFile(dir, sanitize(displayName(context, uri)))
        context.contentResolver.openInputStream(uri)?.use { input ->
          target.outputStream().use { input.copyTo(it) }
        } ?: return@mapNotNull null
        target.absolutePath
      } catch (e: Exception) {
        Log.w(TAG, "Could not copy $uri", e)
        null
      }
    }
    if (copied.isNotEmpty()) {
      synchronized(lock) {
        File(dir, "pending.txt").appendText(copied.joinToString("\n", postfix = "\n"))
      }
    }
    return copied.size
  }

  private fun displayName(context: Context, uri: Uri): String {
    context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
      if (c.moveToFirst()) c.getString(0)?.let { return it }
    }
    return uri.lastPathSegment ?: FALLBACK_NAME
  }

  private fun sanitize(name: String): String =
    name.replace(Regex("[/\\\\:*?\"<>|\\p{Cntrl}]"), "_").trimStart('.').ifBlank { FALLBACK_NAME }.take(200)

  private fun uniqueFile(dir: File, name: String): File {
    var file = File(dir, name)
    val stem = name.substringBeforeLast('.', name)
    val ext = name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
    var n = 1
    while (file.exists()) file = File(dir, "$stem (${n++})$ext")
    return file
  }
}

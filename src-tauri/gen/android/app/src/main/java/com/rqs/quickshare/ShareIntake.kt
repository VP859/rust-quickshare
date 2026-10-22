package com.rqs.quickshare

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import androidx.core.content.IntentCompat
import java.io.File

/**
 * Przejmuje pliki z menu „Udostępnij”. URI `content://` nie mają ścieżki na dysku,
 * więc ich zawartość jest kopiowana do `cacheDir/share-in/`, a ścieżki dopisywane do
 * `pending.txt`, skąd odczytuje je backend Rust (`take_shared_files`).
 */
object ShareIntake {
  private const val TAG = "RqsShare"
  private val lock = Any()

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

  /** Zwraca liczbę skopiowanych plików. Wywoływać poza wątkiem UI. */
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
        Log.w(TAG, "Nie udało się skopiować $uri", e)
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
    return uri.lastPathSegment ?: "plik"
  }

  private fun sanitize(name: String): String =
    name.replace(Regex("[/\\\\:*?\"<>|\\p{Cntrl}]"), "_").trimStart('.').ifBlank { "plik" }.take(200)

  private fun uniqueFile(dir: File, name: String): File {
    var file = File(dir, name)
    val stem = name.substringBeforeLast('.', name)
    val ext = name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
    var n = 1
    while (file.exists()) file = File(dir, "$stem (${n++})$ext")
    return file
  }
}

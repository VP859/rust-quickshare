package io.github.vp859.ferry

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.util.Log
import androidx.core.app.Person
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import org.json.JSONArray
import java.io.File

/**
 * Paired devices as Direct Share targets: they appear at the top of the
 * system "Share" menu, so a photo goes to "Laptop" without opening Ferry first.
 *
 * Rust passes the paired devices whenever their list changes
 * ([update], called through JNI, see `platform.rs`); each becomes a
 * long-lived dynamic shortcut in the category declared by `res/xml/shortcuts.xml`.
 * Choosing one starts [MainActivity] with the share intent and the shortcut ID,
 * which [storeTarget] keeps for the frontend (`take_share_target`).
 *
 * Android 10 and newer show these targets; older versions only list Ferry
 * itself in the menu.
 */
object ShareTargets {
  private const val TAG = "FerryShareTargets"
  /** Category of the `<share-target>` in `res/xml/shortcuts.xml`. */
  private const val CATEGORY = "io.github.vp859.ferry.category.SHARE_TARGET"
  /** Shortcut IDs are `peer-<Node ID>`. */
  private const val PREFIX = "peer-"
  private const val TARGET_FILE = "pending-target.txt"
  private const val ICON_SIZE = 108

  /**
   * Replaces the share targets with the devices in [json], a JSON array of
   * `{"id": Node ID, "name": alias}` (most important first; Android shows
   * only the first few).
   */
  @JvmStatic
  fun update(context: Context, json: String) {
    try {
      val devices = JSONArray(json)
      val max = ShortcutManagerCompat.getMaxShortcutCountPerActivity(context).coerceAtLeast(1)
      val shortcuts = (0 until minOf(devices.length(), max)).map { i ->
        val device = devices.getJSONObject(i)
        shortcut(context, device.getString("id"), device.getString("name"), i)
      }
      val kept = shortcuts.map { it.id }.toSet()
      val gone = ShortcutManagerCompat.getDynamicShortcuts(context)
        .map { it.id }
        .filter { it.startsWith(PREFIX) && it !in kept }
      ShortcutManagerCompat.setDynamicShortcuts(context, shortcuts)
      // Long-lived shortcuts stay cached by the system: an unpaired device
      // must not stay in the share menu.
      if (gone.isNotEmpty()) ShortcutManagerCompat.removeLongLivedShortcuts(context, gone)
    } catch (e: Exception) {
      Log.w(TAG, "Could not update the share targets", e)
    }
  }

  private fun shortcut(context: Context, id: String, name: String, rank: Int): ShortcutInfoCompat {
    val person = Person.Builder().setName(name).setKey(id).build()
    return ShortcutInfoCompat.Builder(context, PREFIX + id)
      .setShortLabel(name)
      .setLongLabel(name)
      .setIcon(IconCompat.createWithAdaptiveBitmap(initials(name)))
      // Required by Android; the share intent itself replaces it when chosen.
      .setIntent(Intent(context, MainActivity::class.java).setAction(Intent.ACTION_SEND))
      .setCategories(setOf(CATEGORY))
      .setLongLived(true)
      .setPerson(person)
      .setRank(rank)
      .build()
  }

  /** A round icon with the initials of [name], like the avatars in the app. */
  private fun initials(name: String): Bitmap {
    // Adaptive icons are cut to the inner 72 of 108 units.
    val bitmap = Bitmap.createBitmap(ICON_SIZE, ICON_SIZE, Bitmap.Config.ARGB_8888)
    val canvas = Canvas(bitmap)
    canvas.drawColor(Color.rgb(37, 99, 235))
    val letters = name.trim().split(Regex("\\s+")).take(2)
      .mapNotNull { word -> word.firstOrNull()?.toString() }
      .joinToString("").uppercase().ifEmpty { "?" }
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
      color = Color.WHITE
      textAlign = Paint.Align.CENTER
      textSize = ICON_SIZE * 0.3f
      isFakeBoldText = true
    }
    val y = ICON_SIZE / 2f - (paint.descent() + paint.ascent()) / 2f
    canvas.drawText(letters, ICON_SIZE / 2f, y, paint)
    return bitmap
  }

  /** The Node ID of the device chosen in the share menu, if one was. */
  fun targetOf(intent: Intent?): String? =
    intent?.getStringExtra(ShortcutManagerCompat.EXTRA_SHORTCUT_ID)
      ?.takeIf { it.startsWith(PREFIX) }
      ?.removePrefix(PREFIX)

  /**
   * Keeps the chosen device for the frontend next to the shared files, or
   * forgets an old one when Ferry itself was chosen ([target] `null`).
   */
  fun storeTarget(context: Context, target: String?) {
    val file = File(File(context.cacheDir, "share-in").apply { mkdirs() }, TARGET_FILE)
    if (target == null) file.delete() else file.writeText(target)
  }
}

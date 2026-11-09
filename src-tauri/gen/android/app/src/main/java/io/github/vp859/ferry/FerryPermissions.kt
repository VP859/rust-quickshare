package io.github.vp859.ferry

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.lang.ref.WeakReference

/**
 * Runtime permissions, asked for only after the app said why (called from Rust
 * through JNI, see `platform.rs`; the frontend shows the explanation).
 *
 * - Local network (Android 17+, API 37): needed for any traffic to devices on
 *   the local network – finding them, sending and receiving. Without it Ferry
 *   works only through relays in the internet mode.
 * - Notifications (Android 13+): incoming files and texts while the app is in
 *   the background.
 * - Storage (Android 10 and older): saving received files to Downloads.
 */
object FerryPermissions {
  private const val TAG = "FerryPermissions"
  const val REQUEST = 1

  /** Required on Android 17 (API 37) for any local network traffic. */
  const val ACCESS_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
  private const val API_ANDROID_17 = 37
  private const val PREFS = "ferry_permissions"

  /** The activity that asks (set by [MainActivity]). */
  @Volatile
  var activity: WeakReference<Activity>? = null

  private fun localNetworkNeeded() = Build.VERSION.SDK_INT >= API_ANDROID_17
  private fun notificationsNeeded() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
  private fun storageNeeded() = Build.VERSION.SDK_INT <= Build.VERSION_CODES.Q

  /** Permissions this Android version needs and that are not granted yet. */
  fun missing(context: Context): List<String> = buildList {
    if (notificationsNeeded()) add(Manifest.permission.POST_NOTIFICATIONS)
    if (storageNeeded()) add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
    if (localNetworkNeeded()) add(ACCESS_LOCAL_NETWORK)
  }.filter { ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED }

  private fun state(context: Context, needed: Boolean, permission: String): String = when {
    !needed -> "not_needed"
    ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED -> "granted"
    else -> "denied"
  }

  /**
   * The state as JSON: `local_network`, `notifications` and `storage` are
   * `granted`, `denied` or `not_needed`; `can_ask` is false when Android will no longer
   * show its dialog (refused twice, or "don't ask again"), so only the app
   * settings can change it.
   */
  @JvmStatic
  fun status(context: Context): String {
    val json = JSONObject()
    try {
      json.put("local_network", state(context, localNetworkNeeded(), ACCESS_LOCAL_NETWORK))
      json.put("notifications", state(context, notificationsNeeded(), Manifest.permission.POST_NOTIFICATIONS))
      json.put("storage", state(context, storageNeeded(), Manifest.permission.WRITE_EXTERNAL_STORAGE))
      val asked = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean("asked", false)
      val current = activity?.get()
      // After a refusal Android shows its dialog again only while it would show a
      // rationale; before the first request it always does.
      val canAsk = !asked || (current != null && missing(context).any {
        ActivityCompat.shouldShowRequestPermissionRationale(current, it)
      })
      json.put("can_ask", canAsk)
    } catch (e: Exception) {
      Log.w(TAG, "Could not read the permissions", e)
    }
    return json.toString()
  }

  /** Shows Android's permission dialog for everything missing. */
  @JvmStatic
  fun request(context: Context) {
    val current = activity?.get() ?: return
    val wanted = missing(context)
    if (wanted.isEmpty()) return
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("asked", true).apply()
    current.runOnUiThread {
      ActivityCompat.requestPermissions(current, wanted.toTypedArray(), REQUEST)
    }
  }
}

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
 * Runtime permissions (called from Rust through JNI, see `platform.rs`).
 *
 * Android's dialog appears by itself on the first start ([requestAtStart]) and
 * again for the local network when the user starts a search; the frontend
 * explains each permission on a card and leads to the app settings once
 * Android stops asking. The camera is asked for by the QR scanner plugin, only
 * when the user scans a code.
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

  /** Android's dialog is open (the frontend hides its card meanwhile). */
  @Volatile
  var asking = false

  private fun localNetworkNeeded() = Build.VERSION.SDK_INT >= API_ANDROID_17
  private fun notificationsNeeded() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
  private fun storageNeeded() = Build.VERSION.SDK_INT <= Build.VERSION_CODES.Q

  /** Permissions this Android version needs and that are not granted yet. */
  fun missing(context: Context): List<String> = buildList {
    if (notificationsNeeded()) add(Manifest.permission.POST_NOTIFICATIONS)
    if (storageNeeded()) add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
    if (localNetworkNeeded()) add(ACCESS_LOCAL_NETWORK)
  }.filter { ContextCompat.checkSelfPermission(context, it) != PackageManager.PERMISSION_GRANTED }

  private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

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
      val asked = prefs(context).getBoolean("asked", false)
      val current = activity?.get()
      // After a refusal Android shows its dialog again only while it would show a
      // rationale; before the first request it always does.
      val canAsk = !asked || (current != null && missing(context).any {
        ActivityCompat.shouldShowRequestPermissionRationale(current, it)
      })
      json.put("can_ask", canAsk)
      json.put("asking", asking)
    } catch (e: Exception) {
      Log.w(TAG, "Could not read the permissions", e)
    }
    return json.toString()
  }

  /**
   * Shows Android's permission dialog: for everything missing, or only for
   * the local network when [only] is `local_network`.
   */
  @JvmStatic
  fun request(context: Context, only: String) {
    val current = activity?.get()
    if (current == null) {
      Log.w(TAG, "No activity to ask for permissions")
      return
    }
    ask(current, missing(context).filter { only != "local_network" || it == ACCESS_LOCAL_NETWORK })
  }

  /** On the very first start: asks for everything missing, once. */
  fun requestAtStart(current: Activity) {
    if (prefs(current).getBoolean("asked", false)) return
    ask(current, missing(current))
  }

  /** Android answered the dialog (see `MainActivity.onRequestPermissionsResult`). */
  fun answered() {
    asking = false
  }

  private fun ask(current: Activity, wanted: List<String>) {
    if (wanted.isEmpty()) return
    prefs(current).edit().putBoolean("asked", true).apply()
    asking = true
    current.runOnUiThread {
      ActivityCompat.requestPermissions(current, wanted.toTypedArray(), REQUEST)
    }
  }
}

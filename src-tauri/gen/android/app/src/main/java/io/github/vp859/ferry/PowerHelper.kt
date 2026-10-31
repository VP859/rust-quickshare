package io.github.vp859.ferry

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import org.json.JSONObject

/**
 * What limits Ferry in the background, for the settings screen (called from Rust
 * through JNI, see `platform.rs`).
 *
 * Nothing here asks for special permissions: the user changes the settings
 * themselves in the system app info screen opened by [openAppSettings].
 */
object PowerHelper {
  private const val TAG = "FerryPower"

  /**
   * The current state as JSON:
   * - `battery_optimized` – battery optimization applies (Doze may pause the
   *   network for minutes while the screen is off);
   * - `power_save` – battery saver is on;
   * - `background_restricted` – the user restricted the app in the background;
   *   Android then refuses to start the foreground service;
   * - `data_saver` – data saver blocks background traffic on metered networks.
   */
  @JvmStatic
  fun status(context: Context): String {
    val json = JSONObject()
    try {
      val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager
      json.put("battery_optimized", !power.isIgnoringBatteryOptimizations(context.packageName))
      json.put("power_save", power.isPowerSaveMode)
      val activity = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
      json.put(
        "background_restricted",
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && activity.isBackgroundRestricted
      )
      val connectivity = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
      json.put(
        "data_saver",
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.N &&
          connectivity.restrictBackgroundStatus == ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED
      )
    } catch (e: Exception) {
      Log.w(TAG, "Could not read the power state", e)
    }
    return json.toString()
  }

  /**
   * Opens the system app info screen of Ferry, where "Battery" (or "App battery
   * usage") can be set to "Unrestricted".
   */
  @JvmStatic
  fun openAppSettings(context: Context) {
    try {
      context.startActivity(
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
          .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      )
    } catch (e: Exception) {
      Log.w(TAG, "Could not open the app settings", e)
    }
  }
}

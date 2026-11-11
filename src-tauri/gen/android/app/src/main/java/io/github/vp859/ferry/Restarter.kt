package io.github.vp859.ferry

import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper

/**
 * Restarts Ferry from the "Restart now" button (called from Rust through JNI,
 * see `platform.rs`), e.g. after a new network mode or a restored device key.
 *
 * Tauri cannot restart a mobile app and the node keeps its settings for the
 * life of the process, so the process ends: a new task with [MainActivity] is
 * started first and Android runs it in a fresh process, as
 * `MainActivity.restartProcess` does. Transfers in progress are interrupted.
 */
object Restarter {
  @JvmStatic
  fun restart(context: Context) {
    val start = Intent(context, MainActivity::class.java)
      .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
    // On the UI thread, so the exit follows the start request directly and
    // no new activity is created in this process before it ends.
    Handler(Looper.getMainLooper()).post {
      (FerryPermissions.activity?.get() ?: context).startActivity(start)
      Runtime.getRuntime().exit(0)
    }
  }
}

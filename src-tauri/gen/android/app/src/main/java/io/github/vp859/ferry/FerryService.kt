package io.github.vp859.ferry

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service (`dataSync`) with a persistent notification.
 *
 * The service performs no transfers itself: the iroh node runs inside the app
 * process (Rust). Keeping a foreground service alive prevents Android from killing
 * that process once the app is moved to the background. The service also holds a
 * Wi-Fi [WifiManager.MulticastLock], without which the Wi-Fi driver drops the
 * broadcast/multicast packets used for local device discovery.
 */
class FerryService : Service() {
  companion object {
    private const val TAG = "FerryService"
    private const val CHANNEL_ID = "ferry_node"
    private const val NOTIFICATION_ID = 1001
    private const val ACTION_STOP = "io.github.vp859.ferry.STOP"

    /** Starts the service; failures (e.g. starting from the background) are logged. */
    fun start(context: Context) {
      try {
        ContextCompat.startForegroundService(context, Intent(context, FerryService::class.java))
      } catch (e: Exception) {
        Log.w(TAG, "Could not start the foreground service", e)
      }
    }
  }

  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  private fun acquireMulticastLock() {
    if (multicastLock?.isHeld == true) return
    try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      multicastLock = wifi.createMulticastLock("ferry-discovery").apply {
        setReferenceCounted(false)
        acquire()
      }
    } catch (e: Exception) {
      Log.w(TAG, "No multicast lock - local network discovery may not work", e)
    }
  }

  override fun onDestroy() {
    multicastLock?.takeIf { it.isHeld }?.release()
    multicastLock = null
    super.onDestroy()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      multicastLock?.takeIf { it.isHeld }?.release()
      ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
      stopSelf()
      return START_NOT_STICKY
    }
    createChannel()
    acquireMulticastLock()
    ServiceCompat.startForeground(
      this,
      NOTIFICATION_ID,
      buildNotification(),
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0
    )
    return START_STICKY
  }

  /** Android 15+ limits `dataSync` services; the service stops when the system asks. */
  override fun onTimeout(startId: Int, fgsType: Int) {
    stopSelf(startId)
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val channel = NotificationChannel(
      CHANNEL_ID,
      getString(R.string.notif_channel_node),
      NotificationManager.IMPORTANCE_LOW
    ).apply { description = getString(R.string.notif_channel_node_desc) }
    getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }

  private fun buildNotification() = NotificationCompat.Builder(this, CHANNEL_ID)
    .setSmallIcon(R.drawable.ic_stat_ferry)
    .setContentTitle(getString(R.string.notif_node_title))
    .setContentText(getString(R.string.notif_node_text))
    .setOngoing(true)
    .setPriority(NotificationCompat.PRIORITY_LOW)
    .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
    .setContentIntent(
      PendingIntent.getActivity(
        this, 0,
        Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
      )
    )
    .addAction(
      0, getString(R.string.notif_action_stop),
      PendingIntent.getService(
        this, 1,
        Intent(this, FerryService::class.java).setAction(ACTION_STOP),
        PendingIntent.FLAG_IMMUTABLE
      )
    )
    .build()
}

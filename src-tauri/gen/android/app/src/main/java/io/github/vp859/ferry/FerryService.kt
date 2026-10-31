package io.github.vp859.ferry

import android.annotation.SuppressLint
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
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Foreground service with a persistent notification.
 *
 * The service performs no transfers itself: the iroh node runs inside the app
 * process (Rust). Keeping a foreground service alive prevents Android from killing
 * that process once the app is moved to the background. The service also holds a
 * Wi-Fi [WifiManager.MulticastLock], without which the Wi-Fi driver drops the
 * broadcast/multicast packets used for local device discovery.
 *
 * Type `connectedDevice` (data exchange with other devices over the network) has
 * no daily time limit. `dataSync` is only a fallback for devices that refuse the
 * first type; Android 15+ stops it after 6 hours a day ([onTimeout]).
 *
 * While transfers run ([setTransfersActive], called from Rust) the process holds a
 * partial wake lock and a Wi-Fi lock, so a phone with the screen off neither sleeps
 * nor slows Wi-Fi down in the middle of a transfer.
 */
class FerryService : Service() {
  companion object {
    private const val TAG = "FerryService"
    private const val CHANNEL_ID = "ferry_node"
    private const val NOTIFICATION_ID = 1001
    private const val STOPPED_NOTIFICATION_ID = 1002
    private const val ACTION_STOP = "io.github.vp859.ferry.STOP"

    /** Upper bound of one wake lock, in case the "transfers ended" call never comes. */
    private const val WAKE_LOCK_TIMEOUT_MS = 2 * 60 * 60 * 1000L

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    @Volatile private var transferring = false

    /** Starts the service; failures (e.g. starting from the background) are logged. */
    fun start(context: Context) {
      try {
        ContextCompat.startForegroundService(context, Intent(context, FerryService::class.java))
      } catch (e: Exception) {
        Log.w(TAG, "Could not start the foreground service", e)
      }
    }

    /**
     * Called from Rust when the first transfer starts (`true`) and when the last
     * one ends (`false`): holds or releases the wake and Wi-Fi locks and updates
     * the notification.
     */
    @JvmStatic
    @SuppressLint("WakelockTimeout")
    fun setTransfersActive(context: Context, active: Boolean) {
      synchronized(this) {
        if (active == transferring) return
        transferring = active
        val app = context.applicationContext
        if (active) {
          try {
            wakeLock = (app.getSystemService(Context.POWER_SERVICE) as PowerManager)
              .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "ferry:transfer")
              .apply { setReferenceCounted(false); acquire(WAKE_LOCK_TIMEOUT_MS) }
            @Suppress("DEPRECATION")
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
              WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
              WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = (app.getSystemService(Context.WIFI_SERVICE) as WifiManager)
              .createWifiLock(mode, "ferry:transfer")
              .apply { setReferenceCounted(false); acquire() }
          } catch (e: Exception) {
            Log.w(TAG, "Could not hold the transfer locks", e)
          }
        } else {
          wakeLock?.takeIf { it.isHeld }?.release()
          wifiLock?.takeIf { it.isHeld }?.release()
          wakeLock = null
          wifiLock = null
        }
        try {
          app.getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, buildNotification(app))
        } catch (e: Exception) {
          Log.w(TAG, "Could not update the notification", e)
        }
      }
    }

    private fun buildNotification(context: Context) = NotificationCompat.Builder(context, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_stat_ferry)
      .setContentTitle(context.getString(R.string.notif_node_title))
      .setContentText(
        context.getString(if (transferring) R.string.notif_node_transferring else R.string.notif_node_text)
      )
      .setOngoing(true)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .setContentIntent(openApp(context))
      .addAction(
        0, context.getString(R.string.notif_action_stop),
        PendingIntent.getService(
          context, 1,
          Intent(context, FerryService::class.java).setAction(ACTION_STOP),
          PendingIntent.FLAG_IMMUTABLE
        )
      )
      .build()

    private fun openApp(context: Context) = PendingIntent.getActivity(
      context, 0,
      Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )
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
    setTransfersActive(this, false)
    super.onDestroy()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      stopEverything()
      return START_NOT_STICKY
    }
    createChannel()
    acquireMulticastLock()
    startInForeground()
    return START_STICKY
  }

  /** `connectedDevice` first; `dataSync` if this device refuses it. */
  private fun startInForeground() {
    val notification = buildNotification(this)
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
      ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, 0)
      return
    }
    try {
      ServiceCompat.startForeground(
        this, NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
      )
    } catch (e: Exception) {
      Log.w(TAG, "connectedDevice refused, falling back to dataSync", e)
      ServiceCompat.startForeground(
        this, NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
      )
    }
  }

  private fun stopEverything() {
    multicastLock?.takeIf { it.isHeld }?.release()
    setTransfersActive(this, false)
    ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
    stopSelf()
  }

  /**
   * Android 15+ ends a `dataSync` service after its daily time; it must stop within
   * seconds or the app is killed. Stops completely (not only this start) and tells
   * the user how to continue.
   */
  override fun onTimeout(startId: Int, fgsType: Int) {
    Log.w(TAG, "Foreground service time limit reached (type $fgsType)")
    stopEverything()
    try {
      val notification = NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(R.drawable.ic_stat_ferry)
        .setContentTitle(getString(R.string.notif_stopped_title))
        .setContentText(getString(R.string.notif_stopped_text))
        .setStyle(NotificationCompat.BigTextStyle().bigText(getString(R.string.notif_stopped_text)))
        .setAutoCancel(true)
        .setContentIntent(openApp(this))
        .build()
      getSystemService(NotificationManager::class.java).notify(STOPPED_NOTIFICATION_ID, notification)
    } catch (e: Exception) {
      Log.w(TAG, "Could not show the stop notification", e)
    }
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
}

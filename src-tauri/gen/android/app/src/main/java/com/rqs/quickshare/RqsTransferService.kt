package com.rqs.quickshare

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
 * Usługa pierwszoplanowa (dataSync) z powiadomieniem ciągłym. Sama nie wykonuje
 * transferów – węzeł iroh działa w procesie aplikacji (Rust); usługa sprawia,
 * że system nie zabija tego procesu po zminimalizowaniu aplikacji.
 */
class RqsTransferService : Service() {
  companion object {
    private const val TAG = "RqsService"
    private const val CHANNEL_ID = "rqs_node"
    private const val NOTIFICATION_ID = 1001
    private const val ACTION_STOP = "com.rqs.quickshare.STOP"

    fun start(context: Context) {
      try {
        ContextCompat.startForegroundService(context, Intent(context, RqsTransferService::class.java))
      } catch (e: Exception) {
        // Np. ForegroundServiceStartNotAllowedException przy starcie z tła.
        Log.w(TAG, "Nie udało się uruchomić usługi pierwszoplanowej", e)
      }
    }
  }

  /** Bez blokady sterownik Wi-Fi odrzuca pakiety broadcast/multicast wykrywania urządzeń. */
  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  private fun acquireMulticastLock() {
    if (multicastLock?.isHeld == true) return
    try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      multicastLock = wifi.createMulticastLock("rqs-discovery").apply {
        setReferenceCounted(false)
        acquire()
      }
    } catch (e: Exception) {
      Log.w(TAG, "Brak blokady multicast – wykrywanie w sieci lokalnej może nie działać", e)
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

  /** Android 15+: limit czasu usług dataSync – kończymy zgodnie z wymaganiami systemu. */
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
    .setSmallIcon(R.drawable.ic_stat_rqs)
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
        Intent(this, RqsTransferService::class.java).setAction(ACTION_STOP),
        PendingIntent.FLAG_IMMUTABLE
      )
    )
    .build()
}

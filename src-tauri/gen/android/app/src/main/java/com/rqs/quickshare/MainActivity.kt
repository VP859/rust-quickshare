package com.rqs.quickshare

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  companion object {
    private const val PERMISSIONS_REQUEST = 1
    /** Android 17 (API 37): ruch w sieci lokalnej (broadcast, multicast, połączenia do
     *  adresów LAN) wymaga uprawnienia z grupy „Urządzenia w pobliżu”. */
    private const val ACCESS_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
    private const val API_ANDROID_17 = 37
  }

  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    requestRuntimePermissions()
    // Węzeł P2P żyje w tym procesie – usługa pierwszoplanowa chroni go przed zabiciem w tle.
    RqsTransferService.start(this)
    if (savedInstanceState == null) handleShareIntent(intent)
  }

  override fun onWebViewCreate(webView: WebView) {
    this.webView = webView
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    handleShareIntent(intent)
  }

  /** ACTION_SEND / ACTION_SEND_MULTIPLE: kopiuje pliki i otwiera ekran wyboru odbiorcy. */
  private fun handleShareIntent(intent: Intent?) {
    val uris = ShareIntake.extractUris(intent)
    if (uris.isEmpty()) return
    Thread {
      if (ShareIntake.copyToCache(this, uris) > 0) {
        runOnUiThread {
          webView?.evaluateJavascript("window.dispatchEvent(new Event('rqs-share'))", null)
        }
      }
    }.start()
  }

  private fun requestRuntimePermissions() {
    val wanted = buildList {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS)
      if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.P) add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
      // Bez tego Android 17 blokuje wykrywanie urządzeń i bezpośrednie transfery w LAN
      // (zostaje tylko relay przez internet).
      if (Build.VERSION.SDK_INT >= API_ANDROID_17) add(ACCESS_LOCAL_NETWORK)
    }.filter { ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED }
    if (wanted.isNotEmpty()) ActivityCompat.requestPermissions(this, wanted.toTypedArray(), PERMISSIONS_REQUEST)
  }

  override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
    super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    if (requestCode != PERMISSIONS_REQUEST) return
    val lanGranted = permissions.indices.any {
      permissions[it] == ACCESS_LOCAL_NETWORK && grantResults.getOrNull(it) == PackageManager.PERMISSION_GRANTED
    }
    // Interfejs od razu ponawia wyszukiwanie urządzeń w sieci lokalnej.
    if (lanGranted) {
      webView?.evaluateJavascript("window.dispatchEvent(new Event('rqs-lan-permission'))", null)
    }
  }
}

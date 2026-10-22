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
    }.filter { ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED }
    if (wanted.isNotEmpty()) ActivityCompat.requestPermissions(this, wanted.toTypedArray(), 1)
  }
}

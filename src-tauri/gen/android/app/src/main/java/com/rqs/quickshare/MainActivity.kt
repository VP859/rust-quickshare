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

/**
 * Entry activity hosting the Tauri WebView.
 *
 * Requests runtime permissions, keeps the P2P node alive through [RqsTransferService]
 * and forwards files and texts from the system "Share" menu to the frontend.
 *
 * Events dispatched to the WebView (`window`):
 * - `rqs-share` – shared files or a shared text are ready to be fetched with
 *   `take_shared_files` / `take_shared_text`;
 * - `rqs-lan-permission` – local network access was granted (Android 17+).
 */
class MainActivity : TauriActivity() {
  companion object {
    private const val PERMISSIONS_REQUEST = 1

    /** Required on Android 17 (API 37) for any local network traffic. */
    private const val ACCESS_LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
    private const val API_ANDROID_17 = 37
  }

  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    requestRuntimePermissions()
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

  /** Takes files or a text from `ACTION_SEND` / `ACTION_SEND_MULTIPLE` and notifies the frontend. */
  private fun handleShareIntent(intent: Intent?) {
    val text = ShareIntake.extractText(intent)
    if (text != null) {
      ShareIntake.storeText(this, text)
      notifyShare()
      return
    }
    val uris = ShareIntake.extractUris(intent)
    if (uris.isEmpty()) return
    Thread {
      if (ShareIntake.copyToCache(this, uris) > 0) runOnUiThread { notifyShare() }
    }.start()
  }

  private fun notifyShare() {
    webView?.evaluateJavascript("window.dispatchEvent(new Event('rqs-share'))", null)
  }

  private fun requestRuntimePermissions() {
    val wanted = buildList {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS)
      if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.Q) add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
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
    if (lanGranted) {
      webView?.evaluateJavascript("window.dispatchEvent(new Event('rqs-lan-permission'))", null)
    }
  }
}

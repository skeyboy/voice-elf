package com.voiceelf.client

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private var pendingCapture: CaptureRequest? = null
  private var pendingOverlayPayload: String? = null
  private var safeInsetTop = 0f
  private var safeInsetRight = 0f
  private var safeInsetBottom = 0f
  private var safeInsetLeft = 0f

  private val microphonePermissionLauncher = registerForActivityResult(
    ActivityResultContracts.RequestPermission(),
  ) { granted ->
    if (!granted) {
      pendingCapture = null
      emitNativeEvent(
        JSONObject()
          .put("type", "capture-error")
          .put("message", "麦克风权限未开启，请在系统设置中允许 Voice Elf 使用麦克风后重试"),
      )
      return@registerForActivityResult
    }
    requestNotificationPermissionThenContinue()
  }

  private val notificationPermissionLauncher = registerForActivityResult(
    ActivityResultContracts.RequestPermission(),
  ) {
    // Notification permission only controls visibility; it must not block capture.
    continueCaptureRequest()
  }

  private val projectionLauncher = registerForActivityResult(
    ActivityResultContracts.StartActivityForResult(),
  ) { result ->
    val request = pendingCapture
    if (request == null) return@registerForActivityResult
    if (result.resultCode != Activity.RESULT_OK || result.data == null) {
      pendingCapture = null
      emitNativeEvent(JSONObject().put("type", "capture-error").put("message", "未授予系统内录权限"))
      return@registerForActivityResult
    }
    startCaptureService(request, result.resultCode, result.data)
    pendingCapture = null
  }

  private val overlayPermissionLauncher = registerForActivityResult(
    ActivityResultContracts.StartActivityForResult(),
  ) {
    val payload = pendingOverlayPayload
    pendingOverlayPayload = null
    if (payload == null) return@registerForActivityResult
    if (!Settings.canDrawOverlays(this)) {
      emitNativeEvent(JSONObject().put("type", "overlay-error").put("message", "需要悬浮窗权限才能显示字幕大屏"))
      return@registerForActivityResult
    }
    startSubtitleOverlay(payload)
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    appWebView = webView
    ViewCompat.setOnApplyWindowInsetsListener(webView) { view, insets ->
      val safeInsets = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout(),
      )
      val density = resources.displayMetrics.density
      safeInsetTop = safeInsets.top / density
      safeInsetRight = safeInsets.right / density
      safeInsetBottom = safeInsets.bottom / density
      safeInsetLeft = safeInsets.left / density
      applySafeAreaToWebView()
      insets
    }
    ViewCompat.requestApplyInsets(webView)
    webView.addJavascriptInterface(AndroidBridge(webView), "VoiceElfAndroid")
    VoiceCaptureService.nativeEventSink = ::emitNativeEvent
    SubtitleOverlayService.nativeEventSink = ::emitNativeEvent
    openRequestedRoute(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    openRequestedRoute(intent)
  }

  override fun onDestroy() {
    if (isFinishing) SubtitleOverlayService.clearSessionPosition(this)
    if (VoiceCaptureService.nativeEventSink != null) VoiceCaptureService.nativeEventSink = null
    if (SubtitleOverlayService.nativeEventSink != null) SubtitleOverlayService.nativeEventSink = null
    appWebView = null
    super.onDestroy()
  }

  private inner class AndroidBridge(private val webView: WebView) {
    @JavascriptInterface
    fun platform() = "android"

    @JavascriptInterface
    fun safeAreaInsets() = safeAreaPayload().toString()

    @JavascriptInterface
    fun startCapture(
      microphone: Boolean,
      systemAudio: Boolean,
      noiseSuppression: Boolean,
      echoCancellation: Boolean,
    ) {
      runTrusted(webView) {
        if (!microphone && !systemAudio) {
          emitNativeEvent(JSONObject().put("type", "capture-error").put("message", "请至少选择一个音频来源"))
          return@runTrusted
        }
        pendingCapture = CaptureRequest(
          microphone,
          systemAudio,
          noiseSuppression,
          echoCancellation,
        )
        if (!hasPermission(Manifest.permission.RECORD_AUDIO)) {
          microphonePermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
        } else {
          requestNotificationPermissionThenContinue()
        }
      }
    }

    @JavascriptInterface
    fun updateCapture(
      microphone: Boolean,
      systemAudio: Boolean,
      noiseSuppression: Boolean,
      echoCancellation: Boolean,
    ) = runTrusted(webView) {
      startService(Intent(this@MainActivity, VoiceCaptureService::class.java).apply {
        action = VoiceCaptureService.ACTION_UPDATE
        putExtra(VoiceCaptureService.EXTRA_MICROPHONE, microphone)
        putExtra(VoiceCaptureService.EXTRA_SYSTEM_AUDIO, systemAudio)
        putExtra(VoiceCaptureService.EXTRA_NOISE_SUPPRESSION, noiseSuppression)
        putExtra(VoiceCaptureService.EXTRA_ECHO_CANCELLATION, echoCancellation)
      })
    }

    @JavascriptInterface
    fun captureReady() = runTrusted(webView) {
      startService(Intent(this@MainActivity, VoiceCaptureService::class.java).apply {
        action = VoiceCaptureService.ACTION_CAPTURE_READY
      })
    }

    @JavascriptInterface
    fun stopCapture() = runTrusted(webView) {
      startService(Intent(this@MainActivity, VoiceCaptureService::class.java).apply {
        action = VoiceCaptureService.ACTION_STOP
      })
    }

    @JavascriptInterface
    fun showSubtitleOverlay(payload: String) = runTrusted(webView) {
      if (Settings.canDrawOverlays(this@MainActivity)) {
        startSubtitleOverlay(payload)
      } else {
        pendingOverlayPayload = payload
        overlayPermissionLauncher.launch(
          Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")),
        )
      }
    }

    @JavascriptInterface
    fun updateSubtitleOverlay(payload: String) = runTrusted(webView) {
      startService(Intent(this@MainActivity, SubtitleOverlayService::class.java).apply {
        action = SubtitleOverlayService.ACTION_UPDATE
        putExtra(SubtitleOverlayService.EXTRA_PAYLOAD, payload)
      })
    }

    @JavascriptInterface
    fun hideSubtitleOverlay() = runTrusted(webView) {
      startService(Intent(this@MainActivity, SubtitleOverlayService::class.java).apply {
        action = SubtitleOverlayService.ACTION_HIDE
      })
    }

    @JavascriptInterface
    fun subtitleOverlayVisible() = SubtitleOverlayService.visible
  }

  private fun continueCaptureRequest() {
    val request = pendingCapture ?: return
    if (!request.systemAudio) {
      startCaptureService(request, null, null)
      pendingCapture = null
      return
    }
    val manager = getSystemService(MediaProjectionManager::class.java)
    projectionLauncher.launch(manager.createScreenCaptureIntent())
  }

  private fun requestNotificationPermissionThenContinue() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
      !hasPermission(Manifest.permission.POST_NOTIFICATIONS)
    ) {
      notificationPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
    } else {
      continueCaptureRequest()
    }
  }

  private fun startCaptureService(request: CaptureRequest, resultCode: Int?, data: Intent?) {
    val intent = Intent(this, VoiceCaptureService::class.java).apply {
      action = VoiceCaptureService.ACTION_START
      putExtra(VoiceCaptureService.EXTRA_MICROPHONE, request.microphone)
      putExtra(VoiceCaptureService.EXTRA_SYSTEM_AUDIO, request.systemAudio)
      putExtra(VoiceCaptureService.EXTRA_NOISE_SUPPRESSION, request.noiseSuppression)
      putExtra(VoiceCaptureService.EXTRA_ECHO_CANCELLATION, request.echoCancellation)
      resultCode?.let { putExtra(VoiceCaptureService.EXTRA_RESULT_CODE, it) }
      data?.let { putExtra(VoiceCaptureService.EXTRA_RESULT_DATA, it) }
    }
    ContextCompat.startForegroundService(this, intent)
  }

  private fun startSubtitleOverlay(payload: String) {
    ContextCompat.startForegroundService(
      this,
      Intent(this, SubtitleOverlayService::class.java).apply {
        action = SubtitleOverlayService.ACTION_SHOW
        putExtra(SubtitleOverlayService.EXTRA_PAYLOAD, payload)
      },
    )
  }

  private fun runTrusted(webView: WebView, block: () -> Unit) {
    runOnUiThread {
      val uri = webView.url?.let(Uri::parse)
      if (uri?.host == "127.0.0.1" || uri?.host == "localhost") block()
      else emitNativeEvent(JSONObject().put("type", "capture-error").put("message", "已阻止非本地页面调用原生录音能力"))
    }
  }

  private fun hasPermission(permission: String) =
    ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED

  private fun openRequestedRoute(intent: Intent?) {
    val route = intent?.getStringExtra(EXTRA_ROUTE) ?: return
    intent.removeExtra(EXTRA_ROUTE)
    val quoted = JSONObject.quote(route)
    appWebView?.post { appWebView?.evaluateJavascript("window.location.assign($quoted)", null) }
  }

  private fun emitNativeEvent(payload: JSONObject) {
    val script = "window.dispatchEvent(new CustomEvent('voice-elf:android-native',{detail:${payload}}))"
    appWebView?.post { appWebView?.evaluateJavascript(script, null) }
  }

  private fun safeAreaPayload() = JSONObject()
    .put("top", safeInsetTop)
    .put("right", safeInsetRight)
    .put("bottom", safeInsetBottom)
    .put("left", safeInsetLeft)

  private fun applySafeAreaToWebView() {
    val payload = safeAreaPayload()
    val script = "window.dispatchEvent(new CustomEvent('voice-elf:android-safe-area',{detail:$payload}))"
    appWebView?.post { appWebView?.evaluateJavascript(script, null) }
  }

  private data class CaptureRequest(
    val microphone: Boolean,
    val systemAudio: Boolean,
    val noiseSuppression: Boolean,
    val echoCancellation: Boolean,
  )

  companion object {
    const val EXTRA_ROUTE = "voice_elf_route"
  }
}

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
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private var pendingCapture: CaptureRequest? = null
  private var pendingOverlayPayload: String? = null
  private var pendingDownload: DownloadRequest? = null
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

  private val downloadDestinationLauncher = registerForActivityResult(
    ActivityResultContracts.StartActivityForResult(),
  ) { result ->
    val request = pendingDownload
    pendingDownload = null
    val destination = result.data?.data
    if (request == null) return@registerForActivityResult
    if (result.resultCode != Activity.RESULT_OK || destination == null) {
      emitNativeEvent(JSONObject().put("type", "download-cancelled"))
      return@registerForActivityResult
    }
    saveDownload(request, destination)
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

    @JavascriptInterface
    fun downloadFile(url: String, fileName: String, mimeType: String) =
      runTrusted(webView, "download-error") {
        val resolvedUrl = webView.url?.let { pageUrl ->
          runCatching { URI(pageUrl).resolve(url).toString() }.getOrNull()
        }
        val resolvedUri = resolvedUrl?.let(Uri::parse)
        if (resolvedUrl == null ||
          (resolvedUri?.host != "127.0.0.1" && resolvedUri?.host != "localhost") ||
          (resolvedUri.scheme != "http" && resolvedUri.scheme != "https")
        ) {
          emitNativeEvent(JSONObject().put("type", "download-error").put("message", "下载地址无效"))
          return@runTrusted
        }
        val safeName = sanitizeFileName(fileName)
        val safeMimeType = when (mimeType) {
          "text/plain", "application/zip", "audio/wav" -> mimeType
          else -> "application/octet-stream"
        }
        pendingDownload = DownloadRequest(
          url = resolvedUrl,
          fileName = safeName,
          mimeType = safeMimeType,
          cookie = CookieManager.getInstance().getCookie(resolvedUrl).orEmpty(),
          userAgent = webView.settings.userAgentString.orEmpty(),
        )
        downloadDestinationLauncher.launch(
          Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = safeMimeType
            putExtra(Intent.EXTRA_TITLE, safeName)
          },
        )
      }
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

  private fun saveDownload(request: DownloadRequest, destination: Uri) {
    emitNativeEvent(
      JSONObject().put("type", "download-started").put("fileName", request.fileName),
    )
    Thread({
      var connection: HttpURLConnection? = null
      try {
        connection = URL(request.url).openConnection() as HttpURLConnection
        connection.connectTimeout = 15_000
        connection.readTimeout = 120_000
        connection.instanceFollowRedirects = false
        if (request.cookie.isNotBlank()) connection.setRequestProperty("Cookie", request.cookie)
        if (request.userAgent.isNotBlank()) connection.setRequestProperty("User-Agent", request.userAgent)
        val status = connection.responseCode
        if (status !in 200..299) throw IllegalStateException("下载失败（HTTP $status）")
        val output = contentResolver.openOutputStream(destination, "w")
          ?: throw IllegalStateException("无法写入所选文件")
        output.buffered().use { target ->
          connection.inputStream.buffered().use { source ->
            source.copyTo(target, DEFAULT_BUFFER_SIZE)
          }
        }
        emitNativeEvent(
          JSONObject().put("type", "download-completed").put("fileName", request.fileName),
        )
      } catch (error: Exception) {
        runCatching { contentResolver.delete(destination, null, null) }
        emitNativeEvent(
          JSONObject()
            .put("type", "download-error")
            .put("message", error.message ?: "无法保存会议记录"),
        )
      } finally {
        connection?.disconnect()
      }
    }, "voice-elf-room-download").start()
  }

  private fun sanitizeFileName(fileName: String): String {
    val sanitized = fileName
      .substringAfterLast('/')
      .replace(Regex("[\\\\/:*?\"<>|]"), "_")
      .trim()
      .take(160)
    return sanitized.ifBlank { "voice-elf-room-records.zip" }
  }

  private fun runTrusted(webView: WebView, errorType: String = "capture-error", block: () -> Unit) {
    runOnUiThread {
      val uri = webView.url?.let(Uri::parse)
      if (uri?.host == "127.0.0.1" || uri?.host == "localhost") block()
      else emitNativeEvent(JSONObject().put("type", errorType).put("message", "已阻止非本地页面调用原生能力"))
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

  private data class DownloadRequest(
    val url: String,
    val fileName: String,
    val mimeType: String,
    val cookie: String,
    val userAgent: String,
  )

  companion object {
    const val EXTRA_ROUTE = "voice_elf_route"
  }
}

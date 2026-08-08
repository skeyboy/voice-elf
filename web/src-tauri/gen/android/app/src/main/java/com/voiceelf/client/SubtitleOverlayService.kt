package com.voiceelf.client

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.TextView
import org.json.JSONObject
import kotlin.math.roundToInt

class SubtitleOverlayService : Service() {
  private lateinit var windowManager: WindowManager
  private var overlay: LinearLayout? = null
  private var toolbar: LinearLayout? = null
  private var sourceText: TextView? = null
  private var translationText: TextView? = null
  private var roomId = ""
  private var roomName = "实时字幕"
  private var scale = 1f
  private val handler = Handler(Looper.getMainLooper())
  private val hideToolbar = Runnable { toolbar?.animate()?.alpha(0f)?.setDuration(160)?.withEndAction {
    toolbar?.visibility = View.INVISIBLE
  }?.start() }

  override fun onCreate() {
    super.onCreate()
    windowManager = getSystemService(WindowManager::class.java)
    createNotificationChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_SHOW -> show(intent.getStringExtra(EXTRA_PAYLOAD).orEmpty())
      ACTION_UPDATE -> update(intent.getStringExtra(EXTRA_PAYLOAD).orEmpty())
      ACTION_HIDE -> closeOverlay(true)
    }
    return START_NOT_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onDestroy() {
    removeOverlay()
    super.onDestroy()
  }

  private fun show(payload: String) {
    if (!Settings.canDrawOverlays(this)) {
      emitError("悬浮窗权限未开启")
      stopSelf()
      return
    }
    val data = payloadObject(payload) ?: return
    roomId = data.optString("roomId")
    roomName = data.optString("roomName", "实时字幕")
    if (overlay == null) createOverlay()
    visible = true
    startForegroundCompat(buildNotification())
    applyPayload(data)
    showToolbarTemporarily()
    emit(JSONObject().put("type", "overlay-opened"))
  }

  private fun update(payload: String) {
    val root = overlay ?: return
    val data = payloadObject(payload) ?: return
    roomId = data.optString("roomId", roomId)
    roomName = data.optString("roomName", roomName)
    applyPayload(data)
    root.contentDescription = "$roomName 实时字幕"
  }

  private fun createOverlay() {
    val saved = getSharedPreferences(PREFS, MODE_PRIVATE)
    scale = saved.getFloat("scale", 1f).coerceIn(MIN_SCALE, MAX_SCALE)
    val root = LinearLayout(this).apply {
      orientation = LinearLayout.VERTICAL
      gravity = Gravity.CENTER_VERTICAL
      setPadding(dp(14), dp(8), dp(14), dp(14))
      elevation = dp(12).toFloat()
      background = roundedBackground(Color.argb(238, 25, 31, 28), dp(12).toFloat())
      isClickable = true
      setOnClickListener { showToolbarTemporarily() }
    }
    val controls = LinearLayout(this).apply {
      orientation = LinearLayout.HORIZONTAL
      gravity = Gravity.CENTER_VERTICAL
      minimumHeight = dp(48)
    }
    val handle = TextView(this).apply {
      text = roomName
      setTextColor(Color.rgb(204, 216, 209))
      textSize = 12f
      gravity = Gravity.CENTER_VERTICAL
      setPadding(dp(4), 0, dp(8), 0)
      contentDescription = "拖动字幕悬浮窗"
      setOnTouchListener(DragTouchListener())
    }
    controls.addView(handle, LinearLayout.LayoutParams(0, dp(48), 1f))
    controls.addView(textButton("−", "缩小字幕") { resize(-0.1f) })
    controls.addView(textButton("+", "放大字幕") { resize(0.1f) })
    controls.addView(iconButton(android.R.drawable.ic_menu_manage, "字幕设置") { openApp("/settings") })
    controls.addView(iconButton(android.R.drawable.ic_menu_view, "回到会议") { openApp("/rooms/$roomId") })
    controls.addView(iconButton(android.R.drawable.ic_menu_close_clear_cancel, "关闭字幕") { closeOverlay(true) })

    val source = TextView(this).apply {
      setTextColor(Color.WHITE)
      gravity = Gravity.START
      setLineSpacing(0f, 1.18f)
      includeFontPadding = false
      text = "等待发言"
    }
    val translation = TextView(this).apply {
      setTextColor(Color.rgb(130, 225, 177))
      gravity = Gravity.START
      setLineSpacing(0f, 1.18f)
      includeFontPadding = false
      setPadding(0, dp(6), 0, 0)
      text = "实时译文将在这里显示"
    }
    root.addView(controls, LinearLayout.LayoutParams.MATCH_PARENT, dp(48))
    root.addView(source, LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
    root.addView(translation, LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
    overlay = root
    toolbar = controls
    sourceText = source
    translationText = translation
    applyScale()

    val params = WindowManager.LayoutParams(
      saved.getInt("width", dp(360)),
      WindowManager.LayoutParams.WRAP_CONTENT,
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
      } else {
        @Suppress("DEPRECATION") WindowManager.LayoutParams.TYPE_PHONE
      },
      WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
        WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
        WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
      PixelFormat.TRANSLUCENT,
    ).apply {
      gravity = Gravity.TOP or Gravity.START
      x = saved.getInt("x", dp(16))
      y = saved.getInt("y", dp(96))
    }
    root.tag = params
    windowManager.addView(root, params)
  }

  private fun applyPayload(data: JSONObject) {
    val root = overlay ?: return
    val source = data.optString("source").ifBlank { "等待发言" }
    val translation = data.optString("translation").ifBlank { "实时译文将在这里显示" }
    sourceText?.text = source
    translationText?.text = translation
    sourceText?.visibility = if (data.optBoolean("sourceVisible", true)) View.VISIBLE else View.GONE
    translationText?.visibility = if (data.optBoolean("translationVisible", true)) View.VISIBLE else View.GONE
    parseColor(data.optString("backgroundColor"))?.let { color ->
      root.background = roundedBackground(colorWithMinimumAlpha(color), dp(12).toFloat())
    }
    parseColor(data.optString("sourceColor"))?.let { sourceText?.setTextColor(it) }
    parseColor(data.optString("translationColor"))?.let { translationText?.setTextColor(it) }
  }

  private fun resize(delta: Float) {
    scale = (scale + delta).coerceIn(MIN_SCALE, MAX_SCALE)
    applyScale()
    val root = overlay ?: return
    val params = root.tag as? WindowManager.LayoutParams ?: return
    val screenWidth = resources.displayMetrics.widthPixels
    params.width = (dp(360) * scale).roundToInt().coerceIn(dp(280), screenWidth - dp(24))
    windowManager.updateViewLayout(root, params)
    persist(params)
    showToolbarTemporarily()
  }

  private fun applyScale() {
    sourceText?.textSize = 22f * scale
    translationText?.textSize = 18f * scale
  }

  private fun showToolbarTemporarily() {
    handler.removeCallbacks(hideToolbar)
    toolbar?.apply {
      animate().cancel()
      visibility = View.VISIBLE
      alpha = 1f
    }
    handler.postDelayed(hideToolbar, TOOLBAR_TIMEOUT_MS)
  }

  private fun openApp(route: String) {
    startActivity(Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
      putExtra(MainActivity.EXTRA_ROUTE, route)
    })
    showToolbarTemporarily()
  }

  private fun closeOverlay(notifyWeb: Boolean) {
    removeOverlay()
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf()
    if (notifyWeb) emit(JSONObject().put("type", "overlay-closed"))
  }

  private fun removeOverlay() {
    handler.removeCallbacks(hideToolbar)
    overlay?.let { runCatching { windowManager.removeView(it) } }
    overlay = null
    toolbar = null
    sourceText = null
    translationText = null
    visible = false
  }

  private inner class DragTouchListener : View.OnTouchListener {
    private var downRawX = 0f
    private var downRawY = 0f
    private var startX = 0
    private var startY = 0
    private var moved = false

    override fun onTouch(view: View, event: MotionEvent): Boolean {
      val root = overlay ?: return false
      val params = root.tag as? WindowManager.LayoutParams ?: return false
      when (event.actionMasked) {
        MotionEvent.ACTION_DOWN -> {
          downRawX = event.rawX
          downRawY = event.rawY
          startX = params.x
          startY = params.y
          moved = false
          showToolbarTemporarily()
          view.isPressed = true
          return true
        }
        MotionEvent.ACTION_MOVE -> {
          moved = moved || kotlin.math.abs(event.rawX - downRawX) > dp(8) ||
            kotlin.math.abs(event.rawY - downRawY) > dp(8)
          params.x = startX + (event.rawX - downRawX).roundToInt()
          params.y = startY + (event.rawY - downRawY).roundToInt()
          clampPosition(params, root)
          windowManager.updateViewLayout(root, params)
          return true
        }
        MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
          view.isPressed = false
          clampPosition(params, root)
          windowManager.updateViewLayout(root, params)
          persist(params)
          showToolbarTemporarily()
          if (!moved && event.actionMasked == MotionEvent.ACTION_UP) view.performClick()
          return true
        }
      }
      return false
    }
  }

  private fun clampPosition(params: WindowManager.LayoutParams, root: View) {
    val metrics = resources.displayMetrics
    val width = if (root.width > 0) root.width else params.width
    val height = if (root.height > 0) root.height else dp(180)
    params.x = params.x.coerceIn(0, maxOf(0, metrics.widthPixels - width))
    params.y = params.y.coerceIn(0, maxOf(0, metrics.heightPixels - height))
  }

  private fun persist(params: WindowManager.LayoutParams) {
    getSharedPreferences(PREFS, MODE_PRIVATE).edit()
      .putInt("x", params.x)
      .putInt("y", params.y)
      .putInt("width", params.width)
      .putFloat("scale", scale)
      .apply()
  }

  private fun textButton(label: String, description: String, action: () -> Unit) =
    TextView(this).apply {
      text = label
      textSize = 22f
      setTextColor(Color.WHITE)
      gravity = Gravity.CENTER
      contentDescription = description
      isClickable = true
      setBackgroundColor(Color.TRANSPARENT)
      setOnClickListener { action() }
    }.also { it.layoutParams = LinearLayout.LayoutParams(dp(48), dp(48)) }

  private fun iconButton(icon: Int, description: String, action: () -> Unit) =
    ImageButton(this).apply {
      setImageResource(icon)
      setColorFilter(Color.WHITE)
      setBackgroundColor(Color.TRANSPARENT)
      contentDescription = description
      setPadding(dp(13), dp(13), dp(13), dp(13))
      setOnClickListener { action() }
      layoutParams = LinearLayout.LayoutParams(dp(48), dp(48))
    }

  private fun startForegroundCompat(notification: Notification) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
  }

  private fun buildNotification(): Notification {
    val openIntent = Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
      putExtra(MainActivity.EXTRA_ROUTE, "/rooms/$roomId")
    }
    val closeIntent = Intent(this, SubtitleOverlayService::class.java).apply { action = ACTION_HIDE }
    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, CHANNEL_ID)
    } else {
      @Suppress("DEPRECATION") Notification.Builder(this)
    }
    return builder
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentTitle("字幕大屏正在显示")
      .setContentText(roomName)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setCategory(Notification.CATEGORY_SERVICE)
      .setContentIntent(
        PendingIntent.getActivity(this, 20, openIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE),
      )
      .addAction(
        Notification.Action.Builder(
          android.R.drawable.ic_menu_close_clear_cancel,
          "关闭",
          PendingIntent.getService(this, 21, closeIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE),
        ).build(),
      )
      .build()
  }

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    getSystemService(NotificationManager::class.java).createNotificationChannel(
      NotificationChannel(CHANNEL_ID, "字幕悬浮窗", NotificationManager.IMPORTANCE_LOW).apply {
        description = "字幕大屏在其他应用上方显示时使用"
        setShowBadge(false)
      },
    )
  }

  private fun payloadObject(payload: String): JSONObject? = try {
    JSONObject(payload)
  } catch (_: Exception) {
    emitError("字幕悬浮窗收到无效数据")
    null
  }

  private fun roundedBackground(color: Int, radius: Float) = GradientDrawable().apply {
    setColor(color)
    cornerRadius = radius
    setStroke(dp(1), Color.argb(80, 255, 255, 255))
  }

  private fun parseColor(value: String): Int? = runCatching { Color.parseColor(value) }.getOrNull()

  private fun colorWithMinimumAlpha(color: Int) = Color.argb(maxOf(220, Color.alpha(color)), Color.red(color), Color.green(color), Color.blue(color))

  private fun dp(value: Int) = (value * resources.displayMetrics.density).roundToInt()

  private fun emitError(message: String) {
    emit(JSONObject().put("type", "overlay-error").put("message", message))
  }

  private fun emit(payload: JSONObject) {
    nativeEventSink?.invoke(payload)
  }

  companion object {
    const val ACTION_SHOW = "com.voiceelf.client.overlay.SHOW"
    const val ACTION_UPDATE = "com.voiceelf.client.overlay.UPDATE"
    const val ACTION_HIDE = "com.voiceelf.client.overlay.HIDE"
    const val EXTRA_PAYLOAD = "subtitle_payload"
    private const val CHANNEL_ID = "subtitle_overlay"
    private const val NOTIFICATION_ID = 1202
    private const val PREFS = "subtitle_overlay_state"
    private const val TOOLBAR_TIMEOUT_MS = 3_200L
    private const val MIN_SCALE = 0.8f
    private const val MAX_SCALE = 1.5f
    @Volatile var nativeEventSink: ((JSONObject) -> Unit)? = null
    @Volatile var visible = false
  }
}

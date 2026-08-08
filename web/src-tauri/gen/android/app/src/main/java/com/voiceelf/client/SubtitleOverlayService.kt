package com.voiceelf.client

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.Rect
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import android.util.TypedValue
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.widget.TextViewCompat
import org.json.JSONObject
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlin.math.sqrt

class SubtitleOverlayService : Service() {
  private lateinit var windowManager: WindowManager
  private var overlay: FrameLayout? = null
  private var toolbar: LinearLayout? = null
  private val resizeHandles = mutableListOf<View>()
  private var sourceText: TextView? = null
  private var translationText: TextView? = null
  private var roomId = ""
  private var roomName = "实时字幕"
  private val handler = Handler(Looper.getMainLooper())
  private val hideToolbar = Runnable { toolbar?.animate()?.alpha(0f)?.setDuration(160)?.withEndAction {
    toolbar?.visibility = View.INVISIBLE
    resizeHandles.forEach { it.visibility = View.INVISIBLE }
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

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    val root = overlay ?: return
    val params = root.tag as? WindowManager.LayoutParams ?: return
    val bounds = safeBounds()
    params.width = params.width.coerceIn(minOf(dp(MIN_WIDTH_DP), bounds.width()), bounds.width())
    params.height = params.height.coerceIn(minOf(dp(MIN_HEIGHT_DP), bounds.height()), bounds.height())
    clampPosition(params)
    windowManager.updateViewLayout(root, params)
    persist(params)
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
    val dragListener = DragTouchListener()
    val root = FrameLayout(this).apply {
      elevation = dp(12).toFloat()
      background = roundedBackground(Color.argb(238, 25, 31, 28), dp(12).toFloat())
      isClickable = true
      setOnTouchListener(dragListener)
    }
    val content = LinearLayout(this).apply {
      orientation = LinearLayout.VERTICAL
      setPadding(dp(14), dp(8), dp(14), dp(14))
      isClickable = true
      setOnTouchListener(dragListener)
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
      setOnTouchListener(dragListener)
    }
    controls.addView(handle, LinearLayout.LayoutParams(0, dp(48), 1f))
    controls.addView(iconButton(android.R.drawable.ic_menu_manage, "字幕设置") { openApp("/settings") }.apply {
      setOnTouchListener(dragListener)
    })
    controls.addView(iconButton(android.R.drawable.ic_menu_view, "回到会议") { openApp("/rooms/$roomId") }.apply {
      setOnTouchListener(dragListener)
    })
    controls.addView(iconButton(android.R.drawable.ic_menu_close_clear_cancel, "关闭字幕") { closeOverlay(true) }.apply {
      setOnTouchListener(dragListener)
    })

    val source = captionTextView(22f, 12, 30).apply {
      setTextColor(Color.WHITE)
      text = "等待发言"
      setOnTouchListener(dragListener)
    }
    val translation = captionTextView(18f, 11, 26).apply {
      setTextColor(Color.rgb(130, 225, 177))
      setPadding(0, dp(6), 0, 0)
      text = "实时译文将在这里显示"
      setOnTouchListener(dragListener)
    }
    val captions = LinearLayout(this).apply {
      orientation = LinearLayout.VERTICAL
      gravity = Gravity.CENTER_VERTICAL
      setOnTouchListener(dragListener)
      addView(source, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
      addView(translation, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
    }
    content.addView(controls, LinearLayout.LayoutParams.MATCH_PARENT, dp(48))
    content.addView(captions, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
    root.addView(content, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))

    listOf(
      Triple(Gravity.TOP or Gravity.START, -1, -1),
      Triple(Gravity.TOP or Gravity.END, 1, -1),
      Triple(Gravity.BOTTOM or Gravity.START, -1, 1),
      Triple(Gravity.BOTTOM or Gravity.END, 1, 1),
    ).forEach { (gravity, horizontal, vertical) ->
      val resizeHandle = resizeHandle(horizontal, vertical)
      root.addView(resizeHandle, FrameLayout.LayoutParams(dp(30), dp(30), gravity))
      resizeHandles.add(resizeHandle)
    }
    overlay = root
    toolbar = controls
    sourceText = source
    translationText = translation
    val bounds = safeBounds()
    val dimensionRatio = sqrt(DEFAULT_AREA_RATIO)
    val defaultWidth = (bounds.width() * dimensionRatio).roundToInt()
    val defaultHeight = (bounds.height() * dimensionRatio).roundToInt()
    val hasCurrentLayout = saved.getInt("layout_version", 0) == CURRENT_LAYOUT_VERSION
    val width = (if (hasCurrentLayout) saved.getInt("width", defaultWidth) else defaultWidth)
      .coerceIn(minOf(dp(MIN_WIDTH_DP), bounds.width()), bounds.width())
    val height = (if (hasCurrentLayout) saved.getInt("height", defaultHeight) else defaultHeight)
      .coerceIn(minOf(dp(MIN_HEIGHT_DP), bounds.height()), bounds.height())
    val params = WindowManager.LayoutParams(
      width,
      height,
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
      x = lastX ?: bounds.left + (bounds.width() - width) / 2
      y = lastY ?: bounds.top + (bounds.height() - height) / 2
    }
    root.tag = params
    clampPosition(params)
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

  private fun showToolbarTemporarily() {
    handler.removeCallbacks(hideToolbar)
    toolbar?.apply {
      animate().cancel()
      visibility = View.VISIBLE
      alpha = 1f
    }
    resizeHandles.forEach {
      it.visibility = View.VISIBLE
      it.alpha = 1f
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
    resizeHandles.clear()
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
          moved = moved || abs(event.rawX - downRawX) > dp(8) ||
            abs(event.rawY - downRawY) > dp(8)
          params.x = startX + (event.rawX - downRawX).roundToInt()
          params.y = startY + (event.rawY - downRawY).roundToInt()
          clampPosition(params)
          windowManager.updateViewLayout(root, params)
          return true
        }
        MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
          view.isPressed = false
          clampPosition(params)
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

  private inner class ResizeTouchListener(
    private val horizontal: Int,
    private val vertical: Int,
  ) : View.OnTouchListener {
    private var downRawX = 0f
    private var downRawY = 0f
    private var startX = 0
    private var startY = 0
    private var startWidth = 0
    private var startHeight = 0

    override fun onTouch(view: View, event: MotionEvent): Boolean {
      val root = overlay ?: return false
      val params = root.tag as? WindowManager.LayoutParams ?: return false
      when (event.actionMasked) {
        MotionEvent.ACTION_DOWN -> {
          downRawX = event.rawX
          downRawY = event.rawY
          startX = params.x
          startY = params.y
          startWidth = params.width
          startHeight = params.height
          view.isPressed = true
          showToolbarTemporarily()
          return true
        }
        MotionEvent.ACTION_MOVE -> {
          val bounds = safeBounds()
          val deltaX = (event.rawX - downRawX).roundToInt()
          val deltaY = (event.rawY - downRawY).roundToInt()
          val maxWidth = if (horizontal < 0) startX + startWidth - bounds.left else bounds.right - startX
          val maxHeight = if (vertical < 0) startY + startHeight - bounds.top else bounds.bottom - startY
          val width = (startWidth + deltaX * horizontal)
            .coerceIn(minOf(dp(MIN_WIDTH_DP), maxWidth), maxWidth)
          val height = (startHeight + deltaY * vertical)
            .coerceIn(minOf(dp(MIN_HEIGHT_DP), maxHeight), maxHeight)
          params.width = width
          params.height = height
          if (horizontal < 0) params.x = startX + startWidth - width
          if (vertical < 0) params.y = startY + startHeight - height
          windowManager.updateViewLayout(root, params)
          return true
        }
        MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
          view.isPressed = false
          clampPosition(params)
          windowManager.updateViewLayout(root, params)
          persist(params)
          showToolbarTemporarily()
          return true
        }
      }
      return false
    }
  }

  private fun clampPosition(params: WindowManager.LayoutParams) {
    val bounds = safeBounds()
    val width = params.width
    val height = params.height
    params.x = params.x.coerceIn(bounds.left, maxOf(bounds.left, bounds.right - width))
    params.y = params.y.coerceIn(bounds.top, maxOf(bounds.top, bounds.bottom - height))
  }

  private fun persist(params: WindowManager.LayoutParams) {
    lastX = params.x
    lastY = params.y
    getSharedPreferences(PREFS, MODE_PRIVATE).edit()
      .remove("x")
      .remove("y")
      .putInt("width", params.width)
      .putInt("height", params.height)
      .putInt("layout_version", CURRENT_LAYOUT_VERSION)
      .apply()
  }

  private fun safeBounds(): Rect {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
      val metrics = windowManager.currentWindowMetrics
      val insets = metrics.windowInsets.getInsetsIgnoringVisibility(
        WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout(),
      )
      return Rect(
        metrics.bounds.left + insets.left,
        metrics.bounds.top + insets.top,
        metrics.bounds.right - insets.right,
        metrics.bounds.bottom - insets.bottom,
      )
    }
    val metrics = resources.displayMetrics
    val statusBarHeight = resources.getIdentifier("status_bar_height", "dimen", "android")
      .takeIf { it > 0 }
      ?.let(resources::getDimensionPixelSize)
      ?: 0
    val navigationBarHeight = resources.getIdentifier("navigation_bar_height", "dimen", "android")
      .takeIf { it > 0 }
      ?.let(resources::getDimensionPixelSize)
      ?: 0
    return Rect(0, statusBarHeight, metrics.widthPixels, metrics.heightPixels - navigationBarHeight)
  }

  private fun captionTextView(preferredSize: Float, minimumSize: Int, maximumSize: Int) =
    TextView(this).apply {
      textSize = preferredSize
      gravity = Gravity.START or Gravity.CENTER_VERTICAL
      setLineSpacing(0f, 1.16f)
      includeFontPadding = false
      maxLines = 12
      TextViewCompat.setAutoSizeTextTypeUniformWithConfiguration(
        this,
        minimumSize,
        maximumSize,
        1,
        TypedValue.COMPLEX_UNIT_SP,
      )
    }

  private fun resizeHandle(horizontal: Int, vertical: Int) = View(this).apply {
    contentDescription = "拖动缩放字幕悬浮窗"
    isClickable = true
    background = roundedBackground(Color.argb(175, 92, 111, 102), dp(6).toFloat())
    setOnTouchListener(ResizeTouchListener(horizontal, vertical))
  }

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
    private const val DEFAULT_AREA_RATIO = 0.618f
    private const val CURRENT_LAYOUT_VERSION = 2
    private const val MIN_WIDTH_DP = 240
    private const val MIN_HEIGHT_DP = 160
    @Volatile var nativeEventSink: ((JSONObject) -> Unit)? = null
    @Volatile var visible = false
    @Volatile private var lastX: Int? = null
    @Volatile private var lastY: Int? = null

    fun clearSessionPosition(context: Context) {
      lastX = null
      lastY = null
      context.getSharedPreferences(PREFS, MODE_PRIVATE).edit()
        .remove("x")
        .remove("y")
        .apply()
    }
  }
}

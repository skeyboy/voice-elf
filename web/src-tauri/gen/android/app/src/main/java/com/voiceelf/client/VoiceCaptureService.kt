package com.voiceelf.client

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Base64
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicBoolean

class VoiceCaptureService : Service() {
  private var microphone = false
  private var systemAudio = false
  private var projectionResultCode: Int? = null
  private var projectionResultData: Intent? = null
  private var mediaProjection: MediaProjection? = null
  private var audioRecord: AudioRecord? = null
  private var captureThread: Thread? = null
  private val capturing = AtomicBoolean(false)

  override fun onCreate() {
    super.onCreate()
    createNotificationChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_START -> startCaptureSession(intent)
      ACTION_CAPTURE_READY -> startSystemAudioIfReady()
      ACTION_STOP -> stopSelfSafely(true)
    }
    return START_NOT_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onDestroy() {
    stopSystemAudio()
    super.onDestroy()
  }

  private fun startCaptureSession(intent: Intent) {
    microphone = intent.getBooleanExtra(EXTRA_MICROPHONE, false)
    systemAudio = intent.getBooleanExtra(EXTRA_SYSTEM_AUDIO, false)
    projectionResultCode = if (intent.hasExtra(EXTRA_RESULT_CODE)) {
      intent.getIntExtra(EXTRA_RESULT_CODE, 0)
    } else null
    projectionResultData = getProjectionIntent(intent)

    if (systemAudio && Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
      emitError("系统内录需要 Android 10 或更高版本")
      stopSelfSafely(false)
      return
    }
    startForegroundCompat(buildNotification())
    emit(JSONObject().put("type", "capture-started").put("systemAudio", systemAudio))
  }

  private fun startForegroundCompat(notification: Notification) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      var type = 0
      if (microphone && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
      }
      if (systemAudio) type = type or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
      startForeground(NOTIFICATION_ID, notification, type)
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
  }

  private fun startSystemAudioIfReady() {
    if (!systemAudio || capturing.get()) return
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
      emitError("系统内录需要 Android 10 或更高版本")
      stopSelfSafely(false)
      return
    }
    startSystemAudioApi29()
  }

  @RequiresApi(Build.VERSION_CODES.Q)
  private fun startSystemAudioApi29() {
    val resultCode = projectionResultCode
    val resultData = projectionResultData
    if (resultCode == null || resultData == null) {
      emitError("系统内录授权已失效，请重新开始录音")
      stopSelfSafely(false)
      return
    }
    try {
      if (ContextCompat.checkSelfPermission(this, android.Manifest.permission.RECORD_AUDIO) !=
        PackageManager.PERMISSION_GRANTED
      ) {
        throw SecurityException("麦克风权限已被撤销，请重新授权")
      }
      val manager = getSystemService(MediaProjectionManager::class.java)
      val projection = manager.getMediaProjection(resultCode, resultData)
        ?: throw IllegalStateException("系统未返回可用的内录授权")
      projection.registerCallback(object : MediaProjection.Callback() {
        override fun onStop() {
          emitError("系统已停止内录共享")
          stopSelfSafely(false)
        }
      }, Handler(Looper.getMainLooper()))
      mediaProjection = projection

      val captureConfig = AudioPlaybackCaptureConfiguration.Builder(projection)
        .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
        .addMatchingUsage(AudioAttributes.USAGE_GAME)
        .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
        .build()
      val format = AudioFormat.Builder()
        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
        .setSampleRate(SAMPLE_RATE)
        .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
        .build()
      val minBuffer = AudioRecord.getMinBufferSize(
        SAMPLE_RATE,
        AudioFormat.CHANNEL_IN_MONO,
        AudioFormat.ENCODING_PCM_16BIT,
      )
      val record = AudioRecord.Builder()
        .setAudioFormat(format)
        .setBufferSizeInBytes(maxOf(minBuffer, PCM_CHUNK_BYTES * 4))
        .setAudioPlaybackCaptureConfig(captureConfig)
        .build()
      audioRecord = record
      capturing.set(true)
      record.startRecording()
      captureThread = Thread({ captureLoop(record) }, "voice-elf-system-audio").apply { start() }
    } catch (error: Exception) {
      emitError(error.message ?: "无法启动系统内录")
      stopSelfSafely(false)
    }
  }

  private fun captureLoop(record: AudioRecord) {
    val buffer = ByteArray(PCM_CHUNK_BYTES)
    while (capturing.get()) {
      val count = record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
      if (count > 0) {
        val encoded = Base64.encodeToString(buffer, 0, count, Base64.NO_WRAP)
        emit(JSONObject().put("type", "audio-pcm").put("data", encoded))
      } else if (count < 0) {
        emitError("系统内录读取失败（$count）")
        break
      }
    }
  }

  private fun stopSystemAudio() {
    capturing.set(false)
    try {
      audioRecord?.stop()
    } catch (_: IllegalStateException) {
    }
    audioRecord?.release()
    audioRecord = null
    captureThread?.interrupt()
    captureThread = null
    mediaProjection?.stop()
    mediaProjection = null
  }

  private fun stopSelfSafely(notifyWeb: Boolean) {
    stopSystemAudio()
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf()
    if (notifyWeb) emit(JSONObject().put("type", "capture-stopped"))
  }

  private fun buildNotification(): Notification {
    val openIntent = Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
    }
    val stopIntent = Intent(this, VoiceCaptureService::class.java).apply { action = ACTION_STOP }
    val mode = when {
      microphone && systemAudio -> "麦克风与系统内录正在运行"
      systemAudio -> "系统内录正在运行"
      else -> "麦克风录音正在运行"
    }
    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, CHANNEL_ID)
    } else {
      @Suppress("DEPRECATION") Notification.Builder(this)
    }
    return builder
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentTitle("Voice Elf 正在转译")
      .setContentText(mode)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setCategory(Notification.CATEGORY_SERVICE)
      .setContentIntent(
        PendingIntent.getActivity(this, 10, openIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE),
      )
      .addAction(
        Notification.Action.Builder(
          android.R.drawable.ic_media_pause,
          "停止",
          PendingIntent.getService(this, 11, stopIntent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE),
        ).build(),
      )
      .build()
  }

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(NotificationManager::class.java)
    manager.createNotificationChannel(
      NotificationChannel(CHANNEL_ID, "录音与实时转译", NotificationManager.IMPORTANCE_LOW).apply {
        description = "录音在后台持续运行时显示"
        setShowBadge(false)
      },
    )
  }

  @Suppress("DEPRECATION")
  private fun getProjectionIntent(intent: Intent): Intent? =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      intent.getParcelableExtra(EXTRA_RESULT_DATA, Intent::class.java)
    } else {
      intent.getParcelableExtra(EXTRA_RESULT_DATA)
    }

  private fun emitError(message: String) {
    emit(JSONObject().put("type", "capture-error").put("message", message))
  }

  private fun emit(payload: JSONObject) {
    nativeEventSink?.invoke(payload)
  }

  companion object {
    const val ACTION_START = "com.voiceelf.client.capture.START"
    const val ACTION_CAPTURE_READY = "com.voiceelf.client.capture.READY"
    const val ACTION_STOP = "com.voiceelf.client.capture.STOP"
    const val EXTRA_MICROPHONE = "microphone"
    const val EXTRA_SYSTEM_AUDIO = "system_audio"
    const val EXTRA_RESULT_CODE = "projection_result_code"
    const val EXTRA_RESULT_DATA = "projection_result_data"
    private const val CHANNEL_ID = "voice_capture"
    private const val NOTIFICATION_ID = 1201
    private const val SAMPLE_RATE = 48_000
    private const val PCM_CHUNK_BYTES = 4096
    @Volatile var nativeEventSink: ((JSONObject) -> Unit)? = null
  }
}

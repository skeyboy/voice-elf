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
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.NoiseSuppressor
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Process
import android.util.Base64
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt

class VoiceCaptureService : Service() {
  private var microphone = false
  private var systemAudio = false
  private var noiseSuppression = true
  private var echoCancellation = true
  private var projectionResultCode: Int? = null
  private var projectionResultData: Intent? = null
  private var mediaProjection: MediaProjection? = null
  private var playbackRecord: AudioRecord? = null
  private var microphoneRecord: AudioRecord? = null
  private var noiseSuppressor: NoiseSuppressor? = null
  private var echoCanceler: AcousticEchoCanceler? = null
  private var captureThread: Thread? = null
  private var microphoneThread: Thread? = null
  private var microphoneQueue = ArrayBlockingQueue<ShortArray>(MICROPHONE_QUEUE_CAPACITY)
  private val capturing = AtomicBoolean(false)
  private val stoppingProjection = AtomicBoolean(false)

  override fun onCreate() {
    super.onCreate()
    createNotificationChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_START -> startCaptureSession(intent)
      ACTION_CAPTURE_READY -> startSystemAudioIfReady()
      ACTION_UPDATE -> updateCaptureSession(intent)
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
    noiseSuppression = intent.getBooleanExtra(EXTRA_NOISE_SUPPRESSION, true)
    echoCancellation = intent.getBooleanExtra(EXTRA_ECHO_CANCELLATION, true)
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

  private fun updateCaptureSession(intent: Intent) {
    val requestedSystemAudio = intent.getBooleanExtra(EXTRA_SYSTEM_AUDIO, systemAudio)
    if (!systemAudio || !requestedSystemAudio || mediaProjection == null) {
      emitError("内录模式已变化，请重新授予录音权限")
      return
    }
    microphone = intent.getBooleanExtra(EXTRA_MICROPHONE, microphone)
    noiseSuppression = intent.getBooleanExtra(EXTRA_NOISE_SUPPRESSION, noiseSuppression)
    echoCancellation = intent.getBooleanExtra(EXTRA_ECHO_CANCELLATION, echoCancellation)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      try {
        restartAudioRecordsApi29()
        startForegroundCompat(buildNotification())
      } catch (error: Exception) {
        emitError(error.message ?: "无法更新录音设置")
        stopSelfSafely(false)
      }
    }
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
      stoppingProjection.set(false)
      projection.registerCallback(object : MediaProjection.Callback() {
        override fun onStop() {
          if (stoppingProjection.get()) return
          emitError("系统已停止内录共享")
          stopSelfSafely(false)
        }
      }, Handler(Looper.getMainLooper()))
      mediaProjection = projection
      startAudioRecordsApi29(projection)
    } catch (error: Exception) {
      emitError(error.message ?: "无法启动系统内录")
      stopSelfSafely(false)
    }
  }

  @RequiresApi(Build.VERSION_CODES.Q)
  private fun restartAudioRecordsApi29() {
    val projection = mediaProjection ?: throw IllegalStateException("系统内录授权已失效")
    stopAudioRecords()
    startAudioRecordsApi29(projection)
  }

  @RequiresApi(Build.VERSION_CODES.Q)
  private fun startAudioRecordsApi29(projection: MediaProjection) {
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
    if (minBuffer <= 0) throw IllegalStateException("设备不支持 48 kHz PCM 内录")
    val playback = AudioRecord.Builder()
        .setAudioFormat(format)
        .setBufferSizeInBytes(maxOf(minBuffer * 2, PCM_CHUNK_BYTES * 8))
        .setAudioPlaybackCaptureConfig(captureConfig)
        .build()
    if (playback.state != AudioRecord.STATE_INITIALIZED) {
      playback.release()
      throw IllegalStateException("系统内录设备初始化失败")
    }
    val mic = if (microphone) createMicrophoneRecord(format, minBuffer) else null
    playbackRecord = playback
    microphoneRecord = mic
    microphoneQueue = ArrayBlockingQueue(MICROPHONE_QUEUE_CAPACITY)
    attachMicrophoneEffects(mic)
    capturing.set(true)
    mic?.startRecording()
    playback.startRecording()
    if ((mic != null && mic.recordingState != AudioRecord.RECORDSTATE_RECORDING) ||
      playback.recordingState != AudioRecord.RECORDSTATE_RECORDING
    ) {
      stopAudioRecords()
      throw IllegalStateException("音频设备未进入录音状态")
    }
    microphoneThread = mic?.let { record ->
      Thread({ microphoneLoop(record) }, "voice-elf-microphone").apply { start() }
    }
    captureThread = Thread({ captureLoop(playback) }, "voice-elf-system-audio").apply { start() }
  }

  private fun createMicrophoneRecord(format: AudioFormat, minBuffer: Int): AudioRecord {
    val source = if (noiseSuppression || echoCancellation) {
      MediaRecorder.AudioSource.VOICE_COMMUNICATION
    } else {
      MediaRecorder.AudioSource.MIC
    }
    return AudioRecord.Builder()
      .setAudioSource(source)
      .setAudioFormat(format)
      .setBufferSizeInBytes(maxOf(minBuffer * 2, PCM_CHUNK_BYTES * 8))
      .build()
      .also {
        if (it.state != AudioRecord.STATE_INITIALIZED) {
          it.release()
          throw IllegalStateException("麦克风设备初始化失败")
        }
      }
  }

  private fun attachMicrophoneEffects(record: AudioRecord?) {
    if (record == null) return
    if (NoiseSuppressor.isAvailable()) {
      noiseSuppressor = NoiseSuppressor.create(record.audioSessionId)?.apply {
        enabled = noiseSuppression
      }
    }
    if (AcousticEchoCanceler.isAvailable()) {
      echoCanceler = AcousticEchoCanceler.create(record.audioSessionId)?.apply {
        enabled = echoCancellation
      }
    }
  }

  private fun microphoneLoop(record: AudioRecord) {
    Process.setThreadPriority(Process.THREAD_PRIORITY_AUDIO)
    val buffer = ShortArray(PCM_CHUNK_SAMPLES)
    while (capturing.get()) {
      val count = record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
      if (count > 0) {
        val chunk = buffer.copyOf(count)
        if (!microphoneQueue.offer(chunk)) {
          microphoneQueue.poll()
          microphoneQueue.offer(chunk)
        }
      } else if (count < 0 && capturing.get()) {
        failCapture("麦克风读取失败（$count）")
        break
      }
    }
  }

  private fun captureLoop(record: AudioRecord) {
    Process.setThreadPriority(Process.THREAD_PRIORITY_AUDIO)
    val buffer = ShortArray(PCM_CHUNK_SAMPLES)
    while (capturing.get()) {
      val count = record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
      if (count > 0) {
        val microphoneSamples = if (microphone) {
          microphoneQueue.poll(MICROPHONE_WAIT_MS, TimeUnit.MILLISECONDS)
        } else null
        val pcm = encodePcm16(buffer, count, microphoneSamples)
        val encoded = Base64.encodeToString(pcm, Base64.NO_WRAP)
        emit(
          JSONObject()
            .put("type", "audio-pcm")
            .put("data", encoded)
            .put("sampleRate", SAMPLE_RATE),
        )
      } else if (count < 0 && capturing.get()) {
        failCapture("系统内录读取失败（$count）")
        break
      }
    }
  }

  private fun encodePcm16(system: ShortArray, count: Int, mic: ShortArray?): ByteArray {
    val output = ByteArray(count * 2)
    for (index in 0 until count) {
      val sample = if (mic != null && index < mic.size) {
        (system[index] * SYSTEM_MIX_GAIN + mic[index] * MICROPHONE_MIX_GAIN)
          .roundToInt()
          .coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
      } else {
        system[index].toInt()
      }
      output[index * 2] = (sample and 0xff).toByte()
      output[index * 2 + 1] = ((sample ushr 8) and 0xff).toByte()
    }
    return output
  }

  private fun failCapture(message: String) {
    Handler(Looper.getMainLooper()).post {
      if (!capturing.get()) return@post
      emitError(message)
      stopSelfSafely(false)
    }
  }

  private fun stopAudioRecords() {
    capturing.set(false)
    listOf(playbackRecord, microphoneRecord).forEach { record ->
      try {
        record?.stop()
      } catch (_: IllegalStateException) {
      }
    }
    captureThread?.interrupt()
    microphoneThread?.interrupt()
    if (Thread.currentThread() !== captureThread) captureThread?.join(500)
    if (Thread.currentThread() !== microphoneThread) microphoneThread?.join(500)
    captureThread = null
    microphoneThread = null
    noiseSuppressor?.release()
    echoCanceler?.release()
    noiseSuppressor = null
    echoCanceler = null
    playbackRecord?.release()
    microphoneRecord?.release()
    playbackRecord = null
    microphoneRecord = null
    microphoneQueue.clear()
  }

  private fun stopSystemAudio() {
    stopAudioRecords()
    stoppingProjection.set(true)
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
    const val ACTION_UPDATE = "com.voiceelf.client.capture.UPDATE"
    const val ACTION_STOP = "com.voiceelf.client.capture.STOP"
    const val EXTRA_MICROPHONE = "microphone"
    const val EXTRA_SYSTEM_AUDIO = "system_audio"
    const val EXTRA_NOISE_SUPPRESSION = "noise_suppression"
    const val EXTRA_ECHO_CANCELLATION = "echo_cancellation"
    const val EXTRA_RESULT_CODE = "projection_result_code"
    const val EXTRA_RESULT_DATA = "projection_result_data"
    private const val CHANNEL_ID = "voice_capture"
    private const val NOTIFICATION_ID = 1201
    private const val SAMPLE_RATE = 48_000
    private const val PCM_CHUNK_BYTES = 8192
    private const val PCM_CHUNK_SAMPLES = PCM_CHUNK_BYTES / 2
    private const val MICROPHONE_QUEUE_CAPACITY = 6
    private const val MICROPHONE_WAIT_MS = 12L
    private const val SYSTEM_MIX_GAIN = 0.5f
    private const val MICROPHONE_MIX_GAIN = 0.5f
    @Volatile var nativeEventSink: ((JSONObject) -> Unit)? = null
  }
}

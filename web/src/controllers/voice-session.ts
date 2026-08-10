import {
  AudioCapture,
  PcmPlayer,
  Waveform,
  type AudioCaptureOptions,
  type VadBoundary,
} from '../audio';
import type { ServerEvent, SessionConfig } from '../protocol';
import type { ConnectionStatus } from '../components/topbar';
import {
  connectGrpcRealtime,
  connectWebSocket,
  type RealtimeTransport,
} from '../realtime-transport';

const MIN_VALID_SPEECH_FRAMES = 3;
const PLAYBACK_TAIL_GUARD_MS = 300;

interface VoiceSessionCallbacks {
  onEvent: (event: ServerEvent) => void;
  onConnection: (status: ConnectionStatus) => void;
  onRecording: (recording: boolean) => void;
  onCaptureError: (message: string) => void;
}

export class VoiceSession {
  private readonly audioCapture = new AudioCapture();
  private readonly waveform: Waveform;
  private transport: RealtimeTransport | null = null;
  private socketVersion = 0;
  private reconnectTimer = 0;
  private receivingAudio: {
    utteranceId: string;
    channels: number;
  } | null = null;
  private audioSampleRate = 24_000;
  private recording = false;
  private activeTcId: string | null = null;
  private activeSampleCount = 0;
  private segmentsStarted = 0;
  private activeEnhancedVoiceFilter = false;
  private readonly playbackHolds = new Set<'stream' | 'media'>();
  private readonly playbackReleaseTimers = new Map<'stream' | 'media', number>();
  private captureOptionsRevision = 0;
  private captureReconfigurePromise: Promise<void> | null = null;
  private destroyed = false;

  constructor(
    private readonly roomId: string,
    private canPublish: boolean,
    canvas: HTMLCanvasElement,
    private readonly player: PcmPlayer,
    private readonly config: () => SessionConfig,
    private readonly enhancedVoiceFilter: () => boolean,
    private readonly captureOptions: () => AudioCaptureOptions,
    private readonly callbacks: VoiceSessionCallbacks,
  ) {
    this.waveform = new Waveform(canvas);
    this.player.setPlaybackListener((active) => this.setPlaybackHold('stream', active));
  }

  connect() {
    this.disconnect();
    const version = this.socketVersion;
    this.destroyed = false;
    this.callbacks.onConnection('connecting');
    let opened = false;
    let usingFallback = false;
    const callbacks = {
      message: (message: string | ArrayBuffer) => this.handleMessage(message),
      open: () => {
        opened = true;
        this.callbacks.onConnection('connected');
      },
      close: () => {
        if (version !== this.socketVersion || this.destroyed) return;
        if (!opened && !usingFallback) {
          usingFallback = true;
          this.transport?.close();
          this.transport = connectWebSocket(this.roomId, callbacks);
          return;
        }
        this.receivingAudio = null;
        this.player.stop();
        this.callbacks.onConnection('offline');
        if (this.recording) void this.stopRecording();
        this.reconnectTimer = window.setTimeout(() => this.connect(), 1800);
      },
    };
    this.transport = connectGrpcRealtime(this.roomId, callbacks);
  }

  sendConfig() {
    if (!this.canPublish) return;
    this.sendJson({ type: 'configure', ...this.config() });
  }

  setCanPublish(canPublish: boolean) {
    if (this.canPublish === canPublish) return;
    this.canPublish = canPublish;
    if (!canPublish && this.recording) void this.stopRecording();
  }

  async toggleRecording() {
    if (!this.canPublish) return;
    if (this.recording) await this.stopRecording();
    else {
      if (!this.transport?.open) {
        throw new Error('实时会话尚未连接，请稍后重试');
      }
      await this.startRecording();
    }
  }

  reconfigureCapture() {
    this.captureOptionsRevision += 1;
    if (!this.recording) return Promise.resolve();
    this.captureReconfigurePromise ??= this.applyLatestCaptureOptions().finally(() => {
      this.captureReconfigurePromise = null;
    });
    return this.captureReconfigurePromise;
  }

  private async applyLatestCaptureOptions() {
    try {
      while (this.recording) {
        const revision = this.captureOptionsRevision;
        const options = this.captureOptions();
        if (!this.audioCapture.updateOptions(options)) {
          await this.audioCapture.stop();
          if (!this.recording) return;
          await this.beginCapture(options);
          this.syncPlaybackSuppression();
        }
        if (revision === this.captureOptionsRevision) return;
      }
    } catch (error) {
      const captureError = error instanceof Error ? error : new Error('无法更新录音设置');
      this.callbacks.onCaptureError(captureError.message);
      if (this.recording) await this.stopRecording();
    }
  }

  setMuted(muted: boolean) {
    this.player.muted = muted;
    if (muted) this.player.stop();
  }

  setExternalPlaybackActive(active: boolean) {
    this.setPlaybackHold('media', active);
  }

  async destroy() {
    this.destroyed = true;
    if (this.recording) await this.stopRecording();
    this.player.setPlaybackListener(null);
    this.playbackReleaseTimers.forEach((timer) => window.clearTimeout(timer));
    this.playbackReleaseTimers.clear();
    this.playbackHolds.clear();
    this.disconnect();
    this.audioCapture.dispose();
    this.waveform.destroy();
  }

  private disconnect() {
    this.socketVersion += 1;
    window.clearTimeout(this.reconnectTimer);
    this.transport?.close();
    this.transport = null;
  }

  private async startRecording() {
    if (!this.canPublish) return;
    this.activeEnhancedVoiceFilter = this.enhancedVoiceFilter();
    this.activeTcId = null;
    this.activeSampleCount = 0;
    this.segmentsStarted = 0;
    try {
      // getDisplayMedia must be invoked before any awaited work consumes the click gesture.
      const capture = this.beginCapture(this.captureOptions());
      const playbackUnlock = this.player.unlock();
      await Promise.all([capture, playbackUnlock]);
    } catch (error) {
      throw error;
    }
    this.recording = true;
    this.syncPlaybackSuppression();
    this.waveform.setActive(true);
    this.callbacks.onRecording(true);
  }

  private beginCapture(options: AudioCaptureOptions) {
    return this.audioCapture.start(
      this.config().max_utterance_seconds,
      this.activeEnhancedVoiceFilter,
      options,
      (pcm) => {
        if (this.activeTcId && this.transport?.open) {
          this.transport.sendAudio(pcm);
          this.activeSampleCount += pcm.byteLength / Int16Array.BYTES_PER_ELEMENT;
        }
      },
      (level) => this.waveform.push(level),
      (boundary) => this.handleVadBoundary(boundary),
      () => this.sendConfig(),
      (error) => void this.handleCaptureFailure(error),
    );
  }

  private async stopRecording() {
    this.recording = false;
    this.syncPlaybackSuppression();
    await this.audioCapture.stop();
    if (this.activeTcId) {
      this.finishVadSegment('manual');
    }
    if (this.segmentsStarted === 0) {
      const tcId = crypto.randomUUID();
      this.sendJson({
        type: 'start',
        tc_id: tcId,
        vad: this.vadStartMetadata(),
        ...this.config(),
      });
      this.sendJson({
        type: 'end',
        tc_id: tcId,
        is_silent_vad: true,
        vad: { reason: 'silent', sample_count: 0, speech_frames: 0 },
      });
    }
    this.sendJson({ type: 'flush' });
    this.segmentsStarted = 0;
    this.waveform.setActive(false);
    this.callbacks.onRecording(false);
  }

  private handleVadBoundary(boundary: VadBoundary) {
    if (boundary.type === 'speech_start') {
      if (this.activeTcId) {
        this.finishVadSegment('superseded');
      }
      const tcId = crypto.randomUUID();
      this.activeTcId = tcId;
      this.activeSampleCount = 0;
      this.segmentsStarted += 1;
      this.sendJson({
        type: 'start',
        tc_id: tcId,
        vad: this.vadStartMetadata(),
        ...this.config(),
      });
      return;
    }
    if (!this.activeTcId) return;
    this.finishVadSegment(boundary.reason, boundary.speechFrames);
  }

  private finishVadSegment(
    reason: 'silence' | 'max_duration' | 'manual' | 'superseded',
    speechFrames?: number,
  ) {
    if (!this.activeTcId) return;
    const vad = {
      reason,
      sample_count: this.activeSampleCount,
      ...(speechFrames === undefined ? {} : { speech_frames: speechFrames }),
    };
    this.sendJson({
      type: 'end',
      tc_id: this.activeTcId,
      is_silent_vad:
        speechFrames !== undefined && speechFrames < MIN_VALID_SPEECH_FRAMES,
      vad,
    });
    this.activeTcId = null;
    this.activeSampleCount = 0;
  }

  private vadStartMetadata() {
    return {
      engine: this.activeEnhancedVoiceFilter
        ? 'silero-v6.2-lele-enhanced'
        : 'silero-v6.2-lele',
      sample_rate: 16_000,
      frame_samples: 512,
      pre_roll_samples: this.activeEnhancedVoiceFilter ? 16_384 : 8_192,
    };
  }

  private async handleCaptureFailure(error: Error) {
    this.callbacks.onCaptureError(error.message);
    if (this.recording) await this.stopRecording();
  }

  private setPlaybackHold(kind: 'stream' | 'media', active: boolean) {
    const pendingRelease = this.playbackReleaseTimers.get(kind);
    if (pendingRelease) window.clearTimeout(pendingRelease);
    this.playbackReleaseTimers.delete(kind);
    if (active) {
      this.playbackHolds.add(kind);
      this.syncPlaybackSuppression();
      return;
    }
    const timer = window.setTimeout(() => {
      this.playbackReleaseTimers.delete(kind);
      this.playbackHolds.delete(kind);
      this.syncPlaybackSuppression();
    }, PLAYBACK_TAIL_GUARD_MS);
    this.playbackReleaseTimers.set(kind, timer);
  }

  private syncPlaybackSuppression() {
    this.audioCapture.setSuppressed(this.recording && this.playbackHolds.size > 0);
  }

  private handleMessage(message: string | ArrayBuffer) {
    if (message instanceof ArrayBuffer) {
      const playback = this.receivingAudio;
      if (!playback) return;
      void this.player.enqueue(message, this.audioSampleRate, playback.channels);
      return;
    }
    let event: ServerEvent;
    try {
      event = JSON.parse(message) as ServerEvent;
    } catch {
      this.callbacks.onCaptureError('服务端返回了无效的实时消息');
      return;
    }
    if (event.type === 'room_subscribed') {
      this.setCanPublish(event.can_publish);
      if (event.can_publish) this.sendConfig();
    }
    if (event.type === 'ready' && this.canPublish) this.sendConfig();
    if (event.type === 'audio_start') {
      if (event.codec !== 'pcm_s16le') {
        this.callbacks.onCaptureError(`浏览器暂不支持实时播放 ${event.codec} 音频`);
        this.receivingAudio = null;
        return;
      }
      this.audioSampleRate = event.sample_rate;
      this.receivingAudio = {
        utteranceId: event.utterance_id,
        channels: event.channels,
      };
    }
    if (event.type === 'audio_end') {
      if (this.receivingAudio?.utteranceId === event.utterance_id) this.receivingAudio = null;
    }
    this.callbacks.onEvent(event);
  }

  private sendJson(payload: object) {
    if (this.transport?.open) this.transport.sendJson(payload);
  }
}

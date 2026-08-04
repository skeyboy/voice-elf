import { MicrophoneCapture, PcmPlayer, Waveform, type VadBoundary } from '../audio';
import type { ServerEvent, SessionConfig } from '../protocol';
import type { ConnectionStatus } from '../components/topbar';

interface PlaybackProgress {
  utteranceId: string;
  currentSeconds: number;
  durationSeconds: number;
}

const MIN_VALID_SPEECH_FRAMES = 6;
const PLAYBACK_TAIL_GUARD_MS = 300;

interface VoiceSessionCallbacks {
  onEvent: (event: ServerEvent) => void;
  onConnection: (status: ConnectionStatus) => void;
  onRecording: (recording: boolean) => void;
  onCaptureError: (message: string) => void;
  onPlaybackProgress: (progress: PlaybackProgress) => void;
}

export class VoiceSession {
  private readonly microphone = new MicrophoneCapture();
  private readonly waveform: Waveform;
  private socket: WebSocket | null = null;
  private socketVersion = 0;
  private reconnectTimer = 0;
  private receivingAudio: {
    utteranceId: string;
    sampleRate: number;
    sampleCount: number;
    playedSamples: number;
  } | null = null;
  private audioSampleRate = 24_000;
  private recording = false;
  private activeTcId: string | null = null;
  private activeSampleCount = 0;
  private segmentsStarted = 0;
  private activeEnhancedVoiceFilter = false;
  private readonly playbackHolds = new Set<'stream' | 'media'>();
  private readonly playbackReleaseTimers = new Map<'stream' | 'media', number>();
  private destroyed = false;

  constructor(
    private readonly roomId: string,
    private canPublish: boolean,
    canvas: HTMLCanvasElement,
    private readonly player: PcmPlayer,
    private readonly config: () => SessionConfig,
    private readonly enhancedVoiceFilter: () => boolean,
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
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    this.socket = new WebSocket(
      `${scheme}//${window.location.host}/ws?room_id=${encodeURIComponent(this.roomId)}`,
    );
    this.socket.binaryType = 'arraybuffer';
    this.socket.onopen = () => this.callbacks.onConnection('connected');
    this.socket.onmessage = (message) => this.handleMessage(message);
    this.socket.onerror = () => this.callbacks.onConnection('offline');
    this.socket.onclose = () => {
      if (version !== this.socketVersion || this.destroyed) return;
      this.receivingAudio = null;
      this.player.stop();
      this.callbacks.onConnection('offline');
      if (this.recording) void this.stopRecording();
      this.reconnectTimer = window.setTimeout(() => this.connect(), 1800);
    };
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
    else await this.startRecording();
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
    this.microphone.dispose();
    this.waveform.destroy();
  }

  private disconnect() {
    this.socketVersion += 1;
    window.clearTimeout(this.reconnectTimer);
    if (this.socket) {
      this.socket.onclose = null;
      this.socket.close();
    }
    this.socket = null;
  }

  private async startRecording() {
    if (!this.canPublish) return;
    if (this.socket?.readyState !== WebSocket.OPEN) return;
    await this.player.unlock();
    this.activeEnhancedVoiceFilter = this.enhancedVoiceFilter();
    this.activeTcId = null;
    this.activeSampleCount = 0;
    this.segmentsStarted = 0;
    try {
      await this.microphone.start(
        this.config().max_utterance_seconds,
        this.activeEnhancedVoiceFilter,
        (pcm) => {
          if (this.activeTcId && this.socket?.readyState === WebSocket.OPEN) {
            this.socket.send(pcm);
            this.activeSampleCount += pcm.byteLength / Int16Array.BYTES_PER_ELEMENT;
          }
        },
        (level) => this.waveform.push(level),
        (boundary) => this.handleVadBoundary(boundary),
        () => {
          this.sendConfig();
        },
        (error) => void this.handleCaptureFailure(error),
      );
    } catch (error) {
      throw error;
    }
    this.recording = true;
    this.syncPlaybackSuppression();
    this.waveform.setActive(true);
    this.callbacks.onRecording(true);
  }

  private async stopRecording() {
    this.recording = false;
    this.syncPlaybackSuppression();
    await this.microphone.stop();
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
    this.microphone.setSuppressed(this.recording && this.playbackHolds.size > 0);
  }

  private handleMessage(message: MessageEvent) {
    if (message.data instanceof ArrayBuffer) {
      const playback = this.receivingAudio;
      if (!playback) return;
      const chunkSamples = message.data.byteLength / Int16Array.BYTES_PER_ELEMENT;
      void this.player.enqueue(message.data, this.audioSampleRate, () => {
        if (!playback) return;
        playback.playedSamples += chunkSamples;
        this.callbacks.onPlaybackProgress({
          utteranceId: playback.utteranceId,
          currentSeconds: playback.playedSamples / playback.sampleRate,
          durationSeconds: playback.sampleCount / playback.sampleRate,
        });
      });
      return;
    }
    let event: ServerEvent;
    try {
      event = JSON.parse(message.data as string) as ServerEvent;
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
      this.audioSampleRate = event.sample_rate;
      this.receivingAudio = {
        utteranceId: event.utterance_id,
        sampleRate: event.sample_rate,
        sampleCount: event.sample_count,
        playedSamples: 0,
      };
      this.callbacks.onPlaybackProgress({
        utteranceId: event.utterance_id,
        currentSeconds: 0,
        durationSeconds: event.sample_count / event.sample_rate,
      });
    }
    if (event.type === 'audio_end') this.receivingAudio = null;
    this.callbacks.onEvent(event);
  }

  private sendJson(payload: object) {
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(JSON.stringify(payload));
  }
}

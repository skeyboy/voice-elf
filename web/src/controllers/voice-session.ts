import { MicrophoneCapture, PcmPlayer, Waveform } from '../audio';
import type { ServerEvent, SessionConfig } from '../protocol';
import type { ConnectionStatus } from '../components/topbar';

interface PlaybackProgress {
  utteranceId: string;
  currentSeconds: number;
  durationSeconds: number;
}

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
  private destroyed = false;

  constructor(
    private readonly roomId: string,
    canvas: HTMLCanvasElement,
    private readonly player: PcmPlayer,
    private readonly config: () => SessionConfig,
    private readonly callbacks: VoiceSessionCallbacks,
  ) {
    this.waveform = new Waveform(canvas);
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
      this.callbacks.onConnection('offline');
      if (this.recording) void this.stopRecording();
      this.reconnectTimer = window.setTimeout(() => this.connect(), 1800);
    };
  }

  sendConfig() {
    this.sendJson({ type: 'configure', ...this.config() });
  }

  async toggleRecording() {
    if (this.recording) await this.stopRecording();
    else await this.startRecording();
  }

  setMuted(muted: boolean) {
    this.player.muted = muted;
    if (muted) this.player.stop();
  }

  async destroy() {
    this.destroyed = true;
    if (this.recording) await this.stopRecording();
    this.disconnect();
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
    if (this.socket?.readyState !== WebSocket.OPEN) return;
    await this.player.unlock();
    try {
      await this.microphone.start(
        this.config().max_utterance_seconds,
        (pcm) => {
          if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(pcm);
        },
        (level) => this.waveform.push(level),
        (boundary) => this.sendJson({ type: boundary }),
        () => {
          this.sendConfig();
          this.sendJson({ type: 'start' });
        },
        (error) => void this.handleCaptureFailure(error),
      );
    } catch (error) {
      throw error;
    }
    this.recording = true;
    this.waveform.setActive(true);
    this.callbacks.onRecording(true);
  }

  private async stopRecording() {
    this.recording = false;
    await this.microphone.stop();
    this.sendJson({ type: 'flush' });
    this.waveform.setActive(false);
    this.callbacks.onRecording(false);
  }

  private async handleCaptureFailure(error: Error) {
    this.callbacks.onCaptureError(error.message);
    if (this.recording) await this.stopRecording();
  }

  private handleMessage(message: MessageEvent) {
    if (message.data instanceof ArrayBuffer) {
      const playback = this.receivingAudio;
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
    const event = JSON.parse(message.data as string) as ServerEvent;
    if (event.type === 'ready') this.sendConfig();
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

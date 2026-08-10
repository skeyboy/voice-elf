import {
  isAndroidNativeShell,
  markAndroidCaptureReady,
  startAndroidCapture,
  stopAndroidCapture,
  subscribeAndroidNative,
  supportsAndroidSystemAudio,
  updateAndroidCapture,
} from './android-native';
import {
  isMacNativeShell,
  startMacSystemAudioCapture,
  stopMacSystemAudioCapture,
  subscribeMacNativeAudio,
} from './macos-native';

interface CaptureMessage {
  type: 'samples';
  payload?: ArrayBuffer;
}

export type VadEndReason = 'silence' | 'max_duration' | 'manual';

export type VadBoundary =
  | { type: 'speech_start' }
  | { type: 'speech_end'; reason: VadEndReason; speechFrames: number };

export interface AudioCaptureOptions {
  microphone: boolean;
  systemAudio: boolean;
  noiseSuppression: boolean;
  echoCancellation: boolean;
}

interface VadMessage {
  type:
    | 'initializing'
    | 'ready'
    | 'pcm'
    | 'level'
    | 'speech_start'
    | 'speech_end'
    | 'flushed'
    | 'preloaded'
    | 'error';
  payload?: ArrayBuffer;
  message?: string;
  value?: number;
  reason?: VadEndReason;
  speechFrames?: number;
  stage?: 'manifest' | 'download' | 'compile';
  loadedBytes?: number;
  totalBytes?: number;
}

const VAD_STAGE_LABELS = {
  worker: '启动 Worker',
  manifest: '读取资源清单',
  download: '下载 WASM',
  compile: '编译 WASM',
} as const;
const VAD_STAGE_TIMEOUT_MS: Record<keyof typeof VAD_STAGE_LABELS, number> = {
  worker: 20_000,
  manifest: 30_000,
  download: 300_000,
  compile: 90_000,
};

let sharedVadWorker: Worker | null = null;

type ExtendedDisplayMediaStreamOptions = Omit<DisplayMediaStreamOptions, 'audio'> & {
  audio?: boolean | (MediaTrackConstraints & { suppressLocalAudioPlayback?: boolean });
  preferCurrentTab?: boolean;
  selfBrowserSurface?: 'include' | 'exclude';
  systemAudio?: 'include' | 'exclude';
  surfaceSwitching?: 'include' | 'exclude';
  windowAudio?: 'exclude' | 'system' | 'window';
};

function isMacOSBrowser() {
  return /Macintosh|Mac OS X/i.test(navigator.userAgent);
}

function isChromiumBrowser() {
  return /Chrome|Chromium|CriOS|Edg\//i.test(navigator.userAgent);
}

function webDisplayAudioOptions(): ExtendedDisplayMediaStreamOptions {
  return {
    video: { displaySurface: 'browser' },
    audio: { suppressLocalAudioPlayback: false },
    preferCurrentTab: false,
    selfBrowserSurface: 'exclude',
    systemAudio: 'include',
    surfaceSwitching: 'include',
    windowAudio: 'window',
  };
}

function decodeBase64(value: string) {
  const binary = window.atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes.buffer;
}

function acquireVadWorker() {
  sharedVadWorker ??= new Worker('/vad-worker.js', {
    type: 'module',
    name: 'voice-elf-vad',
  });
  return sharedVadWorker;
}

export function scheduleVadPreload() {
  const idleApi = window as unknown as {
    requestIdleCallback?: (callback: IdleRequestCallback, options?: IdleRequestOptions) => number;
    cancelIdleCallback?: (handle: number) => void;
  };
  let cancelled = false;
  let idleId = 0;
  let fallbackTimer = 0;
  const preload = () => {
    if (!cancelled) acquireVadWorker().postMessage({ type: 'preload' });
  };
  const scheduleWhenIdle = () => {
    if (idleApi.requestIdleCallback) {
      idleId = idleApi.requestIdleCallback(preload, { timeout: 5_000 });
    } else {
      fallbackTimer = window.setTimeout(preload, 1_500);
    }
  };
  if (document.readyState === 'complete') scheduleWhenIdle();
  else window.addEventListener('load', scheduleWhenIdle, { once: true });

  return () => {
    cancelled = true;
    window.removeEventListener('load', scheduleWhenIdle);
    if (idleId) idleApi.cancelIdleCallback?.(idleId);
    window.clearTimeout(fallbackTimer);
  };
}

export function supportsSystemAudioCapture() {
  if (supportsAndroidSystemAudio() || isMacNativeShell()) return true;
  // Safari and Firefox expose getDisplayMedia on macOS but return video-only streams.
  if (isMacOSBrowser() && !isChromiumBrowser()) return false;
  return Boolean(
    window.isSecureContext && navigator.mediaDevices?.getDisplayMedia,
  );
}

export function systemAudioCaptureHelp() {
  if (isMacNativeShell()) return '设备播放声音';
  if (isMacOSBrowser() && isChromiumBrowser()) {
    return '请选择带声音的 Chrome 标签页或窗口，并开启共享音频';
  }
  if (isMacOSBrowser()) return 'macOS 网页内录需要使用 Chrome 浏览器';
  return '标签页、窗口或设备播放声音';
}

async function assertMicrophonePermissionIsRequestable() {
  if (!navigator.permissions?.query) return;
  try {
    const status = await navigator.permissions.query({
      name: 'microphone' as PermissionName,
    });
    if (status.state === 'denied') {
      throw new Error('麦克风权限已被拒绝，请在系统或浏览器设置中允许后重试');
    }
  } catch (error) {
    if (error instanceof Error && error.message.includes('麦克风权限已被拒绝')) throw error;
    // Safari and older WebViews do not expose microphone through Permissions API.
  }
}

function captureError(error: unknown, source: 'microphone' | 'system') {
  if (!(error instanceof DOMException)) {
    return error instanceof Error ? error : new Error('无法启动音频采集');
  }
  if (error.name === 'NotAllowedError') {
    return new Error(
      source === 'system'
        ? '未获得系统音频共享权限，已取消本次录音'
        : '未获得麦克风权限，请在系统或浏览器设置中允许后重试',
    );
  }
  if (error.name === 'NotFoundError') {
    return new Error(source === 'system' ? '没有可共享的系统音频' : '未检测到可用麦克风');
  }
  if (error.name === 'NotReadableError') {
    return new Error(
      source === 'system' ? '系统音频正在被占用或无法读取' : '麦克风正在被其他应用占用',
    );
  }
  return error;
}

export class AudioCapture {
  private context: AudioContext | null = null;
  private streams: MediaStream[] = [];
  private sourceNodes: MediaStreamAudioSourceNode[] = [];
  private gainNodes: GainNode[] = [];
  private mixer: GainNode | null = null;
  private limiter: DynamicsCompressorNode | null = null;
  private node: AudioWorkletNode | null = null;
  private vadWorker: Worker | null = null;
  private vadReady = false;
  private onPcm: ((pcm: ArrayBuffer) => void) | null = null;
  private onLevel: ((level: number) => void) | null = null;
  private onBoundary: ((boundary: VadBoundary) => void) | null = null;
  private onFatalError: ((error: Error) => void) | null = null;
  private flushResolver: (() => void) | null = null;
  private suppressed = false;
  private stopping = false;
  private androidCapture = false;
  private macCapture = false;
  private macCaptureReady = false;
  private currentOptions: AudioCaptureOptions | null = null;
  private unsubscribeAndroidPcm = () => {};
  private unsubscribeMacPcm = () => {};

  private async prepareVad(
    maxUtteranceSeconds: number,
    inputSampleRate: number,
    enhancedVoiceFilter: boolean,
  ) {
    const worker = this.vadWorker ?? acquireVadWorker();
    this.vadWorker = worker;
    this.vadReady = false;
    const ready = new Promise<void>((resolve, reject) => {
      let initialized = false;
      let stage: keyof typeof VAD_STAGE_LABELS = 'worker';
      let loadedBytes = 0;
      let totalBytes = 0;
      let timeout = 0;
      const armTimeout = () => {
        window.clearTimeout(timeout);
        timeout = window.setTimeout(() => {
          const progress =
            stage === 'download' && loadedBytes > 0
              ? `，已接收 ${(loadedBytes / 1024 / 1024).toFixed(1)} MB${totalBytes > 0 ? ` / ${(totalBytes / 1024 / 1024).toFixed(1)} MB` : ''}`
              : '';
          reject(new Error(`浏览器音频 VAD 初始化超时（${VAD_STAGE_LABELS[stage]}${progress}）`));
        }, VAD_STAGE_TIMEOUT_MS[stage]);
      };
      armTimeout();
      worker.onmessage = (event: MessageEvent<VadMessage>) => {
        const message = event.data;
        if (message.type === 'initializing' && message.stage) {
          stage = message.stage;
          loadedBytes = message.loadedBytes ?? loadedBytes;
          totalBytes = message.totalBytes ?? totalBytes;
          armTimeout();
        } else if (message.type === 'ready') {
          window.clearTimeout(timeout);
          initialized = true;
          this.vadReady = true;
          resolve();
        } else if (message.type === 'pcm' && message.payload) {
          this.onPcm?.(message.payload);
        } else if (message.type === 'level') {
          this.onLevel?.(message.value ?? 0);
        } else if (message.type === 'speech_start' || message.type === 'speech_end') {
          this.onBoundary?.(
            message.type === 'speech_start'
              ? { type: 'speech_start' }
              : {
                  type: 'speech_end',
                  reason: message.reason ?? 'silence',
                  speechFrames: message.speechFrames ?? 0,
                },
          );
        } else if (message.type === 'flushed') {
          this.flushResolver?.();
          this.flushResolver = null;
        } else if (message.type === 'error') {
          const error = new Error(message.message || '浏览器音频 VAD 运行失败');
          this.vadReady = false;
          window.clearTimeout(timeout);
          if (initialized) {
            this.destroyVad();
            this.onFatalError?.(error);
          } else {
            reject(error);
          }
        }
      };
      worker.onerror = () => {
        const error = new Error('浏览器音频 VAD Worker 运行失败');
        this.vadReady = false;
        window.clearTimeout(timeout);
        if (initialized) {
          this.destroyVad();
          this.onFatalError?.(error);
        } else {
          reject(error);
        }
      };
    });
    worker.postMessage({
      type: 'init',
      maxUtteranceSeconds,
      inputSampleRate,
      enhancedVoiceFilter,
    });
    await ready;
  }

  async start(
    maxUtteranceSeconds: number,
    enhancedVoiceFilter: boolean,
    options: AudioCaptureOptions,
    onPcm: (pcm: ArrayBuffer) => void,
    onLevel: (level: number) => void,
    onBoundary: (boundary: VadBoundary) => void,
    onReady: () => void,
    onFatalError: (error: Error) => void,
  ) {
    if (this.context) return;
    const androidShell = isAndroidNativeShell();
    const macShell = isMacNativeShell();
    if ((!window.isSecureContext || !navigator.mediaDevices) && !androidShell && !macShell) {
      throw new Error('当前访问地址不是浏览器安全上下文；音频采集需要使用受信任的 HTTPS 地址');
    }
    if (!options.microphone && !options.systemAudio) {
      throw new Error('请至少选择麦克风或系统音频');
    }
    const streams: MediaStream[] = [];
    if (androidShell) {
      this.androidCapture = await startAndroidCapture(
        options.microphone,
        options.systemAudio,
        options.noiseSuppression,
        options.echoCancellation,
      );
    }
    if (options.systemAudio) {
      if (!supportsSystemAudioCapture()) {
        throw new Error('当前浏览器或设备不支持系统音频采集');
      }
      try {
        if (androidShell) {
          // MediaProjection audio arrives through the native bridge after VAD is ready.
        } else if (macShell) {
          // ScreenCaptureKit PCM starts after AudioWorklet and VAD are ready.
        } else {
          const display = await navigator.mediaDevices.getDisplayMedia(webDisplayAudioOptions());
          if (display.getAudioTracks().length === 0) {
            const surface = display.getVideoTracks()[0]?.getSettings().displaySurface;
            display.getTracks().forEach((track) => track.stop());
            if (isMacOSBrowser()) {
              if (surface === 'browser') {
                throw new Error(
                  '所选 Chrome 标签页未共享音频，请重新选择正在播放声音的标签页并开启“共享标签页音频”',
                );
              }
              if (surface === 'window') {
                throw new Error(
                  '所选窗口没有提供音频轨道，请选择支持窗口音频的来源；仍不可用时请改选带声音的 Chrome 标签页',
                );
              }
              throw new Error(
                '整个屏幕没有提供系统音频轨道，请改选带声音的 Chrome 标签页或支持音频的窗口',
              );
            }
            throw new Error('所选共享来源没有音频轨道，请重新选择并开启共享音频');
          }
          streams.push(display);
        }
      } catch (error) {
        if (this.androidCapture) stopAndroidCapture();
        throw captureError(error, 'system');
      }
    }
    const nativeMixedCapture = androidShell && options.systemAudio;
    if (options.microphone && !nativeMixedCapture) {
      if (!navigator.mediaDevices.getUserMedia) {
        streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
        throw new Error('当前浏览器或设备不支持麦克风录音');
      }
      try {
        await assertMicrophonePermissionIsRequestable();
        streams.push(
          await navigator.mediaDevices.getUserMedia({
            audio: {
              channelCount: 1,
              echoCancellation: options.echoCancellation,
              noiseSuppression: options.noiseSuppression,
              autoGainControl: false,
            },
          }),
        );
      } catch (error) {
        streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
        if (this.androidCapture) stopAndroidCapture();
        throw captureError(error, 'microphone');
      }
    }

    const context = new AudioContext({ latencyHint: 'interactive' });
    this.onLevel = onLevel;
    this.onFatalError = onFatalError;
    try {
      await Promise.all([
        context.audioWorklet.addModule('/audio-processor.js'),
        this.prepareVad(
          maxUtteranceSeconds,
          nativeMixedCapture ? 48_000 : context.sampleRate,
          enhancedVoiceFilter,
        ),
      ]);
      await context.resume();
    } catch (error) {
      streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
      await context.close();
      this.destroyVad();
      this.onLevel = null;
      this.onFatalError = null;
      if (this.androidCapture) stopAndroidCapture();
      this.androidCapture = false;
      throw error;
    }
    const audioStreams = streams.map(
      (stream) => new MediaStream(stream.getAudioTracks()),
    );
    const sourceNodes = audioStreams.map((stream) => context.createMediaStreamSource(stream));
    const gainNodes = sourceNodes.map(() => context.createGain());
    const mixer = context.createGain();
    mixer.channelCount = 1;
    mixer.channelCountMode = 'explicit';
    mixer.channelInterpretation = 'speakers';
    const limiter = context.createDynamicsCompressor();
    limiter.threshold.value = -3;
    limiter.knee.value = 6;
    limiter.ratio.value = 12;
    limiter.attack.value = 0.003;
    limiter.release.value = 0.12;
    const inputGain = sourceNodes.length > 1 ? Math.SQRT1_2 : 1;
    sourceNodes.forEach((source, index) => {
      gainNodes[index].gain.value = inputGain;
      source.connect(gainNodes[index]).connect(mixer);
    });
    const node = sourceNodes.length > 0 || (macShell && options.systemAudio)
      ? new AudioWorkletNode(context, 'microphone-tap-processor')
      : null;
    if (node) {
      const silent = context.createGain();
      silent.gain.value = 0;
      mixer.connect(limiter).connect(node).connect(silent).connect(context.destination);
    }
    this.onPcm = onPcm;
    this.onBoundary = onBoundary;
    if (node) node.port.onmessage = (event: MessageEvent<CaptureMessage>) => {
      if (
        event.data.type === 'samples' &&
        event.data.payload &&
        this.vadReady &&
        this.vadWorker &&
        !this.suppressed
      ) {
        this.vadWorker.postMessage(
          { type: 'samples', payload: event.data.payload },
          [event.data.payload],
        );
      }
    };
    this.unsubscribeAndroidPcm = subscribeAndroidNative((event) => {
      if (
        event.type === 'audio-pcm'
        && this.vadReady
        && this.vadWorker
        && !this.suppressed
      ) {
        if (event.sampleRate !== 48_000) {
          this.onFatalError?.(new Error(`Android 内录采样率异常（${event.sampleRate} Hz）`));
          return;
        }
        this.vadWorker.postMessage({ type: 'android_pcm', payload: event.data });
      } else if (event.type === 'capture-stopped' && !this.stopping && this.context) {
        this.onFatalError?.(new Error('Android 后台录音已停止'));
      } else if (event.type === 'capture-error' && !this.stopping) {
        this.onFatalError?.(new Error(event.message));
      }
    });
    this.unsubscribeMacPcm = subscribeMacNativeAudio((event) => {
      if (event.type === 'audio-pcm' && this.node && !this.stopping) {
        const payload = decodeBase64(event.data);
        this.node.port.postMessage(
          { type: 'external-pcm', payload, sampleRate: event.sampleRate },
          [payload],
        );
      } else if (
        event.type === 'capture-stopped'
        && this.macCaptureReady
        && !this.stopping
        && this.context
      ) {
        this.onFatalError?.(new Error('macOS 系统内录已停止'));
      } else if (event.type === 'capture-error' && this.macCaptureReady && !this.stopping) {
        this.onFatalError?.(new Error(event.message));
      }
    });
    const displayStream = options.systemAudio && !androidShell && !macShell ? streams[0] : null;
    displayStream?.getAudioTracks().forEach((track) => {
      track.addEventListener(
        'ended',
        () => {
          if (!this.stopping && this.context) {
            this.onFatalError?.(new Error('系统音频共享已停止'));
          }
        },
        { once: true },
      );
    });
    this.streams = streams;
    this.sourceNodes = sourceNodes;
    this.gainNodes = gainNodes;
    this.mixer = mixer;
    this.limiter = limiter;
    this.context = context;
    this.node = node;
    this.currentOptions = { ...options };
    if (macShell && options.systemAudio) {
      if (!node) {
        await this.stop();
        throw new Error('无法初始化 macOS 系统音频混音器');
      }
      node.port.postMessage({ type: 'external-start' });
      this.macCapture = true;
      try {
        await startMacSystemAudioCapture();
        this.macCaptureReady = true;
      } catch (error) {
        await this.stop();
        throw error;
      }
    }
    if (this.androidCapture) markAndroidCaptureReady();
    onReady();
  }

  async stop() {
    this.stopping = true;
    this.macCaptureReady = false;
    this.node?.disconnect();
    if (this.node) this.node.port.onmessage = null;
    this.sourceNodes.forEach((source) => source.disconnect());
    this.gainNodes.forEach((gain) => gain.disconnect());
    this.mixer?.disconnect();
    this.limiter?.disconnect();
    this.streams.forEach((stream) => stream.getTracks().forEach((track) => track.stop()));
    this.unsubscribeAndroidPcm();
    this.unsubscribeAndroidPcm = () => {};
    this.unsubscribeMacPcm();
    this.unsubscribeMacPcm = () => {};
    if (this.androidCapture) stopAndroidCapture();
    if (this.macCapture) await stopMacSystemAudioCapture();
    if (this.vadReady && this.vadWorker) {
      await new Promise<void>((resolve) => {
        const timeout = window.setTimeout(resolve, 1_000);
        this.flushResolver = () => {
          window.clearTimeout(timeout);
          resolve();
        };
        this.vadWorker?.postMessage({ type: 'flush' });
      });
    }
    await this.context?.close();
    this.node = null;
    this.streams = [];
    this.sourceNodes = [];
    this.gainNodes = [];
    this.mixer = null;
    this.limiter = null;
    this.context = null;
    this.androidCapture = false;
    this.macCapture = false;
    this.currentOptions = null;
    this.onPcm = null;
    this.onLevel = null;
    this.onBoundary = null;
    this.onFatalError = null;
    this.vadReady = false;
    this.stopping = false;
  }

  updateOptions(options: AudioCaptureOptions) {
    if (
      !this.context
      || !this.androidCapture
      || !this.currentOptions?.systemAudio
      || !options.systemAudio
    ) {
      return false;
    }
    updateAndroidCapture(
      options.microphone,
      options.systemAudio,
      options.noiseSuppression,
      options.echoCancellation,
    );
    this.currentOptions = { ...options };
    return true;
  }

  setSuppressed(suppressed: boolean) {
    if (this.suppressed === suppressed) return;
    this.suppressed = suppressed;
    this.vadWorker?.postMessage({ type: 'suppress', value: suppressed });
    if (suppressed) this.onLevel?.(0);
  }

  dispose() {
    this.destroyVad();
  }

  private destroyVad() {
    const worker = this.vadWorker;
    worker?.terminate();
    if (sharedVadWorker === worker) sharedVadWorker = null;
    this.vadWorker = null;
    this.vadReady = false;
    this.suppressed = false;
    this.flushResolver = null;
  }
}

export class PcmPlayer {
  private context: AudioContext | null = null;
  private nextStart = 0;
  private generation = 0;
  private sources = new Set<AudioBufferSourceNode>();
  private pendingEnqueues = 0;
  private playbackListener: ((active: boolean) => void) | null = null;
  private playbackActive = false;
  muted = false;

  async unlock() {
    this.context ??= new AudioContext({ latencyHint: 'interactive' });
    if (this.context.state === 'suspended') await this.context.resume();
  }

  async enqueue(
    bytes: ArrayBuffer,
    sampleRate: number,
    channels = 1,
    onEnded?: () => void,
  ) {
    if (this.muted) return;
    if (channels < 1 || bytes.byteLength % (channels * Int16Array.BYTES_PER_ELEMENT) !== 0) {
      throw new Error('服务端返回了无效的音频帧');
    }
    this.pendingEnqueues += 1;
    this.updatePlaybackState();
    try {
      await this.unlock();
      const context = this.context as AudioContext;
      const samples = new Int16Array(bytes);
      const frameCount = samples.length / channels;
      const buffer = context.createBuffer(channels, frameCount, sampleRate);
      for (let channelIndex = 0; channelIndex < channels; channelIndex += 1) {
        const channel = buffer.getChannelData(channelIndex);
        for (let frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
          channel[frameIndex] = samples[frameIndex * channels + channelIndex] / 32768;
        }
      }
      const source = context.createBufferSource();
      const generation = this.generation;
      source.buffer = buffer;
      source.connect(context.destination);
      this.sources.add(source);
      source.onended = () => {
        this.sources.delete(source);
        if (generation === this.generation) onEnded?.();
        this.updatePlaybackState();
      };
      const startAt = Math.max(context.currentTime + 0.025, this.nextStart);
      source.start(startAt);
      this.nextStart = startAt + buffer.duration;
    } finally {
      this.pendingEnqueues -= 1;
      this.updatePlaybackState();
    }
  }

  setPlaybackListener(listener: ((active: boolean) => void) | null) {
    this.playbackListener = listener;
    listener?.(this.playbackActive);
  }

  reset() {
    this.nextStart = this.context?.currentTime ?? 0;
  }

  stop() {
    this.generation += 1;
    this.sources.forEach((source) => source.stop());
    this.sources.clear();
    this.reset();
    this.updatePlaybackState();
  }

  private updatePlaybackState() {
    const active = this.pendingEnqueues > 0 || this.sources.size > 0;
    if (active === this.playbackActive) return;
    this.playbackActive = active;
    this.playbackListener?.(active);
  }
}

export class Waveform {
  private values = Array.from({ length: 52 }, (_, index) => 0.05 + (index % 5) * 0.015);
  private frame = 0;
  private lastDrawAt = 0;
  private active = false;
  private running = true;

  constructor(private readonly canvas: HTMLCanvasElement) {
    window.addEventListener('resize', this.resize);
    this.resize();
    this.draw();
  }

  setActive(active: boolean) {
    this.active = active;
  }

  push(level: number) {
    this.canvas.dataset.audioLevel = level.toFixed(4);
    this.values.shift();
    this.values.push(Math.max(0.035, level));
  }

  destroy() {
    this.running = false;
    window.removeEventListener('resize', this.resize);
  }

  private resize = () => {
    const ratio = window.devicePixelRatio || 1;
    const bounds = this.canvas.getBoundingClientRect();
    this.canvas.width = Math.max(1, Math.round(bounds.width * ratio));
    this.canvas.height = Math.max(1, Math.round(bounds.height * ratio));
  };

  private draw = (timestamp = performance.now()) => {
    if (!this.running) return;
    if (timestamp - this.lastDrawAt < 50) {
      requestAnimationFrame(this.draw);
      return;
    }
    this.lastDrawAt = timestamp;
    const context = this.canvas.getContext('2d');
    if (!context) return;
    const width = this.canvas.width;
    const height = this.canvas.height;
    context.clearRect(0, 0, width, height);
    const ratio = window.devicePixelRatio || 1;
    const gap = 4 * ratio;
    const barWidth = Math.max(2 * ratio, (width - gap * (this.values.length - 1)) / this.values.length);
    this.frame += 0.035;
    this.values.forEach((raw, index) => {
      const idle = 0.055 + Math.sin(this.frame + index * 0.42) * 0.025;
      const value = this.active ? raw : idle;
      const barHeight = Math.max(3 * ratio, value * height * 0.84);
      const x = index * (barWidth + gap);
      const y = (height - barHeight) / 2;
      context.fillStyle = index > this.values.length * 0.68 ? '#e76f51' : '#257052';
      context.globalAlpha = this.active ? 0.9 : 0.28;
      context.beginPath();
      context.roundRect(x, y, barWidth, barHeight, barWidth / 2);
      context.fill();
    });
    context.globalAlpha = 1;
    requestAnimationFrame(this.draw);
  };
}

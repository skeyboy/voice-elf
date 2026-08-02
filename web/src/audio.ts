interface CaptureMessage {
  type: 'samples';
  payload?: ArrayBuffer;
}

type VadBoundary = 'speech_start' | 'speech_end';

interface VadMessage {
  type: 'ready' | 'pcm' | 'level' | 'speech_start' | 'speech_end' | 'flushed' | 'error';
  payload?: ArrayBuffer;
  message?: string;
  value?: number;
}

export class MicrophoneCapture {
  private context: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private node: AudioWorkletNode | null = null;
  private vadWorker: Worker | null = null;
  private vadReady = false;
  private onPcm: ((pcm: ArrayBuffer) => void) | null = null;
  private onLevel: ((level: number) => void) | null = null;
  private onBoundary: ((boundary: VadBoundary) => void) | null = null;
  private onFatalError: ((error: Error) => void) | null = null;
  private flushResolver: (() => void) | null = null;

  private async prepareVad(maxUtteranceSeconds: number, inputSampleRate: number) {
    this.destroyVad();
    const worker = new Worker('/vad-worker.js', { type: 'module', name: 'voice-elf-vad' });
    this.vadWorker = worker;
    const ready = new Promise<void>((resolve, reject) => {
      let initialized = false;
      const timeout = window.setTimeout(
        () => reject(new Error('浏览器音频 VAD 初始化超时')),
        5_000,
      );
      worker.onmessage = (event: MessageEvent<VadMessage>) => {
        const message = event.data;
        if (message.type === 'ready') {
          window.clearTimeout(timeout);
          initialized = true;
          this.vadReady = true;
          resolve();
        } else if (message.type === 'pcm' && message.payload) {
          this.onPcm?.(message.payload);
        } else if (message.type === 'level') {
          this.onLevel?.(message.value ?? 0);
        } else if (message.type === 'speech_start' || message.type === 'speech_end') {
          this.onBoundary?.(message.type);
        } else if (message.type === 'flushed') {
          this.flushResolver?.();
          this.flushResolver = null;
        } else if (message.type === 'error') {
          const error = new Error(message.message || '浏览器音频 VAD 运行失败');
          this.vadReady = false;
          window.clearTimeout(timeout);
          if (initialized) this.onFatalError?.(error);
          else reject(error);
        }
      };
      worker.onerror = () => {
        const error = new Error('浏览器音频 VAD Worker 运行失败');
        this.vadReady = false;
        window.clearTimeout(timeout);
        if (initialized) this.onFatalError?.(error);
        else reject(error);
      };
    });
    worker.postMessage({ type: 'init', maxUtteranceSeconds, inputSampleRate });
    await ready;
  }

  async start(
    maxUtteranceSeconds: number,
    onPcm: (pcm: ArrayBuffer) => void,
    onLevel: (level: number) => void,
    onBoundary: (boundary: VadBoundary) => void,
    onReady: () => void,
    onFatalError: (error: Error) => void,
  ) {
    if (this.context) return;
    if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia) {
      throw new Error('当前访问地址不是浏览器安全上下文；局域网麦克风测试需要使用受信任的 HTTPS 地址');
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: false,
      },
    });
    const context = new AudioContext({ latencyHint: 'interactive' });
    this.onLevel = onLevel;
    this.onFatalError = onFatalError;
    try {
      await Promise.all([
        context.audioWorklet.addModule('/audio-processor.js'),
        this.prepareVad(maxUtteranceSeconds, context.sampleRate),
      ]);
      await context.resume();
    } catch (error) {
      stream.getTracks().forEach((track) => track.stop());
      await context.close();
      this.destroyVad();
      this.onLevel = null;
      this.onFatalError = null;
      throw error;
    }
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, 'microphone-tap-processor');
    const silent = context.createGain();
    silent.gain.value = 0;
    source.connect(node).connect(silent).connect(context.destination);
    this.onPcm = onPcm;
    this.onBoundary = onBoundary;
    onReady();
    node.port.onmessage = (event: MessageEvent<CaptureMessage>) => {
      if (event.data.type === 'samples' && event.data.payload && this.vadReady && this.vadWorker) {
        this.vadWorker.postMessage(
          { type: 'samples', payload: event.data.payload },
          [event.data.payload],
        );
      }
    };
    this.stream = stream;
    this.context = context;
    this.node = node;
  }

  async stop() {
    this.node?.disconnect();
    if (this.node) this.node.port.onmessage = null;
    this.stream?.getTracks().forEach((track) => track.stop());
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
    this.stream = null;
    this.context = null;
    this.onPcm = null;
    this.onLevel = null;
    this.onBoundary = null;
    this.onFatalError = null;
    this.destroyVad();
  }

  private destroyVad() {
    this.vadWorker?.terminate();
    this.vadWorker = null;
    this.vadReady = false;
    this.flushResolver = null;
  }
}

export class PcmPlayer {
  private context: AudioContext | null = null;
  private nextStart = 0;
  private generation = 0;
  private sources = new Set<AudioBufferSourceNode>();
  muted = false;

  async unlock() {
    this.context ??= new AudioContext({ latencyHint: 'interactive' });
    if (this.context.state === 'suspended') await this.context.resume();
  }

  async enqueue(bytes: ArrayBuffer, sampleRate: number, onEnded?: () => void) {
    if (this.muted) return;
    await this.unlock();
    const context = this.context as AudioContext;
    const samples = new Int16Array(bytes);
    const buffer = context.createBuffer(1, samples.length, sampleRate);
    const channel = buffer.getChannelData(0);
    for (let index = 0; index < samples.length; index += 1) {
      channel[index] = samples[index] / 32768;
    }
    const source = context.createBufferSource();
    const generation = this.generation;
    source.buffer = buffer;
    source.connect(context.destination);
    this.sources.add(source);
    source.onended = () => {
      this.sources.delete(source);
      if (generation === this.generation) onEnded?.();
    };
    const startAt = Math.max(context.currentTime + 0.025, this.nextStart);
    source.start(startAt);
    this.nextStart = startAt + buffer.duration;
  }

  reset() {
    this.nextStart = this.context?.currentTime ?? 0;
  }

  stop() {
    this.generation += 1;
    this.sources.forEach((source) => source.stop());
    this.sources.clear();
    this.reset();
  }
}

export class Waveform {
  private values = Array.from({ length: 52 }, (_, index) => 0.05 + (index % 5) * 0.015);
  private frame = 0;
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

  private draw = () => {
    if (!this.running) return;
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

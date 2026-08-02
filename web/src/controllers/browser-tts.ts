import type { LatencyReport } from '../protocol';

interface TtsResult {
  pcm: ArrayBuffer;
  sampleRate: number;
  synthesisMs: number;
}

export interface SavedBrowserAudio {
  utterance_id: string;
  translated_audio_url: string;
  latency: LatencyReport;
  pcm: ArrayBuffer;
  sampleRate: number;
}

interface WorkerMessage {
  type: 'progress' | 'result' | 'error';
  id: string;
  value?: number;
  pcm?: ArrayBuffer;
  sampleRate?: number;
  synthesisMs?: number;
  message?: string;
}

export class BrowserTts {
  private worker: Worker | null = null;
  private workerVariant = '';
  private chain = Promise.resolve();
  private pending = new Map<string, {
    resolve: (result: TtsResult) => void;
    reject: (error: Error) => void;
    onProgress: (value: number) => void;
  }>();

  synthesizeAndSave(
    utteranceId: string,
    text: string,
    language: string,
    voice: string,
    onProgress: (value: number) => void,
  ): Promise<SavedBrowserAudio> {
    if (language === 'zh') {
      return Promise.reject(new Error('Supertonic 3 不支持中文译声，译文已保留'));
    }
    const task = this.chain.then(async () => {
      const result = await this.synthesize(utteranceId, text, language, voice, onProgress);
      onProgress(97);
      const query = new URLSearchParams({
        sample_rate: String(result.sampleRate),
        synthesis_ms: String(result.synthesisMs),
      });
      const response = await fetch(
        `/api/utterances/${encodeURIComponent(utteranceId)}/translated-audio?${query}`,
        {
          method: 'POST',
          credentials: 'include',
          headers: { 'content-type': 'application/octet-stream' },
          body: result.pcm,
        },
      );
      if (!response.ok) {
        const payload = (await response.json().catch(() => null)) as { error?: string } | null;
        throw new Error(payload?.error ?? `译声保存失败 (${response.status})`);
      }
      const saved = await response.json() as Omit<SavedBrowserAudio, 'pcm' | 'sampleRate'>;
      onProgress(100);
      return { ...saved, pcm: result.pcm, sampleRate: result.sampleRate };
    });
    this.chain = task.then(() => undefined, () => undefined);
    return task;
  }

  destroy() {
    this.worker?.terminate();
    this.worker = null;
    this.workerVariant = '';
    for (const pending of this.pending.values()) pending.reject(new Error('浏览器 TTS 已停止'));
    this.pending.clear();
  }

  private synthesize(
    id: string,
    text: string,
    language: string,
    voice: string,
    onProgress: (value: number) => void,
  ) {
    const variant = 'official-v3';
    if (this.worker && this.workerVariant !== variant) {
      this.worker.terminate();
      this.worker = null;
    }
    this.worker ??= this.createWorker();
    this.workerVariant = variant;
    return new Promise<TtsResult>((resolve, reject) => {
      this.pending.set(id, { resolve, reject, onProgress });
      this.worker!.postMessage({ type: 'synthesize', id, text, language, voice });
    });
  }

  private createWorker() {
    const worker = new Worker(new URL('./tts-worker.js', import.meta.url), {
      type: 'module',
      name: 'voice-elf-tts',
    });
    worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
      const message = event.data;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      if (message.type === 'progress') {
        pending.onProgress(message.value ?? 0);
      } else if (message.type === 'result' && message.pcm) {
        this.pending.delete(message.id);
        pending.resolve({
          pcm: message.pcm,
          sampleRate: message.sampleRate ?? 44_100,
          synthesisMs: message.synthesisMs ?? 0,
        });
      } else if (message.type === 'error') {
        this.pending.delete(message.id);
        pending.reject(new Error(message.message || '浏览器 TTS 生成失败'));
      }
    };
    worker.onerror = () => {
      for (const pending of this.pending.values()) pending.reject(new Error('浏览器 TTS Worker 运行失败'));
      this.pending.clear();
      worker.terminate();
      this.worker = null;
      this.workerVariant = '';
    };
    return worker;
  }
}

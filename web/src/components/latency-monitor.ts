import type { LatencyReport, PipelinePhase } from '../protocol';
import { refreshIcons } from './icons';

export class LatencyMonitor {
  readonly element: HTMLElement;
  private utteranceCount = 0;
  private totalAudioMs = 0;

  constructor() {
    this.element = document.createElement('aside');
    this.element.className = 'monitor-panel';
    this.element.innerHTML = `
      <div class="panel-heading compact">
        <div><span class="section-kicker"><i data-lucide="activity"></i> PIPELINE</span><h2>延迟监控</h2></div>
        <span class="backend-badge"><i data-lucide="server"></i> --</span>
      </div>
      <div class="total-latency"><div><span>端到端</span><strong>--</strong></div><span class="latency-unit">ms</span></div>
      <ol class="pipeline-list">
        <li data-stage="vad"><span class="stage-icon"><i data-lucide="activity"></i></span><div><strong>VAD</strong><small>语音端点</small></div><output>--</output></li>
        <li data-stage="stt"><span class="stage-icon"><i data-lucide="mic"></i></span><div><strong>STT</strong><small>Qwen ASR</small></div><output>--</output></li>
        <li data-stage="translation"><span class="stage-icon"><i data-lucide="sparkles"></i></span><div><strong>翻译</strong><small>Local LLM</small></div><output>--</output></li>
        <li data-stage="tts"><span class="stage-icon"><i data-lucide="volume-2"></i></span><div><strong>TTS</strong><small>Qwen3 TTS</small></div><output>--</output></li>
      </ol>
      <div class="session-stats">
        <div><i data-lucide="clock-3"></i><span>音频时长</span><strong class="audio-duration">0.0s</strong></div>
        <div><i data-lucide="zap"></i><span>已完成</span><strong class="utterance-count">0</strong></div>
      </div>
    `;
    refreshIcons(this.element);
  }

  setBackend(backend: string) {
    const badge = this.element.querySelector<HTMLElement>('.backend-badge')!;
    badge.innerHTML = `<i data-lucide="server"></i> ${backend.toUpperCase()}`;
    refreshIcons(badge);
  }

  setPhase(phase: PipelinePhase) {
    this.element.querySelectorAll('.pipeline-list li').forEach((item) => item.classList.remove('active'));
    const stage =
      phase === 'speech' || phase === 'listening'
        ? 'vad'
        : phase === 'transcribing'
          ? 'stt'
          : phase === 'translating'
            ? 'translation'
            : 'tts';
    this.element.querySelector(`[data-stage="${stage}"]`)?.classList.add('active');
  }

  addLatency(latency: LatencyReport) {
    const values: Record<string, number> = {
      vad: latency.vad_ms,
      stt: latency.stt_ms,
      translation: latency.translation_ms,
      tts: latency.tts_ms,
    };
    Object.entries(values).forEach(([stage, value]) => {
      const item = this.element.querySelector<HTMLElement>(`[data-stage="${stage}"]`);
      item?.classList.add('complete');
      const output = item?.querySelector('output');
      if (output) output.textContent = `${value} ms`;
    });
    this.element.querySelector('.total-latency strong')!.textContent = String(latency.total_ms);
    this.utteranceCount += 1;
    this.totalAudioMs += latency.audio_ms;
    this.element.querySelector('.utterance-count')!.textContent = String(this.utteranceCount);
    this.element.querySelector('.audio-duration')!.textContent = `${(this.totalAudioMs / 1000).toFixed(1)}s`;
  }

  reset() {
    this.utteranceCount = 0;
    this.totalAudioMs = 0;
    this.element.querySelector('.total-latency strong')!.textContent = '--';
    this.element.querySelector('.utterance-count')!.textContent = '0';
    this.element.querySelector('.audio-duration')!.textContent = '0.0s';
    this.element.querySelectorAll('.pipeline-list li').forEach((item) => {
      item.classList.remove('active', 'complete');
      item.querySelector('output')!.textContent = '--';
    });
  }
}

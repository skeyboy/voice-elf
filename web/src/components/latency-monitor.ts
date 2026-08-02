import type { LatencyReport, PipelinePhase } from '../protocol';
import { refreshIcons } from './icons';

let monitorSequence = 0;

export class LatencyMonitor {
  readonly element: HTMLElement;
  private readonly popover: HTMLElement;
  private readonly popoverId = `latency-popover-${++monitorSequence}`;
  private utteranceCount = 0;
  private totalAudioMs = 0;

  constructor() {
    this.element = document.createElement('div');
    this.element.className = 'latency-monitor';
    this.element.innerHTML = `
      <button class="latency-trigger" type="button" aria-expanded="false" aria-controls="${this.popoverId}" title="查看延迟监控">
        <i data-lucide="activity"></i>
        <span>延迟</span>
        <output class="latency-summary">-- ms</output>
        <i class="latency-chevron" data-lucide="chevron-down"></i>
      </button>
    `;

    this.popover = document.createElement('section');
    this.popover.id = this.popoverId;
    this.popover.className = 'latency-popover';
    this.popover.hidden = true;
    this.popover.setAttribute('aria-label', '延迟监控详情');
    this.popover.innerHTML = `
      <header class="latency-popover-heading">
        <div><span class="section-kicker"><i data-lucide="activity"></i> PIPELINE</span><h2>延迟监控</h2></div>
        <span class="backend-badge"><i data-lucide="server"></i> --</span>
      </header>
      <div class="total-latency"><div><span>端到端</span><strong>--</strong></div><span class="latency-unit">ms</span></div>
      <ol class="pipeline-list">
        <li data-stage="vad"><span class="stage-icon"><i data-lucide="activity"></i></span><div><strong>VAD</strong><small>语音端点</small></div><output>--</output></li>
        <li data-stage="stt"><span class="stage-icon"><i data-lucide="mic"></i></span><div><strong>STT</strong><small>Qwen ASR</small></div><output>--</output></li>
        <li data-stage="translation"><span class="stage-icon"><i data-lucide="sparkles"></i></span><div><strong>翻译</strong><small>Local LLM</small></div><output>--</output></li>
        <li data-stage="tts"><span class="stage-icon"><i data-lucide="volume-2"></i></span><div><strong>TTS</strong><small>Supertonic Web</small></div><output>--</output></li>
      </ol>
      <div class="session-stats">
        <div><i data-lucide="clock-3"></i><span>音频时长</span><strong class="audio-duration">0.0s</strong></div>
        <div><i data-lucide="zap"></i><span>已完成</span><strong class="utterance-count">0</strong></div>
      </div>
    `;
    document.body.append(this.popover);
    this.element.querySelector('button')!.addEventListener('click', this.togglePopover);
    document.addEventListener('pointerdown', this.handleOutsidePointer);
    document.addEventListener('keydown', this.handleKeydown);
    window.addEventListener('resize', this.closePopover);
    document.addEventListener('scroll', this.closePopover, true);
    refreshIcons(this.element);
    refreshIcons(this.popover);
  }

  setBackend(backend: string) {
    const badge = this.popover.querySelector<HTMLElement>('.backend-badge')!;
    badge.innerHTML = `<i data-lucide="server"></i> ${backend.toUpperCase()}`;
    refreshIcons(badge);
  }

  setPhase(phase: PipelinePhase) {
    this.popover.querySelectorAll('.pipeline-list li').forEach((item) => item.classList.remove('active'));
    const stage =
      phase === 'speech' || phase === 'listening'
        ? 'vad'
        : phase === 'transcribing'
          ? 'stt'
          : phase === 'translating'
            ? 'translation'
            : 'tts';
    this.popover.querySelector(`[data-stage="${stage}"]`)?.classList.add('active');
    this.element.dataset.stage = stage;
  }

  addLatency(latency: LatencyReport, countUtterance = true) {
    const values: Record<string, number> = {
      vad: latency.vad_ms,
      stt: latency.stt_ms,
      translation: latency.translation_ms,
      tts: latency.tts_ms,
    };
    Object.entries(values).forEach(([stage, value]) => {
      const item = this.popover.querySelector<HTMLElement>(`[data-stage="${stage}"]`);
      item?.classList.add('complete');
      const output = item?.querySelector('output');
      if (output) output.textContent = `${value} ms`;
    });
    this.popover.querySelector('.total-latency strong')!.textContent = String(latency.total_ms);
    this.element.querySelector('.latency-summary')!.textContent = `${latency.total_ms} ms`;
    if (countUtterance) {
      this.utteranceCount += 1;
      this.totalAudioMs += latency.audio_ms;
      this.popover.querySelector('.utterance-count')!.textContent = String(this.utteranceCount);
      this.popover.querySelector('.audio-duration')!.textContent = `${(this.totalAudioMs / 1000).toFixed(1)}s`;
    }
  }

  reset() {
    this.utteranceCount = 0;
    this.totalAudioMs = 0;
    this.element.querySelector('.latency-summary')!.textContent = '-- ms';
    this.popover.querySelector('.total-latency strong')!.textContent = '--';
    this.popover.querySelector('.utterance-count')!.textContent = '0';
    this.popover.querySelector('.audio-duration')!.textContent = '0.0s';
    this.popover.querySelectorAll('.pipeline-list li').forEach((item) => {
      item.classList.remove('active', 'complete');
      item.querySelector('output')!.textContent = '--';
    });
  }

  destroy() {
    this.element.querySelector('button')?.removeEventListener('click', this.togglePopover);
    document.removeEventListener('pointerdown', this.handleOutsidePointer);
    document.removeEventListener('keydown', this.handleKeydown);
    window.removeEventListener('resize', this.closePopover);
    document.removeEventListener('scroll', this.closePopover, true);
    this.popover.remove();
  }

  private togglePopover = () => {
    if (this.popover.hidden) this.openPopover();
    else this.closePopover();
  };

  private openPopover() {
    this.popover.hidden = false;
    const trigger = this.element.getBoundingClientRect();
    const panel = this.popover.getBoundingClientRect();
    const margin = 12;
    const left = Math.max(margin, Math.min(trigger.right - panel.width, window.innerWidth - panel.width - margin));
    let top = trigger.bottom + 8;
    if (top + panel.height > window.innerHeight - margin) {
      top = Math.max(margin, trigger.top - panel.height - 8);
    }
    this.popover.style.left = `${Math.round(left)}px`;
    this.popover.style.top = `${Math.round(top)}px`;
    this.element.querySelector('button')!.setAttribute('aria-expanded', 'true');
  }

  private closePopover = () => {
    if (this.popover.hidden) return;
    this.popover.hidden = true;
    this.element.querySelector('button')?.setAttribute('aria-expanded', 'false');
  };

  private handleOutsidePointer = (event: PointerEvent) => {
    const target = event.target as Node;
    if (!this.element.contains(target) && !this.popover.contains(target)) this.closePopover();
  };

  private handleKeydown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') this.closePopover();
  };
}

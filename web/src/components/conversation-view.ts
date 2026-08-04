import type { RoomDetail } from '../api';
import { PcmPlayer } from '../audio';
import type { ServerEvent } from '../protocol';
import { languageNames } from '../shared/languages';
import { refreshIcons } from './icons';

type TranscriptEvent = {
  utterance_id: string;
  text: string;
  language: string;
  created_at?: string;
};

export class ConversationView {
  readonly element: HTMLElement;
  private readonly list: HTMLElement;
  private readonly emptyState: HTMLElement;
  private readonly scrollButton: HTMLButtonElement;
  private readonly rows = new Map<string, HTMLElement>();
  private readonly mediaAudios = new Set<HTMLAudioElement>();
  private readonly mediaObjectUrls = new Set<string>();
  private activeMediaAudio: HTMLAudioElement | null = null;
  private autoFollow = true;
  private programmaticScroll = false;
  private scrollEndTimer = 0;

  constructor(
    private readonly player: PcmPlayer,
    private readonly onError: (message: string) => void,
    private readonly onMediaPlaybackChange: (active: boolean) => void = () => {},
  ) {
    this.element = document.createElement('div');
    this.element.className = 'conversation-scroll-region';
    this.element.innerHTML = `
      <div class="conversation-list empty">
        <div class="empty-state">
          <div class="empty-signal"><i data-lucide="languages"></i></div>
          <strong>等待语音</strong>
          <span>16 kHz · PCM16 · MONO</span>
        </div>
      </div>
      <button class="scroll-latest" type="button" hidden><i data-lucide="arrow-down"></i><span>最新记录</span></button>
    `;
    this.list = this.element.querySelector('.conversation-list')!;
    this.emptyState = this.element.querySelector('.empty-state')!;
    this.scrollButton = this.element.querySelector('.scroll-latest')!;
    this.list.addEventListener('scroll', () => this.handleScroll(), { passive: true });
    this.scrollButton.addEventListener('click', () => this.scrollToBottom(true));
    refreshIcons(this.element);
  }

  reset() {
    this.mediaAudios.forEach((audio) => audio.pause());
    this.mediaAudios.clear();
    this.mediaObjectUrls.forEach((url) => URL.revokeObjectURL(url));
    this.mediaObjectUrls.clear();
    this.activeMediaAudio = null;
    this.player.stop();
    this.rows.clear();
    this.list.replaceChildren(this.emptyState);
    this.list.classList.add('empty');
    this.autoFollow = true;
    this.programmaticScroll = false;
    window.clearTimeout(this.scrollEndTimer);
    this.scrollButton.hidden = true;
  }

  destroy() {
    this.reset();
  }

  renderHistory(detail: RoomDetail) {
    this.reset();
    detail.utterances
      .slice()
      .reverse()
      .forEach((utterance) => {
        const recognizing = utterance.status === 'recognizing';
        this.upsertTranscript(
          {
            utterance_id: utterance.id,
            text: utterance.source_text,
            language: utterance.source_language,
            created_at: utterance.created_at,
          },
          recognizing,
        );
        if (utterance.status === 'recognition_failed') {
          this.markRecognitionFailed(utterance.id, '未识别到清晰语音，已保留原声');
        } else if (utterance.status === 'recognition_interrupted') {
          this.markRecognitionFailed(utterance.id, '识别因连接中断而停止，原声已保留');
        } else {
          if (utterance.translated_text) {
            this.applyTranslation({
              type: 'translation',
              utterance_id: utterance.id,
              source_text: utterance.source_text,
              translated_text: utterance.translated_text,
              source_language: utterance.source_language,
              target_language: utterance.target_language,
            });
          }
          if (['translation_failed', 'translation_interrupted'].includes(utterance.status)) {
            this.markProcessingFailed(utterance.id, 'translation', '翻译未完成，原文和原声已保留');
          } else if (['tts_failed', 'tts_interrupted'].includes(utterance.status)) {
            this.markProcessingFailed(utterance.id, 'tts', '译声生成未完成，原文和译文已保留');
          } else if (utterance.status === 'text_ready') {
            this.setItemStatus(utterance.id, '等待生成译声');
          }
        }
        if (utterance.source_audio_url || utterance.translated_audio_url) {
          this.applyMedia({
            type: 'media',
            utterance_id: utterance.id,
            source_audio_url: utterance.source_audio_url,
            translated_audio_url: utterance.translated_audio_url,
          });
        }
        if (utterance.status === 'completed') {
          this.updateItemLatency(utterance.id, utterance.latency.total_ms);
        }
      });
    requestAnimationFrame(() => this.scrollToBottom(true));
  }

  upsertTranscript(event: TranscriptEvent, streaming: boolean) {
    const article = this.ensureTranscriptRow(event);
    this.setSourceText(article, event.text, streaming);
    const pendingLabel = article.querySelector<HTMLElement>('.translation-line.pending small');
    const pendingText = article.querySelector<HTMLElement>('.translation-line.pending .translation-text');
    if (!streaming && event.text && pendingLabel && pendingText) {
      pendingLabel.textContent = 'TRANSLATION';
      pendingText.textContent = '正在翻译';
    }
    this.followIfEnabled();
  }

  removeUtterance(id: string) {
    const article = this.rows.get(id);
    if (!article) return;
    article.querySelectorAll<HTMLAudioElement>('audio').forEach((audio) => {
      audio.pause();
      this.mediaAudios.delete(audio);
    });
    article.remove();
    this.rows.delete(id);
    if (this.rows.size === 0) {
      this.list.replaceChildren(this.emptyState);
      this.list.classList.add('empty');
    }
  }

  private ensureTranscriptRow(event: TranscriptEvent) {
    let article = this.rows.get(event.utterance_id);
    if (!article) {
      this.emptyState.remove();
      this.list.classList.remove('empty');
      article = document.createElement('article');
      article.className = 'conversation-item';
      article.dataset.id = event.utterance_id;
      article.innerHTML = `
        <div class="utterance-meta">
          <span class="source-language">${languageNames[event.language] ?? event.language}</span>
          <time>${formatTimestamp(event.created_at)}</time>
        </div>
        <div class="source-block">
          <p class="source-text" aria-live="polite">
            <span class="source-content"></span>
            <span class="recognition-status" role="status" hidden><i data-lucide="loader-circle"></i><span>识别中</span></span>
          </p>
          <div class="source-media media-slot" aria-label="原声音频"></div>
        </div>
        <div class="translation-line pending">
          <span class="direction-mark"><i data-lucide="sparkles"></i></span>
          <div class="translation-body">
            <small>TRANSLATION</small>
            <p class="translation-text" data-stream-text="">等待原文完成</p>
            <div class="translated-media media-slot" aria-label="译声音频"></div>
          </div>
        </div>
        <span class="item-latency">处理中</span>
      `;
      this.list.append(article);
      this.rows.set(event.utterance_id, article);
      refreshIcons(article);
    }
    article.querySelector<HTMLElement>('.source-language')!.textContent =
      languageNames[event.language] ?? event.language;
    return article;
  }

  applyTranscriptDelta(event: Extract<ServerEvent, { type: 'transcript_delta' }>) {
    const article = this.ensureTranscriptRow(event);
    const content = article.querySelector<HTMLElement>('.source-content')!;
    if (event.done) {
      this.setSourceText(article, event.text, false);
    } else {
      this.appendStreamingText(content, event.text);
      this.setSourceStreamingState(article, true);
      const translation = article.querySelector<HTMLElement>('.translation-line.pending .translation-text');
      if (translation && event.text && !translation.dataset.streamText) {
        translation.textContent = '正在实时翻译';
      }
    }
    this.followIfEnabled();
  }

  markRecognitionFailed(id: string, message: string) {
    const article = this.rows.get(id);
    if (!article) return;
    article.classList.remove('transcript-streaming');
    article.classList.add('recognition-failed');
    this.setSourceText(article, message, false);
    const line = article.querySelector<HTMLElement>('.translation-line');
    if (line) {
      line.classList.add('pending');
      line.querySelector('.direction-mark')!.innerHTML = '<i data-lucide="circle-alert"></i>';
      line.querySelector('small')!.textContent = 'ASR';
      line.querySelector<HTMLElement>('.translation-text')!.textContent = '本条未进入翻译';
      refreshIcons(line);
    }
    const latency = article.querySelector('.item-latency');
    if (latency) latency.textContent = '识别未完成';
    this.followIfEnabled();
  }

  markProcessingFailed(id: string, stage: 'translation' | 'tts', message: string) {
    const article = this.rows.get(id);
    if (!article) return;
    article.classList.remove('transcript-streaming');
    article.classList.add('processing-failed');
    if (stage === 'translation') {
      const line = article.querySelector<HTMLElement>('.translation-line');
      if (line) {
        line.classList.add('pending');
        line.querySelector('.direction-mark')!.innerHTML = '<i data-lucide="circle-alert"></i>';
        line.querySelector('small')!.textContent = 'TRANSLATION';
        line.querySelector<HTMLElement>('.translation-text')!.textContent = message;
        refreshIcons(line);
      }
    } else {
      const status = article.querySelector<HTMLElement>('.tts-generation');
      if (status) {
        status.classList.add('failed');
        status.innerHTML = `<i data-lucide="circle-alert"></i><span>${message}</span>`;
        refreshIcons(status);
      }
    }
    this.setItemStatus(id, stage === 'translation' ? '翻译未完成' : '译声未完成');
    this.followIfEnabled();
  }

  applyTranslationDelta(event: Extract<ServerEvent, { type: 'translation_delta' }>) {
    const article = this.rows.get(event.utterance_id);
    if (!article) return;
    const line = article.querySelector<HTMLElement>('.translation-line');
    if (!line) return;
    line.classList.toggle('pending', !event.done);
    line.classList.toggle('translation-streaming', !event.done);
    line.querySelector('small')!.textContent =
      languageNames[event.target_language] ?? event.target_language;
    const text = line.querySelector<HTMLElement>('.translation-text')!;
    if (!event.done) {
      this.appendStreamingText(text, event.text);
    } else {
      text.textContent = event.text;
      text.dataset.streamText = event.text;
    }
    refreshIcons(line);
    if (event.done) {
      const itemLatency = this.rows.get(event.utterance_id)?.querySelector('.item-latency');
      if (itemLatency?.textContent === '处理中') itemLatency.textContent = '文字完成';
    }
    this.followIfEnabled();
  }

  applyTranslation(event: Extract<ServerEvent, { type: 'translation' }>) {
    if (!this.rows.has(event.utterance_id)) {
      this.upsertTranscript(
        {
          utterance_id: event.utterance_id,
          text: event.source_text,
          language: event.source_language,
        },
        false,
      );
    }
    const line = this.rows.get(event.utterance_id)?.querySelector<HTMLElement>('.translation-line');
    if (!line) return;
    line.classList.remove('pending');
    line.classList.remove('translation-streaming');
    line.querySelector('small')!.textContent =
      languageNames[event.target_language] ?? event.target_language;
    const text = line.querySelector<HTMLElement>('.translation-text')!;
    text.textContent = event.translated_text;
    text.dataset.streamText = event.translated_text;
    refreshIcons(line);
    this.followIfEnabled();
  }

  applyMedia(event: Extract<ServerEvent, { type: 'media' }>) {
    const article = this.rows.get(event.utterance_id);
    if (!article) return;
    const sourceContainer = article.querySelector<HTMLElement>('.source-media')!;
    const translatedContainer = article.querySelector<HTMLElement>('.translated-media')!;
    const source = sourceContainer.querySelector<HTMLElement>('[data-media-kind="source"]');
    const translated = translatedContainer.querySelector<HTMLElement>('[data-media-kind="translated"]');
    if (event.source_audio_url && !source) {
      sourceContainer.append(
        this.createMediaPlayer(event.source_audio_url, '原声', event.utterance_id, false),
      );
    }
    if (event.translated_audio_url && !translated) {
      translatedContainer.querySelector('.tts-generation')?.remove();
      translatedContainer.append(
        this.createMediaPlayer(event.translated_audio_url, '译声', event.utterance_id, true),
      );
    }
    refreshIcons(article);
    this.followIfEnabled();
  }

  markTtsGenerating(id: string) {
    const container = this.rows.get(id)?.querySelector<HTMLElement>('.translated-media');
    if (!container) return;
    let status = container.querySelector<HTMLElement>('.tts-generation');
    if (!status) {
      status = document.createElement('div');
      status.className = 'tts-generation';
      status.innerHTML = '<i data-lucide="audio-lines"></i><span>服务端生成译声</span>';
      container.append(status);
      refreshIcons(status);
    }
    this.setItemStatus(id, '服务端生成译声');
    this.followIfEnabled();
  }

  updateTranslatedProgress(id: string, current: number, duration: number) {
    const control = this.rows.get(id)?.querySelector<HTMLElement>('.translated-audio');
    const progress = control?.querySelector<HTMLInputElement>('.audio-progress');
    const time = control?.querySelector<HTMLOutputElement>('.audio-time');
    if (!progress || !time || !Number.isFinite(duration) || duration <= 0) return;
    progress.max = String(duration);
    progress.value = String(Math.min(current, duration));
    time.textContent = `${formatMediaTime(current)} / ${formatMediaTime(duration)}`;
  }

  updateItemLatency(id: string, totalMs: number) {
    const itemLatency = this.rows.get(id)?.querySelector('.item-latency');
    if (itemLatency) itemLatency.textContent = `${totalMs} ms`;
  }

  private setItemStatus(id: string, status: string) {
    const itemLatency = this.rows.get(id)?.querySelector('.item-latency');
    if (itemLatency) itemLatency.textContent = status;
  }

  private handleScroll() {
    const distance = this.list.scrollHeight - this.list.scrollTop - this.list.clientHeight;
    if (this.programmaticScroll) {
      this.scrollButton.hidden = true;
      window.clearTimeout(this.scrollEndTimer);
      this.scrollEndTimer = window.setTimeout(() => {
        this.programmaticScroll = false;
        const remaining = this.list.scrollHeight - this.list.scrollTop - this.list.clientHeight;
        this.autoFollow = remaining <= 56;
        this.scrollButton.hidden = this.autoFollow;
      }, 140);
      return;
    }
    this.autoFollow = distance <= 56;
    this.scrollButton.hidden = this.autoFollow;
  }

  private followIfEnabled() {
    if (!this.autoFollow) {
      this.scrollButton.hidden = false;
      return;
    }
    requestAnimationFrame(() => this.scrollToBottom(false));
  }

  private scrollToBottom(smooth: boolean) {
    this.autoFollow = true;
    this.programmaticScroll = smooth;
    this.scrollButton.hidden = true;
    this.list.scrollTo({ top: this.list.scrollHeight, behavior: smooth ? 'smooth' : 'auto' });
  }

  private createMediaPlayer(url: string, label: string, utteranceId: string, translated: boolean) {
    const wrapper = document.createElement('div');
    wrapper.className = `media-player${translated ? ' translated-audio' : ''}`;
    wrapper.dataset.mediaKind = translated ? 'translated' : 'source';
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'media-play-button';
    const audio = document.createElement('audio');
    audio.className = 'media-audio';
    audio.preload = 'metadata';
    audio.setAttribute('playsinline', '');
    wrapper.append(button, audio);
    this.mediaAudios.add(audio);
    this.renderMediaButton(button, label, false);

    if (translated) {
      const progress = document.createElement('input');
      progress.className = 'audio-progress';
      progress.type = 'range';
      progress.min = '0';
      progress.max = '1';
      progress.step = '0.01';
      progress.value = '0';
      progress.setAttribute('aria-label', '译声播放进度');
      const time = document.createElement('output');
      time.className = 'audio-time';
      time.textContent = '0:00 / --:--';
      wrapper.append(progress, time);
      audio.addEventListener('loadedmetadata', () =>
        this.updateTranslatedProgress(utteranceId, audio.currentTime, audio.duration),
      );
      audio.addEventListener('timeupdate', () =>
        this.updateTranslatedProgress(utteranceId, audio.currentTime, audio.duration),
      );
      progress.addEventListener('input', () => {
        if (Number.isFinite(audio.duration)) audio.currentTime = Number(progress.value);
      });
    }

    button.addEventListener('click', async () => {
      if (audio.paused) {
        this.player.stop();
        await this.player.unlock();
        if (this.activeMediaAudio && this.activeMediaAudio !== audio) this.activeMediaAudio.pause();
        this.activeMediaAudio = audio;
        try {
          if (!audio.src) {
            button.disabled = true;
            button.innerHTML = `<i data-lucide="loader-circle"></i><span>加载${label}</span>`;
            refreshIcons(button);
            const response = await fetch(url, {
              credentials: 'include',
              cache: 'no-store',
            });
            if (!response.ok) throw new Error(`音频请求失败 (${response.status})`);
            const blob = await response.blob();
            if (!blob.size) throw new Error('音频文件为空');
            const objectUrl = URL.createObjectURL(blob);
            this.mediaObjectUrls.add(objectUrl);
            audio.src = objectUrl;
            audio.load();
          }
          this.onMediaPlaybackChange(true);
          await audio.play();
        } catch (error) {
          this.activeMediaAudio = null;
          this.onMediaPlaybackChange(false);
          this.onError(
            error instanceof Error ? `无法播放${label}：${error.message}` : `无法播放${label}`,
          );
        } finally {
          button.disabled = false;
          if (audio.paused) this.renderMediaButton(button, label, false);
        }
      } else {
        audio.pause();
      }
    });
    audio.addEventListener('error', () => {
      this.onMediaPlaybackChange(false);
      const status = audio.error?.code;
      this.onError(`无法加载${label}${status ? `（媒体错误 ${status}）` : ''}`);
    });
    audio.addEventListener('play', () => this.renderMediaButton(button, label, true));
    audio.addEventListener('pause', () => {
      this.onMediaPlaybackChange(false);
      this.renderMediaButton(button, label, false);
    });
    audio.addEventListener('ended', () => {
      this.onMediaPlaybackChange(false);
      this.renderMediaButton(button, label, false);
      if (this.activeMediaAudio === audio) this.activeMediaAudio = null;
    });
    return wrapper;
  }

  private liveText(value: string) {
    const span = document.createElement('span');
    span.className = 'live-text';
    span.textContent = value;
    window.setTimeout(() => {
      if (span.isConnected) span.replaceWith(document.createTextNode(span.textContent ?? ''));
    }, 220);
    return span;
  }

  private setSourceText(article: HTMLElement, value: string, streaming: boolean) {
    const content = article.querySelector<HTMLElement>('.source-content')!;
    content.textContent = value;
    content.dataset.streamText = value;
    this.setSourceStreamingState(article, streaming);
  }

  private setSourceStreamingState(article: HTMLElement, streaming: boolean) {
    const content = article.querySelector<HTMLElement>('.source-content')!;
    const status = article.querySelector<HTMLElement>('.recognition-status')!;
    article.classList.toggle('transcript-streaming', streaming);
    article.querySelector('.source-text')!.classList.toggle('source-pending', !content.dataset.streamText);
    status.hidden = !streaming;
  }

  private appendStreamingText(container: HTMLElement, nextText: string) {
    const currentText = container.dataset.streamText ?? '';
    if (!nextText.startsWith(currentText)) {
      container.textContent = nextText;
      container.dataset.streamText = nextText;
      return;
    }
    const delta = nextText.slice(currentText.length);
    if (!delta) return;
    if (!currentText) container.replaceChildren();
    container.append(this.liveText(delta));
    container.dataset.streamText = nextText;
  }

  private renderMediaButton(button: HTMLButtonElement, label: string, playing: boolean) {
    button.innerHTML = `<i data-lucide="${playing ? 'pause' : 'play'}"></i><span>${playing ? '暂停' : '播放'}${label}</span>`;
    button.title = `${playing ? '暂停' : '播放'}${label}`;
    button.setAttribute('aria-label', button.title);
    refreshIcons(button);
  }
}

function formatTimestamp(value?: string) {
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(value ? new Date(value) : new Date());
}

function formatMediaTime(seconds: number) {
  const safe = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, '0')}`;
}

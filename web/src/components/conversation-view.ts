import type { RoomDetail, SpeakerIdentity, UtteranceHistory } from '../api';
import { PcmPlayer } from '../audio';
import type { ServerEvent } from '../protocol';
import { languageNames } from '../shared/languages';
import { refreshIcons } from './icons';

type TranscriptEvent = {
  utterance_id: string;
  text: string;
  language: string;
  created_at?: string;
  speakers?: SpeakerIdentity[];
};

interface StreamingTextState {
  target: string;
  done: boolean;
  onStreamingChange: (streaming: boolean) => void;
}

const TYPEWRITER_INTERVAL_MS = 28;

export class ConversationView {
  readonly element: HTMLElement;
  private readonly list: HTMLElement;
  private readonly emptyState: HTMLElement;
  private readonly scrollButton: HTMLButtonElement;
  private readonly historyButton: HTMLButtonElement;
  private readonly speakerDialog: HTMLDialogElement;
  private readonly rows = new Map<string, HTMLElement>();
  private readonly speakers = new Map<string, SpeakerIdentity[]>();
  private readonly mediaAudios = new Set<HTMLAudioElement>();
  private readonly mediaObjectUrls = new Set<string>();
  private readonly streamingText = new Map<HTMLElement, StreamingTextState>();
  private activeMediaAudio: HTMLAudioElement | null = null;
  private typewriterFrame = 0;
  private typewriterLastTick = 0;
  private autoFollow = true;
  private programmaticScroll = false;
  private scrollEndTimer = 0;
  private mergingHistory = false;
  private historyLoading = false;
  private historyHasMore = false;
  private historyPullArmed = false;
  private historyPointerStartY: number | null = null;

  constructor(
    private readonly player: PcmPlayer,
    private readonly onError: (message: string) => void,
    private readonly onMediaPlaybackChange: (active: boolean) => void = () => {},
    private readonly onLoadOlder: () => void = () => {},
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
      <button class="history-older" type="button" hidden><i data-lucide="clock-3"></i><span>加载更早记录</span></button>
      <button class="scroll-latest" type="button" hidden><i data-lucide="arrow-down"></i><span>最新记录</span></button>
      <dialog class="speaker-dialog">
        <div class="speaker-dialog-heading"><div><small>本条音频</small><strong>发言成员</strong></div><button class="icon-button speaker-dialog-close" type="button" aria-label="关闭" title="关闭"><i data-lucide="x"></i></button></div>
        <div class="speaker-dialog-list"></div>
      </dialog>
    `;
    this.list = this.element.querySelector('.conversation-list')!;
    this.emptyState = this.element.querySelector('.empty-state')!;
    this.scrollButton = this.element.querySelector('.scroll-latest')!;
    this.historyButton = this.element.querySelector('.history-older')!;
    this.speakerDialog = this.element.querySelector('.speaker-dialog')!;
    this.list.addEventListener('scroll', () => this.handleScroll(), { passive: true });
    this.list.addEventListener('pointerdown', (event) => {
      this.historyPullArmed = true;
      this.historyPointerStartY = event.clientY;
    });
    this.list.addEventListener('pointermove', (event) => {
      if (
        this.historyPointerStartY !== null
        && event.clientY - this.historyPointerStartY >= 28
        && this.list.scrollTop <= 32
      ) {
        this.historyPointerStartY = event.clientY;
        this.historyPullArmed = false;
        this.requestOlderHistory();
      }
    }, { passive: true });
    this.list.addEventListener('pointerup', () => {
      this.historyPointerStartY = null;
    });
    this.list.addEventListener('wheel', (event) => {
      if (event.deltaY < 0) {
        this.historyPullArmed = true;
        if (this.list.scrollTop <= 32) this.requestOlderHistory();
      }
    }, { passive: true });
    this.scrollButton.addEventListener('click', () => this.scrollToBottom(true));
    this.historyButton.addEventListener('click', () => this.requestOlderHistory());
    this.speakerDialog.querySelector('.speaker-dialog-close')?.addEventListener('click', () =>
      this.speakerDialog.close(),
    );
    this.speakerDialog.addEventListener('click', (event) => {
      if (event.target === this.speakerDialog) this.speakerDialog.close();
    });
    refreshIcons(this.element);
  }

  reset() {
    if (this.speakerDialog.open) this.speakerDialog.close();
    this.mediaAudios.forEach((audio) => audio.pause());
    this.mediaAudios.clear();
    this.mediaObjectUrls.forEach((url) => URL.revokeObjectURL(url));
    this.mediaObjectUrls.clear();
    this.activeMediaAudio = null;
    this.player.stop();
    window.cancelAnimationFrame(this.typewriterFrame);
    this.typewriterFrame = 0;
    this.typewriterLastTick = 0;
    this.streamingText.clear();
    this.rows.clear();
    this.speakers.clear();
    this.list.replaceChildren(this.emptyState);
    this.list.classList.add('empty');
    this.autoFollow = true;
    this.programmaticScroll = false;
    window.clearTimeout(this.scrollEndTimer);
    this.scrollButton.hidden = true;
    this.historyButton.hidden = true;
    this.historyButton.disabled = false;
    this.mergingHistory = false;
    this.historyLoading = false;
    this.historyHasMore = false;
    this.historyPullArmed = false;
    this.historyPointerStartY = null;
  }

  destroy() {
    this.reset();
  }

  renderHistory(detail: RoomDetail) {
    this.renderUtterances(detail.utterances);
  }

  renderUtterances(utterances: UtteranceHistory[]) {
    this.reset();
    this.mergingHistory = true;
    this.mergeUtterances(utterances);
    this.mergingHistory = false;
    requestAnimationFrame(() => this.scrollToBottom(false));
  }

  prependHistory(utterances: UtteranceHistory[]) {
    if (utterances.length === 0) return;
    const existing = new Set(this.rows.keys());
    const anchor = this.list.querySelector('.conversation-item');
    const previousHeight = this.list.scrollHeight;
    const wasFollowing = this.autoFollow;
    this.mergingHistory = true;
    this.mergeUtterances(utterances);
    this.mergingHistory = false;
    const fragment = document.createDocumentFragment();
    utterances
      .slice()
      .reverse()
      .forEach((utterance) => {
        if (!existing.has(utterance.id)) {
          const row = this.rows.get(utterance.id);
          if (row) fragment.append(row);
        }
      });
    this.list.insertBefore(fragment, anchor);
    this.autoFollow = wasFollowing;
    this.list.scrollTop += this.list.scrollHeight - previousHeight;
  }

  setHistoryPagination(loaded: number, total: number, loading: boolean) {
    const hasMore = loaded < total;
    this.historyLoading = loading;
    this.historyHasMore = hasMore;
    this.historyButton.hidden = !loading && !hasMore;
    this.historyButton.disabled = loading;
    this.historyButton.classList.toggle('loading', loading);
    const label = this.historyButton.querySelector('span');
    if (label) {
      label.textContent = loading
        ? '正在加载记录'
        : `加载更早记录 · ${loaded}/${total}`;
    }
    this.historyButton.innerHTML = `<i data-lucide="${loading ? 'loader-circle' : 'clock-3'}"></i><span>${label?.textContent ?? ''}</span>`;
    refreshIcons(this.historyButton);
  }

  mergeHistory(detail: RoomDetail) {
    this.mergeUtterances(detail.utterances);
  }

  private mergeUtterances(utterances: UtteranceHistory[]) {
    utterances
      .slice()
      .reverse()
      .forEach((utterance) => {
        const recognizing = utterance.status === 'recognizing';
        if (utterance.source_text || !this.rows.has(utterance.id)) {
          this.upsertTranscript(
            {
              utterance_id: utterance.id,
              text: utterance.source_text,
              language: utterance.source_language,
              created_at: utterance.created_at,
              speakers: utterance.speakers,
            },
            recognizing,
          );
        }
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
        (utterance.refinements ?? []).forEach((refinement) => {
          this.applyRefinement({
            type: 'transcript_refinement',
            utterance_id: utterance.id,
            engine: refinement.engine,
            status: refinement.status === 'interrupted' ? 'failed' : refinement.status,
            text: refinement.text || null,
            language: refinement.language || null,
            segments: refinement.segments,
            message: refinement.processing_error,
          });
        });
      });
  }

  upsertTranscript(event: TranscriptEvent, streaming: boolean) {
    const article = this.ensureTranscriptRow(event);
    if (event.speakers) this.applySpeakers(event.utterance_id, event.speakers);
    this.setSourceText(article, event.text, streaming);
    const pendingLabel = article.querySelector<HTMLElement>('.translation-line.pending small');
    const pendingText = article.querySelector<HTMLElement>('.translation-line.pending .translation-text');
    if (
      !streaming &&
      event.text &&
      pendingLabel &&
      pendingText &&
      !pendingText.dataset.streamText &&
      !this.streamingText.has(pendingText)
    ) {
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
    article.querySelectorAll<HTMLElement>('[data-stream-text]').forEach((element) =>
      this.streamingText.delete(element),
    );
    article.remove();
    this.rows.delete(id);
    this.speakers.delete(id);
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
          <span class="speaker-slot"></span>
          <time>${formatTimestamp(event.created_at)}</time>
        </div>
        <div class="source-block">
          <p class="source-text" aria-live="polite">
            <span class="source-content" data-stream-text=""></span>
            <span class="source-media media-slot" aria-label="原声音频"></span>
            <span class="recognition-status" role="status" hidden><i data-lucide="loader-circle"></i><span>识别中</span></span>
          </p>
          <div class="refinement-slot" aria-live="polite"></div>
        </div>
        <div class="translation-line pending">
          <span class="direction-mark"><i data-lucide="sparkles"></i></span>
          <div class="translation-body">
            <small>TRANSLATION</small>
            <p class="translation-copy">
              <span class="translation-text" data-stream-text="">等待原文完成</span>
              <span class="translated-media media-slot" aria-label="译声音频"></span>
            </p>
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
    const speakers = event.speakers ?? this.speakers.get(event.utterance_id);
    if (speakers) this.renderSpeakers(article, speakers);
    return article;
  }

  applySpeakers(utteranceId: string, speakers: SpeakerIdentity[]) {
    this.speakers.set(utteranceId, speakers);
    const article = this.rows.get(utteranceId);
    if (article) this.renderSpeakers(article, speakers);
  }

  private renderSpeakers(article: HTMLElement, speakers: SpeakerIdentity[]) {
    const slot = article.querySelector<HTMLElement>('.speaker-slot');
    if (!slot) return;
    slot.replaceChildren();
    if (speakers.length === 0) return;
    if (speakers.length === 1) {
      const badge = document.createElement('span');
      badge.className = 'speaker-badge';
      badge.innerHTML = '<i data-lucide="user-round"></i><span></span>';
      badge.querySelector('span')!.textContent = speakers[0].username;
      slot.append(badge);
    } else {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'speaker-badge multiple';
      button.innerHTML = '<i data-lucide="users"></i><span></span>';
      button.querySelector('span')!.textContent = `多人发言 · ${speakers.length}`;
      button.addEventListener('click', () => this.openSpeakerDialog(speakers));
      slot.append(button);
    }
    refreshIcons(slot);
  }

  private openSpeakerDialog(speakers: SpeakerIdentity[]) {
    const list = this.speakerDialog.querySelector<HTMLElement>('.speaker-dialog-list')!;
    list.replaceChildren(
      ...speakers.map((speaker) => {
        const row = document.createElement('div');
        row.className = 'speaker-dialog-member';
        const avatar = document.createElement('span');
        avatar.className = 'speaker-dialog-avatar';
        avatar.textContent = speaker.username.slice(0, 1).toUpperCase();
        const name = document.createElement('strong');
        name.textContent = speaker.username;
        row.append(avatar, name);
        return row;
      }),
    );
    if (!this.speakerDialog.open) this.speakerDialog.showModal();
  }

  applyTranscriptDelta(event: Extract<ServerEvent, { type: 'transcript_delta' }>) {
    const article = this.ensureTranscriptRow(event);
    this.setSourceText(article, event.text, !event.done);
    if (!event.done) {
      const translation = article.querySelector<HTMLElement>('.translation-line.pending .translation-text');
      if (translation && event.text && !translation.dataset.streamText) {
        translation.textContent = '正在实时翻译';
      }
    }
    this.followIfEnabled();
  }

  applyRefinement(event: Extract<ServerEvent, { type: 'transcript_refinement' }>) {
    const article = this.rows.get(event.utterance_id);
    const slot = article?.querySelector<HTMLElement>('.refinement-slot');
    if (!article || !slot) return;
    let item = Array.from(slot.querySelectorAll<HTMLElement>('.refinement-result')).find(
      (candidate) => candidate.dataset.engine === event.engine,
    );
    if (!item) {
      item = document.createElement('div');
      item.className = 'refinement-result';
      item.dataset.engine = event.engine;
      item.innerHTML = `
        <span class="refinement-icon"><i data-lucide="scan-text"></i></span>
        <div><small></small><p></p></div>
      `;
      slot.append(item);
    }
    item.classList.toggle('processing', event.status === 'processing');
    item.classList.toggle('failed', event.status === 'failed');
    item.querySelector('small')!.textContent = `会后精识别 · ${refinementEngineLabel(event.engine)}`;
    item.querySelector('p')!.textContent =
      event.status === 'processing'
        ? '处理中'
        : event.status === 'failed'
          ? event.message || '精识别未完成，已保留实时识别结果'
          : event.text || '未返回文字';
    const icon = item.querySelector<HTMLElement>('.refinement-icon')!;
    icon.innerHTML = `<i data-lucide="${event.status === 'processing' ? 'loader-circle' : event.status === 'failed' ? 'circle-alert' : 'badge-check'}"></i>`;
    refreshIcons(item);
    this.followIfEnabled();
  }

  markRecognitionFailed(id: string, message: string) {
    const article = this.rows.get(id);
    if (!article) return;
    article.classList.remove('transcript-streaming');
    article.classList.add('recognition-failed');
    this.setSourceText(article, message, false, false);
    const line = article.querySelector<HTMLElement>('.translation-line');
    if (line) {
      line.classList.add('pending');
      line.querySelector('.direction-mark')!.innerHTML = '<i data-lucide="circle-alert"></i>';
      line.querySelector('small')!.textContent = 'ASR';
      this.commitText(line.querySelector<HTMLElement>('.translation-text')!, '本条未进入翻译');
      this.setTranslationStreamingState(line, false);
      line.classList.add('pending');
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
        this.commitText(line.querySelector<HTMLElement>('.translation-text')!, message);
        this.setTranslationStreamingState(line, false);
        line.classList.add('pending');
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
    line.querySelector('small')!.textContent =
      languageNames[event.target_language] ?? event.target_language;
    const text = line.querySelector<HTMLElement>('.translation-text')!;
    this.queueStreamingText(text, event.text, event.done, (streaming) =>
      this.setTranslationStreamingState(line, streaming),
    );
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
    line.querySelector('small')!.textContent =
      languageNames[event.target_language] ?? event.target_language;
    const text = line.querySelector<HTMLElement>('.translation-text')!;
    if (this.streamingText.has(text) || line.classList.contains('translation-streaming')) {
      this.queueStreamingText(text, event.translated_text, true, (streaming) =>
        this.setTranslationStreamingState(line, streaming),
      );
    } else {
      this.commitText(text, event.translated_text);
      this.setTranslationStreamingState(line, false);
    }
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
        this.createMediaPlayer(event.source_audio_url, '原声', false),
      );
    }
    if (event.translated_audio_url && !translated) {
      translatedContainer.querySelector('.tts-generation')?.remove();
      translatedContainer.append(
        this.createMediaPlayer(event.translated_audio_url, '译声', true),
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
      status = document.createElement('span');
      status.className = 'tts-generation';
      status.innerHTML = '<i data-lucide="audio-lines"></i><span>服务端生成译声</span>';
      container.append(status);
      refreshIcons(status);
    }
    this.setItemStatus(id, '服务端生成译声');
    this.followIfEnabled();
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
    if (this.list.scrollTop <= 32 && this.historyPullArmed) {
      this.historyPullArmed = false;
      this.requestOlderHistory();
    }
  }

  private requestOlderHistory() {
    if (this.historyLoading || !this.historyHasMore) return;
    this.historyPullArmed = false;
    this.historyLoading = true;
    this.onLoadOlder();
  }

  private followIfEnabled() {
    if (this.mergingHistory) return;
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

  private createMediaPlayer(url: string, label: string, translated: boolean) {
    const wrapper = document.createElement('span');
    wrapper.className = 'media-player';
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
    this.renderMediaButton(button, label, 'idle');

    button.addEventListener('click', async () => {
      if (audio.paused) {
        this.player.stop();
        try {
          await this.player.unlock();
          if (this.activeMediaAudio && this.activeMediaAudio !== audio) this.activeMediaAudio.pause();
          this.activeMediaAudio = audio;
          if (!audio.src) {
            button.disabled = true;
            this.renderMediaButton(button, label, 'loading');
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
          await audio.play();
        } catch (error) {
          this.activeMediaAudio = null;
          this.onMediaPlaybackChange(false);
          this.onError(
            error instanceof Error ? `无法播放${label}：${error.message}` : `无法播放${label}`,
          );
        } finally {
          button.disabled = false;
          if (audio.paused) this.renderMediaButton(button, label, 'idle');
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
    audio.addEventListener('play', () => {
      this.onMediaPlaybackChange(true);
      this.renderMediaButton(button, label, 'playing');
    });
    audio.addEventListener('pause', () => {
      this.onMediaPlaybackChange(false);
      this.renderMediaButton(button, label, 'idle');
    });
    audio.addEventListener('ended', () => {
      this.onMediaPlaybackChange(false);
      this.renderMediaButton(button, label, 'idle');
      if (this.activeMediaAudio === audio) this.activeMediaAudio = null;
    });
    return wrapper;
  }

  private setSourceText(
    article: HTMLElement,
    value: string,
    streaming: boolean,
    animate?: boolean,
  ) {
    const content = article.querySelector<HTMLElement>('.source-content')!;
    const shouldAnimate =
      animate ??
      (streaming || article.classList.contains('transcript-streaming') || this.streamingText.has(content));
    if (shouldAnimate) {
      this.queueStreamingText(content, value, !streaming, (active) =>
        this.setSourceStreamingState(article, active),
      );
    } else {
      this.commitText(content, value);
      this.setSourceStreamingState(article, streaming);
    }
  }

  private setSourceStreamingState(article: HTMLElement, streaming: boolean) {
    const content = article.querySelector<HTMLElement>('.source-content')!;
    const status = article.querySelector<HTMLElement>('.recognition-status')!;
    article.classList.toggle('transcript-streaming', streaming);
    article.querySelector('.source-text')!.classList.toggle('source-pending', !content.dataset.streamText);
    status.hidden = !streaming;
  }

  private setTranslationStreamingState(line: HTMLElement, streaming: boolean) {
    line.classList.toggle('pending', streaming);
    line.classList.toggle('translation-streaming', streaming);
  }

  private queueStreamingText(
    element: HTMLElement,
    target: string,
    done: boolean,
    onStreamingChange: (streaming: boolean) => void,
  ) {
    const state = { target, done, onStreamingChange };
    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    let displayed = element.textContent ?? '';
    if (!target.startsWith(displayed)) {
      displayed = commonGraphemePrefix(displayed, target);
      element.textContent = displayed;
    }
    element.dataset.streamText = target;
    if (reducedMotion) {
      element.textContent = target;
      if (done) this.streamingText.delete(element);
      else this.streamingText.set(element, state);
      onStreamingChange(!done);
      return;
    }
    if (displayed === target && done) {
      this.streamingText.delete(element);
      onStreamingChange(false);
      return;
    }
    this.streamingText.set(element, state);
    onStreamingChange(true);
    if (displayed !== target) this.ensureTypewriter();
  }

  private commitText(element: HTMLElement, value: string) {
    this.streamingText.delete(element);
    element.textContent = value;
    element.dataset.streamText = value;
  }

  private ensureTypewriter() {
    if (this.typewriterFrame || !this.hasPendingText()) return;
    this.typewriterLastTick = performance.now() - TYPEWRITER_INTERVAL_MS;
    this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
  }

  private advanceTypewriter = (timestamp: number) => {
    this.typewriterFrame = 0;
    const elapsed = timestamp - this.typewriterLastTick;
    if (elapsed < TYPEWRITER_INTERVAL_MS) {
      this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
      return;
    }
    const steps = Math.min(4, Math.max(1, Math.floor(elapsed / TYPEWRITER_INTERVAL_MS)));
    this.typewriterLastTick = timestamp;
    let changed = false;
    for (const [element, state] of this.streamingText) {
      let displayed = element.textContent ?? '';
      if (!state.target.startsWith(displayed)) {
        displayed = commonGraphemePrefix(displayed, state.target);
        element.textContent = displayed;
      }
      if (displayed === state.target) continue;
      const remaining = graphemes(state.target.slice(displayed.length));
      const catchUp = remaining.length > 72 ? 6 : remaining.length > 36 ? 4 : remaining.length > 16 ? 2 : 1;
      element.textContent = displayed + remaining.slice(0, Math.max(steps, catchUp)).join('');
      changed = true;
      if (element.textContent === state.target && state.done) {
        this.streamingText.delete(element);
        state.onStreamingChange(false);
      }
    }
    if (changed) this.followIfEnabled();
    if (this.hasPendingText()) {
      this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
    } else {
      this.typewriterLastTick = 0;
    }
  };

  private hasPendingText() {
    return Array.from(this.streamingText).some(
      ([element, state]) => (element.textContent ?? '') !== state.target,
    );
  }

  private renderMediaButton(
    button: HTMLButtonElement,
    label: string,
    state: 'idle' | 'loading' | 'playing',
  ) {
    const playing = state === 'playing';
    const action = state === 'loading' ? `加载${label}` : `${playing ? '暂停' : '播放'}${label}`;
    button.innerHTML = `<i data-lucide="${state === 'loading' ? 'loader-circle' : playing ? 'pause' : 'play'}"></i>`;
    button.dataset.state = state;
    button.title = action;
    button.setAttribute('aria-label', button.title);
    button.setAttribute('aria-pressed', String(playing));
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

function refinementEngineLabel(engine: string) {
  return engine === 'moss-transcribe-diarize' ? 'MOSS' : engine;
}

function graphemes(value: string) {
  if ('Segmenter' in Intl) {
    const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
    return Array.from(segmenter.segment(value), (part) => part.segment);
  }
  return Array.from(value);
}

function commonGraphemePrefix(left: string, right: string) {
  const leftParts = graphemes(left);
  const rightParts = graphemes(right);
  let length = 0;
  while (
    length < leftParts.length &&
    length < rightParts.length &&
    leftParts[length] === rightParts[length]
  ) {
    length += 1;
  }
  return leftParts.slice(0, length).join('');
}

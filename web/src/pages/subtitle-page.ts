import { ApiRequestError, apiRequest, type RoomDetail } from '../api';
import { loadAppConfig } from '../app-config';
import { refreshIcons } from '../components/icons';
import type { ServerEvent, SpeakerIdentity } from '../protocol';
import {
  loadSubtitlePreferences,
  subscribeSubtitlePreferences,
  type SubtitlePreferences,
} from '../shared/subtitle-preferences';
import type { Page } from './page';

interface CaptionUtterance {
  id: string;
  source: string;
  sourceTarget: string;
  sourceDone: boolean;
  translation: string;
  translationTarget: string;
  translationDone: boolean;
  speakers: SpeakerIdentity[];
  sourceStreaming: boolean;
  translationStreaming: boolean;
}

type SubtitleConnection = 'connecting' | 'connected' | 'offline';
type NativeSubtitleWindowAction = 'state' | 'toggle_fullscreen' | 'minimize' | 'hide';

const MAX_VISIBLE_UTTERANCES = 3;
const TYPEWRITER_INTERVAL_MS = 28;

export class SubtitlePage implements Page {
  private root: HTMLElement | null = null;
  private socket: WebSocket | null = null;
  private socketVersion = 0;
  private reconnectTimer = 0;
  private destroyed = false;
  private resizeObserver: ResizeObserver | null = null;
  private unsubscribePreferences = () => {};
  private preferences: SubtitlePreferences;
  private utterances = new Map<string, CaptionUtterance>();
  private order: string[] = [];
  private isAppShell = false;
  private typewriterFrame = 0;
  private typewriterLastTick = 0;
  private fitTimer = 0;
  private fitAllowGrow = false;
  private fitScale = 1;
  private announcedText = new Set<string>();
  private historySync: Promise<void> | null = null;
  private readonly handleBrowserFullscreenChange = () => {
    this.setFullscreenControl(Boolean(document.fullscreenElement));
  };

  constructor(
    private readonly userId: string,
    private readonly roomId: string,
    private readonly onRooms: () => void,
    private readonly onError: (message: string) => void,
  ) {
    this.preferences = loadSubtitlePreferences(userId);
  }

  async mount(root: HTMLElement) {
    this.root = root;
    this.destroyed = false;
    document.body.classList.add('subtitle-route');
    let detail: RoomDetail;
    try {
      detail = await this.getDetail();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法打开字幕大屏');
      this.onRooms();
      return;
    }
    this.mergeHistory(detail);
    root.innerHTML = this.template();
    root.querySelector('.subtitle-room-name')!.textContent = detail.room.name;
    void loadAppConfig().then((config) => {
      this.isAppShell = Boolean(config);
      this.root?.querySelector('.subtitle-minimize')?.toggleAttribute('hidden', !this.isAppShell);
      if (this.isAppShell) void this.syncNativeWindowState();
    });
    this.bindEvents();
    document.addEventListener('fullscreenchange', this.handleBrowserFullscreenChange);
    this.applyPreferences(this.preferences);
    this.unsubscribePreferences = subscribeSubtitlePreferences(this.userId, (preferences) => {
      this.preferences = preferences;
      this.applyPreferences(preferences);
    });
    const viewport = root.querySelector<HTMLElement>('.subtitle-viewport')!;
    this.resizeObserver = new ResizeObserver(() => this.scheduleFit(true));
    this.resizeObserver.observe(viewport);
    refreshIcons(root);
    this.render();
    this.connect();
  }

  destroy() {
    this.destroyed = true;
    this.socketVersion += 1;
    window.clearTimeout(this.reconnectTimer);
    this.socket?.close();
    this.socket = null;
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    window.cancelAnimationFrame(this.typewriterFrame);
    this.typewriterFrame = 0;
    window.clearTimeout(this.fitTimer);
    this.fitTimer = 0;
    this.unsubscribePreferences();
    this.unsubscribePreferences = () => {};
    this.historySync = null;
    document.removeEventListener('fullscreenchange', this.handleBrowserFullscreenChange);
    document.body.classList.remove('subtitle-route');
    this.root = null;
  }

  private async getDetail() {
    try {
      return await apiRequest<RoomDetail>(`/api/rooms/${this.roomId}`);
    } catch (error) {
      if (error instanceof ApiRequestError && error.status === 403) {
        await apiRequest(`/api/rooms/${this.roomId}/join`, { method: 'POST' });
        return apiRequest<RoomDetail>(`/api/rooms/${this.roomId}`);
      }
      throw error;
    }
  }

  private template() {
    return `
      <main class="subtitle-display" data-display-mode="${this.preferences.displayMode}" data-content-alignment="${this.preferences.contentAlignment}">
        <header class="subtitle-chrome">
          <div class="subtitle-room">
            <span class="subtitle-live-dot" aria-hidden="true"></span>
            <span class="subtitle-room-name"></span>
            <small class="subtitle-connection" role="status">连接中</small>
          </div>
          <div class="subtitle-actions">
            <button class="subtitle-action subtitle-settings" type="button" title="字幕大屏设置" aria-label="字幕大屏设置"><i data-lucide="sliders-horizontal"></i></button>
            <button class="subtitle-action subtitle-minimize" type="button" title="最小化字幕大屏" aria-label="最小化字幕大屏" hidden><i data-lucide="minus"></i></button>
            <button class="subtitle-action subtitle-fullscreen" type="button" title="进入全屏" aria-label="进入全屏"><i data-lucide="maximize-2"></i></button>
            <button class="subtitle-action subtitle-close" type="button" title="关闭字幕大屏" aria-label="关闭字幕大屏"><i data-lucide="x"></i></button>
          </div>
        </header>
        <section class="subtitle-viewport" aria-label="实时会议字幕">
          <div class="subtitle-empty">
            <i data-lucide="captions"></i>
            <strong>等待发言</strong>
            <span>实时字幕将在这里显示</span>
          </div>
          <div class="subtitle-content" aria-live="off"></div>
          <div class="subtitle-announcer" aria-live="polite" aria-atomic="true"></div>
        </section>
      </main>
    `;
  }

  private bindEvents() {
    if (!this.root) return;
    this.root.querySelector('.subtitle-fullscreen')?.addEventListener('click', () =>
      void this.toggleFullscreen(),
    );
    this.root.querySelector('.subtitle-minimize')?.addEventListener('click', () =>
      void this.minimizeWindow(),
    );
    this.root.querySelector('.subtitle-settings')?.addEventListener('click', () =>
      void this.openMainPage('/settings'),
    );
    this.root.querySelector('.subtitle-close')?.addEventListener('click', () => void this.closeWindow());
  }

  private connect() {
    this.disconnect();
    const version = this.socketVersion;
    this.setConnection('connecting');
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    this.socket = new WebSocket(
      `${scheme}//${window.location.host}/ws?room_id=${encodeURIComponent(this.roomId)}`,
    );
    this.socket.onopen = () => this.setConnection('connected');
    this.socket.onmessage = (message) => this.handleMessage(message);
    this.socket.onerror = () => this.setConnection('offline');
    this.socket.onclose = () => {
      if (version !== this.socketVersion || this.destroyed) return;
      this.setConnection('offline');
      this.reconnectTimer = window.setTimeout(() => this.connect(), 1800);
    };
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

  private handleMessage(message: MessageEvent) {
    if (typeof message.data !== 'string') return;
    let event: ServerEvent;
    try {
      event = JSON.parse(message.data) as ServerEvent;
    } catch {
      this.onError('实时字幕收到无效消息');
      return;
    }
    switch (event.type) {
      case 'room_subscribed':
        this.setConnection('connected');
        void this.syncHistory();
        break;
      case 'ready':
        this.setConnection('connected');
        break;
      case 'utterance_queued':
        this.ensureUtterance(event.utterance_id);
        this.render();
        break;
      case 'utterance_speakers':
        this.ensureUtterance(event.utterance_id).speakers = event.speakers;
        this.render();
        break;
      case 'utterance_discarded':
      case 'recognition_failed':
        this.removeUtterance(event.utterance_id);
        this.render();
        break;
      case 'transcript_delta': {
        const utterance = this.ensureUtterance(event.utterance_id);
        this.setStreamingText(utterance, 'source', event.text, event.done);
        break;
      }
      case 'transcript': {
        const utterance = this.ensureUtterance(event.utterance_id);
        this.setStreamingText(utterance, 'source', event.text, true);
        break;
      }
      case 'translation_delta': {
        const utterance = this.ensureUtterance(event.utterance_id);
        this.setStreamingText(utterance, 'translation', event.text, event.done);
        break;
      }
      case 'translation': {
        const utterance = this.ensureUtterance(event.utterance_id);
        if (!utterance.sourceTarget) {
          this.setStreamingText(utterance, 'source', event.source_text, true, false);
        }
        this.setStreamingText(utterance, 'translation', event.translated_text, true);
        break;
      }
      default:
        break;
    }
  }

  private ensureUtterance(id: string) {
    let utterance = this.utterances.get(id);
    if (!utterance) {
      utterance = this.createUtterance(id);
      this.utterances.set(id, utterance);
      this.order.push(id);
      while (this.order.length > MAX_VISIBLE_UTTERANCES) {
        const removed = this.order.shift();
        if (removed) this.utterances.delete(removed);
      }
    }
    return utterance;
  }

  private createUtterance(id: string): CaptionUtterance {
    return {
      id,
      source: '',
      sourceTarget: '',
      sourceDone: false,
      translation: '',
      translationTarget: '',
      translationDone: false,
      speakers: [],
      sourceStreaming: false,
      translationStreaming: false,
    };
  }

  private mergeHistory(detail: RoomDetail) {
    const recent = detail.utterances
      .filter((utterance) => utterance.source_text || utterance.translated_text)
      .slice(0, MAX_VISIBLE_UTTERANCES)
      .reverse();
    const historyIds = new Set<string>();
    for (const record of recent) {
      historyIds.add(record.id);
      const utterance = this.utterances.get(record.id) ?? this.createUtterance(record.id);
      if (record.source_text) {
        utterance.source = record.source_text;
        utterance.sourceTarget = record.source_text;
        utterance.sourceDone = true;
        utterance.sourceStreaming = false;
      }
      if (record.translated_text) {
        utterance.translation = record.translated_text;
        utterance.translationTarget = record.translated_text;
        utterance.translationDone = true;
        utterance.translationStreaming = false;
      }
      utterance.speakers = record.speakers;
      this.utterances.set(record.id, utterance);
    }

    // Events received after the history snapshot are newer and stay at the end.
    const liveOnly = this.order.filter((id) => !historyIds.has(id) && this.utterances.has(id));
    this.order = [...recent.map((record) => record.id), ...liveOnly].slice(
      -MAX_VISIBLE_UTTERANCES,
    );
    const retained = new Set(this.order);
    for (const id of this.utterances.keys()) {
      if (!retained.has(id)) this.utterances.delete(id);
    }
  }

  private syncHistory() {
    if (this.historySync) return this.historySync;
    this.historySync = this.getDetail()
      .then((detail) => {
        if (this.destroyed) return;
        this.mergeHistory(detail);
        this.render();
      })
      .catch((error) => {
        if (!this.destroyed) {
          this.onError(error instanceof Error ? error.message : '无法同步字幕记录');
        }
      })
      .finally(() => {
        this.historySync = null;
      });
    return this.historySync;
  }

  private removeUtterance(id: string) {
    this.utterances.delete(id);
    this.order = this.order.filter((utteranceId) => utteranceId !== id);
    for (const key of this.announcedText) {
      if (key.startsWith(`${id}:`)) this.announcedText.delete(key);
    }
  }

  private render() {
    if (!this.root) return;
    const content = this.root.querySelector<HTMLElement>('.subtitle-content')!;
    const visible = this.order
      .map((id) => this.utterances.get(id))
      .filter((utterance): utterance is CaptionUtterance => Boolean(utterance))
      .filter(
        (utterance) =>
          utterance.source ||
          utterance.sourceTarget ||
          utterance.translation ||
          utterance.translationTarget,
      );
    const existing = new Map(
      Array.from(content.querySelectorAll<HTMLElement>('.subtitle-utterance')).map((element) => [
        element.dataset.utteranceId ?? '',
        element,
      ]),
    );
    const desired = visible.map((utterance, index) => {
      const article = existing.get(utterance.id) ?? this.captionElement(utterance);
      existing.delete(utterance.id);
      this.updateCaptionElement(article, utterance, index === visible.length - 1);
      return article;
    });
    existing.forEach((element) => element.remove());
    desired.forEach((element, index) => {
      if (content.children[index] !== element) {
        content.insertBefore(element, content.children[index] ?? null);
      }
    });
    this.root.querySelector<HTMLElement>('.subtitle-empty')!.hidden = visible.length > 0;
    this.scheduleFit();
  }

  private captionElement(utterance: CaptionUtterance) {
    const article = document.createElement('article');
    article.className = 'subtitle-utterance';
    article.dataset.utteranceId = utterance.id;
    const speaker = document.createElement('span');
    speaker.className = 'subtitle-speaker';
    const source = document.createElement('p');
    source.className = 'subtitle-source';
    const translation = document.createElement('p');
    translation.className = 'subtitle-translation';
    article.append(speaker, source, translation);
    return article;
  }

  private updateCaptionElement(
    article: HTMLElement,
    utterance: CaptionUtterance,
    active: boolean,
  ) {
    article.classList.toggle('active', active);
    const speaker = article.querySelector<HTMLElement>('.subtitle-speaker')!;
    const speakerText = utterance.speakers.map((identity) => identity.username).join(' / ');
    if (speaker.textContent !== speakerText) speaker.textContent = speakerText;
    speaker.hidden = !speakerText;
    this.updateCaptionText(utterance, 'source', article);
    this.updateCaptionText(utterance, 'translation', article);
  }

  private setStreamingText(
    utterance: CaptionUtterance,
    kind: 'source' | 'translation',
    target: string,
    done: boolean,
    render = true,
  ) {
    const displayedKey = kind;
    const targetKey = kind === 'source' ? 'sourceTarget' : 'translationTarget';
    const doneKey = kind === 'source' ? 'sourceDone' : 'translationDone';
    const streamingKey = kind === 'source' ? 'sourceStreaming' : 'translationStreaming';
    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (reducedMotion) {
      utterance[displayedKey] = target;
    } else if (!target.startsWith(utterance[displayedKey])) {
      utterance[displayedKey] = commonGraphemePrefix(utterance[displayedKey], target);
    }
    utterance[targetKey] = target;
    utterance[doneKey] = done;
    utterance[streamingKey] = !done || utterance[displayedKey] !== target;
    if (done && target) this.announceCompletedText(utterance.id, kind, target);
    if (render) {
      const article = this.findCaptionElement(utterance.id);
      if (article) {
        this.updateCaptionText(utterance, kind, article);
        this.scheduleFit();
      } else {
        this.render();
      }
    }
    if (utterance[displayedKey] !== target) this.ensureTypewriter();
  }

  private ensureTypewriter() {
    if (this.typewriterFrame || !this.hasPendingText() || !this.root) return;
    this.typewriterLastTick = performance.now() - TYPEWRITER_INTERVAL_MS;
    this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
  }

  private advanceTypewriter = (timestamp: number) => {
    this.typewriterFrame = 0;
    if (!this.root || this.destroyed) return;
    const elapsed = timestamp - this.typewriterLastTick;
    if (elapsed < TYPEWRITER_INTERVAL_MS) {
      this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
      return;
    }
    const steps = Math.min(4, Math.max(1, Math.floor(elapsed / TYPEWRITER_INTERVAL_MS)));
    this.typewriterLastTick = timestamp;
    let changed = false;
    for (const utterance of this.utterances.values()) {
      if (this.revealText(utterance, 'source', steps)) {
        this.updateCaptionText(utterance, 'source');
        changed = true;
      }
      if (this.revealText(utterance, 'translation', steps)) {
        this.updateCaptionText(utterance, 'translation');
        changed = true;
      }
    }
    if (changed) this.scheduleFit();
    if (this.hasPendingText()) {
      this.typewriterFrame = window.requestAnimationFrame(this.advanceTypewriter);
    } else {
      this.typewriterLastTick = 0;
    }
  };

  private revealText(
    utterance: CaptionUtterance,
    kind: 'source' | 'translation',
    steps: number,
  ) {
    const displayedKey = kind;
    const targetKey = kind === 'source' ? 'sourceTarget' : 'translationTarget';
    const doneKey = kind === 'source' ? 'sourceDone' : 'translationDone';
    const streamingKey = kind === 'source' ? 'sourceStreaming' : 'translationStreaming';
    const target = utterance[targetKey];
    let displayed = utterance[displayedKey];
    if (displayed === target) return false;
    if (!target.startsWith(displayed)) displayed = commonGraphemePrefix(displayed, target);
    const remaining = graphemes(target.slice(displayed.length));
    const catchUp = remaining.length > 72 ? 6 : remaining.length > 36 ? 4 : remaining.length > 16 ? 2 : 1;
    utterance[displayedKey] = displayed + remaining.slice(0, Math.max(steps, catchUp)).join('');
    utterance[streamingKey] = !utterance[doneKey] || utterance[displayedKey] !== target;
    return true;
  }

  private updateCaptionText(
    utterance: CaptionUtterance,
    kind: 'source' | 'translation',
    article = this.findCaptionElement(utterance.id),
  ) {
    if (!article) return;
    const element = article.querySelector<HTMLElement>(`.subtitle-${kind}`);
    if (!element) return;
    if (element.textContent !== utterance[kind]) element.textContent = utterance[kind];
    element.classList.toggle(
      'streaming',
      kind === 'source' ? utterance.sourceStreaming : utterance.translationStreaming,
    );
  }

  private findCaptionElement(id: string) {
    return Array.from(
      this.root?.querySelectorAll<HTMLElement>('.subtitle-utterance') ?? [],
    ).find((element) => element.dataset.utteranceId === id);
  }

  private hasPendingText() {
    return Array.from(this.utterances.values()).some(
      (utterance) =>
        utterance.source !== utterance.sourceTarget ||
        utterance.translation !== utterance.translationTarget,
    );
  }

  private announceCompletedText(id: string, kind: 'source' | 'translation', value: string) {
    const key = `${id}:${kind}:${value}`;
    if (this.announcedText.has(key)) return;
    this.announcedText.add(key);
    const announcer = this.root?.querySelector<HTMLElement>('.subtitle-announcer');
    if (announcer) announcer.textContent = `${kind === 'source' ? '原文' : '译文'}：${value}`;
  }

  private scheduleFit(allowGrow = false) {
    this.fitAllowGrow ||= allowGrow;
    if (this.fitTimer) return;
    this.fitTimer = window.setTimeout(() => {
      this.fitTimer = 0;
      const shouldAllowGrow = this.fitAllowGrow;
      this.fitAllowGrow = false;
      this.fitContent(shouldAllowGrow);
    }, 48);
  }

  private applyPreferences(preferences: SubtitlePreferences) {
    if (!this.root) return;
    const display = this.root.querySelector<HTMLElement>('.subtitle-display')!;
    display.dataset.displayMode = preferences.displayMode;
    display.dataset.contentAlignment = preferences.contentAlignment;
    display.style.setProperty('--subtitle-background', preferences.backgroundColor);
    display.style.setProperty('--subtitle-source-color', preferences.sourceColor);
    display.style.setProperty('--subtitle-translation-color', preferences.translationColor);
    display.style.setProperty('--subtitle-source-size', `${preferences.sourceFontSize}px`);
    display.style.setProperty('--subtitle-translation-size', `${preferences.translationFontSize}px`);
    display.style.setProperty('--subtitle-line-height', String(preferences.lineHeight));
    display.style.setProperty('--subtitle-block-gap', `${preferences.blockGap}px`);
    display.style.setProperty('--subtitle-padding', `${preferences.screenPadding}px`);
    this.scheduleFit(true);
  }

  private fitContent(allowGrow = false) {
    if (!this.root) return;
    const viewport = this.root.querySelector<HTMLElement>('.subtitle-viewport');
    const content = this.root.querySelector<HTMLElement>('.subtitle-content');
    if (!viewport || !content) return;
    if (!content.childElementCount) {
      this.applyFit(1);
      return;
    }
    const utterances = Array.from(content.children) as HTMLElement[];
    if (allowGrow) {
      utterances.forEach((element) => element.removeAttribute('hidden'));
      this.applyFit(1);
    }
    const viewportStyle = getComputedStyle(viewport);
    const available = Math.max(
      1,
      viewport.clientHeight -
        Number.parseFloat(viewportStyle.paddingTop) -
        Number.parseFloat(viewportStyle.paddingBottom),
    );
    const requiredHeight = () => {
      const visible = utterances.filter((element) => !element.hasAttribute('hidden'));
      const rowGap = Number.parseFloat(getComputedStyle(content).rowGap) || 0;
      return (
        visible.reduce((total, element) => total + element.getBoundingClientRect().height, 0) +
        Math.max(0, visible.length - 1) * rowGap
      );
    };
    let required = requiredHeight();
    if (!allowGrow && required <= available + 2) return;
    if (required > available) {
      const readableScale = Math.min(
        1,
        Math.max(
          24 / this.preferences.sourceFontSize,
          18 / this.preferences.translationFontSize,
        ),
      );
      const minimumScale = allowGrow
        ? readableScale
        : Math.min(readableScale, this.fitScale);
      const scale = Math.max(
        minimumScale,
        Math.min(this.fitScale, this.fitScale * (available / required) * 0.96),
      );
      this.applyFit(scale);
      required = requiredHeight();
    }
    for (const utterance of utterances.slice(0, -1)) {
      if (required <= available) break;
      utterance.hidden = true;
      required = requiredHeight();
    }
    let scale = this.fitScale;
    for (let attempt = 0; attempt < 3 && required > available; attempt += 1) {
      scale = Math.max(0.25, scale * (available / required) * 0.96);
      this.applyFit(scale);
      required = requiredHeight();
    }
  }

  private applyFit(scale: number) {
    const content = this.root?.querySelector<HTMLElement>('.subtitle-content');
    if (!content) return;
    this.fitScale = scale;
    content.style.setProperty(
      '--subtitle-source-fitted',
      `${(this.preferences.sourceFontSize * scale).toFixed(2)}px`,
    );
    content.style.setProperty(
      '--subtitle-translation-fitted',
      `${(this.preferences.translationFontSize * scale).toFixed(2)}px`,
    );
    content.style.setProperty(
      '--subtitle-gap-fitted',
      `${(this.preferences.blockGap * scale).toFixed(2)}px`,
    );
    content.style.setProperty(
      '--subtitle-inner-gap-fitted',
      `${(this.preferences.blockGap * 0.46 * scale).toFixed(2)}px`,
    );
  }

  private setConnection(status: SubtitleConnection) {
    if (!this.root) return;
    const display = this.root.querySelector<HTMLElement>('.subtitle-display')!;
    display.dataset.connection = status;
    this.root.querySelector('.subtitle-connection')!.textContent =
      status === 'connected' ? '实时同步' : status === 'connecting' ? '连接中' : '等待重连';
  }

  private async toggleFullscreen() {
    const nativeFullscreen = await this.performNativeWindowAction('toggle_fullscreen');
    if (nativeFullscreen !== null) {
      this.setFullscreenControl(nativeFullscreen);
      return;
    }
    try {
      if (!document.fullscreenElement) await document.documentElement.requestFullscreen();
      else await document.exitFullscreen();
    } catch {
      this.onError('无法切换字幕大屏全屏状态');
    }
  }

  private async syncNativeWindowState() {
    const fullscreen = await this.performNativeWindowAction('state');
    if (fullscreen !== null) this.setFullscreenControl(fullscreen);
  }

  private setFullscreenControl(fullscreen: boolean) {
    const button = this.root?.querySelector<HTMLButtonElement>('.subtitle-fullscreen');
    if (!button) return;
    const label = fullscreen ? '退出全屏' : '进入全屏';
    button.title = label;
    button.setAttribute('aria-label', label);
    button.innerHTML = `<i data-lucide="${fullscreen ? 'minimize-2' : 'maximize-2'}"></i>`;
    refreshIcons(button);
  }

  private async minimizeWindow() {
    const result = await this.performNativeWindowAction('minimize');
    if (result === null) this.onError('无法最小化字幕大屏');
  }

  private async performNativeWindowAction(action: NativeSubtitleWindowAction) {
    const response = await fetch('/__voice_elf/subtitle-window/action', {
      method: 'POST',
      headers: { accept: 'application/json', 'content-type': 'application/json' },
      body: JSON.stringify({ room_id: this.roomId, action }),
    }).catch(() => null);
    if (
      !response?.ok ||
      !response.headers.get('content-type')?.includes('application/json')
    ) {
      return null;
    }
    const payload = (await response.json().catch(() => null)) as { fullscreen?: unknown } | null;
    return typeof payload?.fullscreen === 'boolean' ? payload.fullscreen : null;
  }

  private async openMainPage(path: string) {
    if (!this.isAppShell) {
      window.open(path, 'voice-elf-settings');
      return;
    }
    const response = await fetch('/__voice_elf/settings-window', { method: 'POST' }).catch(
      () => null,
    );
    if (response?.ok) return;
    this.onError('无法打开字幕大屏设置');
  }

  private async closeWindow() {
    const hidden = await this.performNativeWindowAction('hide');
    if (hidden !== null) return;
    const response = await fetch('/__voice_elf/subtitle-window/close', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ room_id: this.roomId }),
    }).catch(() => null);
    if (response?.ok) return;
    window.close();
    if (!window.closed) this.onRooms();
  }
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

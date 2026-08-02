import { ApiRequestError, apiRequest, type RoomDetail, type RoomInput, type RoomSummary } from '../api';
import { PcmPlayer } from '../audio';
import { ConversationView } from '../components/conversation-view';
import { LanguageDialog } from '../components/language-dialog';
import { refreshIcons } from '../components/icons';
import { LatencyMonitor } from '../components/latency-monitor';
import { RoomEditor } from '../components/room-editor';
import type { ConnectionStatus } from '../components/topbar';
import { VoiceSession } from '../controllers/voice-session';
import { BrowserTts } from '../controllers/browser-tts';
import type { PipelinePhase, ServerEvent, SessionConfig } from '../protocol';
import { languageNames } from '../shared/languages';
import { loadPreferences } from '../shared/preferences';
import type { Page } from './page';

export class TranslatorPage implements Page {
  private root: HTMLElement | null = null;
  private room: RoomSummary | null = null;
  private conversation: ConversationView | null = null;
  private monitor: LatencyMonitor | null = null;
  private voiceSession: VoiceSession | null = null;
  private roomEditor: RoomEditor | null = null;
  private languageDialog: LanguageDialog | null = null;
  private player = new PcmPlayer();
  private browserTts = new BrowserTts();
  private timer = 0;
  private startedAt = 0;
  private recording = false;
  private sourceLanguage = 'auto';
  private targetLanguage = 'zh';
  private maxUtteranceSeconds = 20;
  private voice = 'F1';

  constructor(
    private readonly userId: string,
    private readonly roomId: string,
    private readonly onRooms: () => void,
    private readonly onDeleted: () => void,
    private readonly onConnection: (status: ConnectionStatus) => void,
    private readonly onError: (message: string) => void,
  ) {}

  async mount(root: HTMLElement) {
    this.root = root;
    let detail: RoomDetail;
    try {
      detail = await this.getDetail();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法进入房间');
      this.onRooms();
      return;
    }
    this.room = detail.room;
    this.sourceLanguage = detail.room.source_language;
    this.targetLanguage = detail.room.target_language;
    this.maxUtteranceSeconds = detail.room.max_utterance_seconds;
    const preferences = loadPreferences(this.userId);
    this.voice = preferences.voice;
    this.player.muted = !preferences.autoplay;
    root.innerHTML = this.template(detail.room);
    this.conversation = new ConversationView(this.player, this.onError);
    this.monitor = new LatencyMonitor();
    root.querySelector('.conversation-mount')!.replaceWith(this.conversation.element);
    root.querySelector('.monitor-mount')!.replaceWith(this.monitor.element);
    this.conversation.renderHistory(detail);
    this.monitor.reset();
    detail.utterances
      .slice()
      .reverse()
      .forEach((utterance) => this.monitor?.addLatency(utterance.latency));
    this.bindEvents();
    this.roomEditor = new RoomEditor((saved) => this.applyRoom(saved));
    this.languageDialog = new LanguageDialog(
      (source, target) => this.persistLanguages(source, target),
      this.onError,
    );
    refreshIcons(root);

    if (detail.room.is_owner) {
      this.voiceSession = new VoiceSession(
        detail.room.id,
        root.querySelector<HTMLCanvasElement>('#waveform')!,
        this.player,
        () => this.sessionConfig(),
        {
          onEvent: (event) => this.handleEvent(event),
          onConnection: (status) => this.setConnection(status),
          onRecording: (recording) => this.setRecording(recording),
          onCaptureError: this.onError,
          onPlaybackProgress: (progress) =>
            this.conversation?.updateTranslatedProgress(
              progress.utteranceId,
              progress.currentSeconds,
              progress.durationSeconds,
            ),
        },
      );
      this.voiceSession.connect();
    } else {
      this.onConnection('viewer');
      root.querySelector<HTMLButtonElement>('.record-button')!.disabled = true;
      root.querySelector('.capture-state')!.textContent = '只读预览';
    }
  }

  async destroy() {
    window.clearInterval(this.timer);
    await this.voiceSession?.destroy();
    this.voiceSession = null;
    this.conversation?.destroy();
    this.conversation = null;
    this.monitor?.destroy();
    this.monitor = null;
    this.roomEditor?.destroy();
    this.roomEditor = null;
    this.languageDialog?.destroy();
    this.languageDialog = null;
    this.player.stop();
    this.browserTts.destroy();
    this.onConnection('hidden');
    this.root = null;
  }

  private async getDetail(search = '') {
    const query = search.trim() ? `?q=${encodeURIComponent(search.trim())}` : '';
    try {
      return await apiRequest<RoomDetail>(`/api/rooms/${this.roomId}${query}`);
    } catch (error) {
      if (error instanceof ApiRequestError && error.status === 403 && !search) {
        await apiRequest(`/api/rooms/${this.roomId}/join`, { method: 'POST' });
        return apiRequest<RoomDetail>(`/api/rooms/${this.roomId}`);
      }
      throw error;
    }
  }

  private template(room: RoomSummary) {
    return `
      <main class="translator-page app-shell">
        <section class="room-toolbar">
          <button class="room-back icon-button" type="button" title="返回房间目录" aria-label="返回房间目录"><i data-lucide="arrow-left"></i></button>
          <div class="room-identity-static">
            <span class="room-status-icon"><i data-lucide="door-open"></i></span>
            <span><small>当前房间</small><strong class="room-name">${escapeHtml(room.name)}</strong></span>
            <span class="room-role${room.is_owner ? ' owner' : ''}">${room.is_owner ? '房主控制' : '成员预览'}</span>
          </div>
          <form class="record-search">
            <i data-lucide="search"></i><input type="search" placeholder="检索转写或翻译记录" aria-label="检索房间记录"><button type="submit">检索</button>
          </form>
          <div class="room-owner-actions" ${room.is_owner ? '' : 'hidden'}>
            <button class="icon-button edit-room" type="button" title="编辑房间" aria-label="编辑房间"><i data-lucide="settings"></i></button>
            <button class="icon-button danger delete-room" type="button" title="删除房间" aria-label="删除房间"><i data-lucide="trash-2"></i></button>
          </div>
        </section>

        <div class="workspace-grid">
          <section class="conversation-panel">
            <div class="panel-heading">
              <div><span class="section-kicker"><i data-lucide="radio"></i> LIVE SESSION</span><h1>实时对话</h1></div>
              <div class="panel-actions">
                <span class="capture-session-badge"><span class="session-dot"></span><span class="session-badge-text">${room.is_owner ? '等待录音' : '只读预览'}</span></span>
                <div class="monitor-mount"></div>
                <button class="icon-button refresh-history" type="button" title="刷新记录" aria-label="刷新记录"><i data-lucide="refresh-cw"></i></button>
              </div>
            </div>
            <div class="conversation-mount"></div>
            <div class="capture-console">
              <canvas id="waveform" aria-hidden="true"></canvas>
              <button class="language-config-button" type="button" title="选择翻译语言" ${room.is_owner ? '' : 'disabled'}>
                <span class="language-source-label">${escapeHtml(languageNames[room.source_language] ?? room.source_language)}</span>
                <i data-lucide="arrow-left-right"></i>
                <span class="language-target-label">${escapeHtml(languageNames[room.target_language] ?? room.target_language)}</span>
              </button>
              <button class="record-button" type="button" aria-label="开始录音" title="开始录音" disabled><i data-lucide="mic"></i></button>
              <div class="capture-readout"><strong class="capture-state">${room.is_owner ? '连接中' : '只读预览'}</strong><span class="capture-time">00:00</span></div>
            </div>
          </section>
        </div>
      </main>
    `;
  }

  private bindEvents() {
    if (!this.root || !this.room) return;
    this.root.querySelector('.room-back')?.addEventListener('click', this.onRooms);
    this.root.querySelector('.record-button')?.addEventListener('click', () =>
      void this.voiceSession?.toggleRecording().catch((error) =>
        this.onError(error instanceof Error ? error.message : '无法访问麦克风'),
      ),
    );
    this.root.querySelector('.language-config-button')?.addEventListener('click', () =>
      this.languageDialog?.open(this.sourceLanguage, this.targetLanguage),
    );
    this.root.querySelector('.refresh-history')?.addEventListener('click', () => void this.loadHistory());
    this.root.querySelector<HTMLFormElement>('.record-search')?.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.loadHistory(this.root?.querySelector<HTMLInputElement>('.record-search input')?.value ?? '');
    });
    this.root.querySelector('.edit-room')?.addEventListener('click', () => {
      if (this.room) this.roomEditor?.open(this.room);
    });
    this.root.querySelector('.delete-room')?.addEventListener('click', () => void this.deleteRoom());
  }

  private sessionConfig(): SessionConfig {
    return {
      source_language: this.sourceLanguage,
      target_language: this.targetLanguage,
      voice: this.voice,
      max_utterance_seconds: this.maxUtteranceSeconds,
    };
  }

  private handleEvent(event: ServerEvent) {
    if (!this.root) return;
    switch (event.type) {
      case 'ready':
        this.monitor?.setBackend(event.backend);
        break;
      case 'state':
        this.setPhase(event.phase);
        if (event.phase === 'transcribing' && event.utterance_id) {
          this.conversation?.upsertTranscript(
            { utterance_id: event.utterance_id, text: '', language: this.sessionConfig().source_language },
            true,
          );
        }
        break;
      case 'utterance_queued':
        // Queued segments do not create empty cards. The transcribing state creates the row
        // once the ASR worker actually starts processing this utterance.
        break;
      case 'recognition_failed':
        this.conversation?.markRecognitionFailed(event.utterance_id, event.message);
        break;
      case 'processing_failed':
        this.conversation?.markProcessingFailed(event.utterance_id, event.stage, event.message);
        break;
      case 'transcript':
        this.conversation?.upsertTranscript(event, false);
        break;
      case 'transcript_delta':
        this.conversation?.applyTranscriptDelta(event);
        break;
      case 'translation_delta':
        this.conversation?.applyTranslationDelta(event);
        break;
      case 'translation':
        this.conversation?.applyTranslation(event);
        if (this.room?.is_owner) void this.synthesizeTranslation(event);
        break;
      case 'media':
        this.conversation?.applyMedia(event);
        break;
      case 'latency':
        this.monitor?.addLatency(event.latency);
        this.conversation?.updateItemLatency(event.utterance_id, event.latency.total_ms);
        break;
      case 'warning':
        this.onError(event.message);
        break;
      default:
        break;
    }
  }

  private async synthesizeTranslation(event: Extract<ServerEvent, { type: 'translation' }>) {
    try {
      this.conversation?.updateTtsProgress(event.utterance_id, 0);
      const audio = await this.browserTts.synthesizeAndSave(
        event.utterance_id,
        event.translated_text,
        event.target_language,
        this.voice,
        (progress) => this.conversation?.updateTtsProgress(event.utterance_id, progress),
      );
      this.conversation?.applyMedia({ type: 'media', utterance_id: event.utterance_id,
        source_audio_url: null, translated_audio_url: audio.translated_audio_url });
      this.monitor?.addLatency(audio.latency, false);
      this.conversation?.updateItemLatency(event.utterance_id, audio.latency.total_ms);
      await this.player.enqueue(audio.pcm, audio.sampleRate, () => undefined);
    } catch (error) {
      const message = error instanceof Error ? error.message : '浏览器译声生成失败';
      this.conversation?.markProcessingFailed(event.utterance_id, 'tts', message);
      this.onError(`译声生成失败：${message}`);
    }
  }

  private setPhase(phase: PipelinePhase) {
    const labels: Record<PipelinePhase, string> = {
      listening: '正在聆听',
      speech: '检测到语音',
      transcribing: '语音识别',
      translating: '正在翻译',
      synthesizing: '生成语音',
      playing: '正在播报',
    };
    if (!this.recording || phase === 'listening' || phase === 'speech') {
      this.root!.querySelector('.capture-state')!.textContent =
        this.recording && phase === 'listening' ? '持续聆听' : labels[phase];
    }
    this.monitor?.setPhase(phase);
  }

  private setConnection(status: ConnectionStatus) {
    this.onConnection(status);
    if (!this.root) return;
    this.root.querySelector<HTMLButtonElement>('.record-button')!.disabled = status !== 'connected';
    if (status === 'connected') this.root.querySelector('.capture-state')!.textContent = '就绪';
  }

  private setRecording(recording: boolean) {
    if (!this.root) return;
    this.recording = recording;
    const button = this.root.querySelector<HTMLButtonElement>('.record-button')!;
    const console = this.root.querySelector<HTMLElement>('.capture-console')!;
    const badge = this.root.querySelector<HTMLElement>('.capture-session-badge')!;
    button.classList.toggle('recording', recording);
    console.classList.toggle('is-recording', recording);
    badge.classList.toggle('active', recording);
    this.root.querySelector('.translator-page')?.classList.toggle('is-recording', recording);
    this.root.querySelector('.session-badge-text')!.textContent = recording ? '连续录音中' : '等待录音';
    this.root.querySelector('.capture-state')!.textContent = recording ? '持续聆听' : '就绪';
    button.innerHTML = `<i data-lucide="${recording ? 'circle-stop' : 'mic'}"></i>`;
    button.title = recording ? '停止录音' : '开始录音';
    button.ariaLabel = button.title;
    refreshIcons(button);
    window.clearInterval(this.timer);
    if (recording) {
      this.startedAt = Date.now();
      this.timer = window.setInterval(() => this.updateTimer(), 250);
    } else {
      this.root.querySelector('.capture-time')!.textContent = '00:00';
    }
  }

  private updateTimer() {
    const seconds = Math.floor((Date.now() - this.startedAt) / 1000);
    this.root!.querySelector('.capture-time')!.textContent =
      `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
  }

  private async persistLanguages(source: string, target: string) {
    if (!this.room?.is_owner) return;
    this.room = await apiRequest<RoomSummary>(`/api/rooms/${this.room.id}`, {
      method: 'PATCH',
      body: JSON.stringify({
        name: this.room.name,
        source_language: source,
        target_language: target,
        max_utterance_seconds: this.room.max_utterance_seconds,
      } satisfies RoomInput),
    });
    this.sourceLanguage = this.room.source_language;
    this.targetLanguage = this.room.target_language;
    this.maxUtteranceSeconds = this.room.max_utterance_seconds;
    this.updateLanguageButton();
    this.voiceSession?.sendConfig();
  }

  private async loadHistory(search = '') {
    try {
      const detail = await this.getDetail(search);
      this.conversation?.renderHistory(detail);
      this.monitor?.reset();
      detail.utterances
        .slice()
        .reverse()
        .forEach((utterance) => this.monitor?.addLatency(utterance.latency));
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载记录');
    }
  }

  private applyRoom(room: RoomSummary) {
    this.room = room;
    this.sourceLanguage = room.source_language;
    this.targetLanguage = room.target_language;
    this.maxUtteranceSeconds = room.max_utterance_seconds;
    if (!this.root) return;
    this.root.querySelector('.room-name')!.textContent = room.name;
    this.updateLanguageButton();
    this.voiceSession?.sendConfig();
  }

  private updateLanguageButton() {
    if (!this.root) return;
    this.root.querySelector('.language-source-label')!.textContent =
      languageNames[this.sourceLanguage] ?? this.sourceLanguage;
    this.root.querySelector('.language-target-label')!.textContent =
      languageNames[this.targetLanguage] ?? this.targetLanguage;
  }

  private async deleteRoom() {
    if (!this.room?.is_owner || !window.confirm(`确认删除房间“${this.room.name}”及其全部记录？`)) return;
    try {
      await apiRequest(`/api/rooms/${this.room.id}`, { method: 'DELETE' });
      this.onDeleted();
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法删除房间');
    }
  }
}

function escapeHtml(value: string) {
  const element = document.createElement('div');
  element.textContent = value;
  return element.innerHTML;
}

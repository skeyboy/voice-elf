import {
  ApiRequestError,
  apiRequest,
  type Paginated,
  type RoomDetail,
  type RoomInput,
  type RoomMemberState,
  type RoomSummary,
  type UtteranceHistory,
} from '../api';
import { loadAppConfig } from '../app-config';
import {
  isAndroidNativeShell,
  isAndroidSubtitleOverlayVisible,
  showAndroidSubtitleOverlay,
  subscribeAndroidNative,
  updateAndroidSubtitleOverlay,
  type AndroidSubtitlePayload,
} from '../android-native';
import { PcmPlayer, supportsSystemAudioCapture } from '../audio';
import { CaptureOptions, type CaptureOptionValues } from '../components/capture-options';
import { ConversationView } from '../components/conversation-view';
import { LanguageDialog } from '../components/language-dialog';
import { refreshIcons } from '../components/icons';
import { LatencyMonitor } from '../components/latency-monitor';
import { renderPageLoading } from '../components/page-loading';
import { RoomEditor } from '../components/room-editor';
import type { ConnectionStatus } from '../components/topbar';
import { VoiceSession } from '../controllers/voice-session';
import type { PipelinePhase, ServerEvent, SessionConfig } from '../protocol';
import { languageNames } from '../shared/languages';
import { loadPreferences, savePreferences, subscribePreferences } from '../shared/preferences';
import {
  loadSubtitlePreferences,
  subscribeSubtitlePreferences,
} from '../shared/subtitle-preferences';
import type { Page } from './page';

interface TranslatorViewState {
  query: string;
  scrollTop: number;
}

interface TranslatorCacheEntry {
  detail: RoomDetail;
  utterances: UtteranceHistory[];
  historyPage: number;
  historyTotal: number;
  historyQuery: string;
}

const translatorViewStates = new Map<string, TranslatorViewState>();
const translatorCache = new Map<string, TranslatorCacheEntry>();

export class TranslatorPage implements Page {
  private root: HTMLElement | null = null;
  private room: RoomSummary | null = null;
  private conversation: ConversationView | null = null;
  private monitor: LatencyMonitor | null = null;
  private voiceSession: VoiceSession | null = null;
  private roomEditor: RoomEditor | null = null;
  private languageDialog: LanguageDialog | null = null;
  private captureOptions: CaptureOptions | null = null;
  private player = new PcmPlayer();
  private timer = 0;
  private startedAt = 0;
  private recording = false;
  private sourceLanguage = 'auto';
  private targetLanguage = 'zh';
  private maxUtteranceSeconds = 20;
  private voice = 'F1';
  private enhancedVoiceFilter = true;
  private microphoneCapture = true;
  private systemAudioCapture = false;
  private noiseSuppression = true;
  private echoCancellation = true;
  private readonly latencyIds = new Set<string>();
  private historySync: Promise<void> | null = null;
  private historyRequestVersion = 0;
  private historyPage = 0;
  private historyTotal = 0;
  private historyQuery = '';
  private pendingHistoryScroll: number | null = null;
  private historyRecords: UtteranceHistory[] = [];
  private members: RoomMemberState[] = [];
  private canPublish = false;
  private connectionStatus: ConnectionStatus = 'offline';
  private isAppShell = false;
  private unsubscribePreferences = () => {};
  private unsubscribeSubtitlePreferences = () => {};
  private unsubscribeAndroidNative = () => {};
  private androidOverlayActive = false;
  private readonly androidCaptions = new Map<string, { source: string; translation: string }>();
  private androidCaptionOrder: string[] = [];

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
    const savedView = translatorViewStates.get(this.cacheKey());
    const cached = translatorCache.get(this.cacheKey());
    if (!cached) renderPageLoading(root, '正在进入实时会话', '同步会议、成员和最近字幕');
    this.historyQuery = savedView?.query ?? '';
    this.pendingHistoryScroll = savedView?.scrollTop || null;
    let detail: RoomDetail;
    try {
      detail = cached?.detail ?? await this.getDetail(savedView?.query);
    } catch (error) {
      if (this.root !== root) return;
      this.onError(error instanceof Error ? error.message : '无法进入房间');
      this.onRooms();
      return;
    }
    if (this.root !== root) return;
    this.room = detail.room;
    translatorCache.set(this.cacheKey(), cached ?? {
      detail,
      utterances: [],
      historyPage: 0,
      historyTotal: 0,
      historyQuery: this.historyQuery,
    });
    void loadAppConfig().then((config) => {
      this.isAppShell = Boolean(config);
    });
    this.members = detail.members;
    const currentMember = detail.members.find((member) => member.user_id === this.userId);
    this.canPublish = detail.room.status === 'active'
      && (detail.room.is_owner || Boolean(currentMember && !currentMember.is_muted));
    this.sourceLanguage = detail.room.source_language;
    this.targetLanguage = detail.room.target_language;
    this.maxUtteranceSeconds = detail.room.max_utterance_seconds;
    const preferences = loadPreferences(this.userId);
    this.voice = preferences.voice;
    this.enhancedVoiceFilter = preferences.enhancedVoiceFilter;
    this.microphoneCapture = preferences.microphoneCapture;
    this.systemAudioCapture = preferences.systemAudioCapture && supportsSystemAudioCapture();
    this.noiseSuppression = preferences.noiseSuppression;
    this.echoCancellation = preferences.echoCancellation;
    this.player.muted = !preferences.autoplay;
    root.innerHTML = this.template(detail.room);
    this.renderMembers();
    this.conversation = new ConversationView(this.player, this.onError, (active) =>
      this.voiceSession?.setExternalPlaybackActive(active),
      () => void this.loadHistory(this.historyQuery, false),
    );
    this.monitor = new LatencyMonitor();
    root.querySelector('.conversation-mount')!.replaceWith(this.conversation.element);
    root.querySelector('.monitor-mount')!.replaceWith(this.monitor.element);
    this.captureOptions = new CaptureOptions(
      this.captureOptionValues(),
      supportsSystemAudioCapture(),
      (values) => this.updateCaptureOptions(values),
      {
        sourceLabel: languageNames[this.sourceLanguage] ?? this.sourceLanguage,
        targetLabel: languageNames[this.targetLanguage] ?? this.targetLanguage,
        editable: detail.room.is_owner,
        onOpen: () => this.languageDialog?.open(this.sourceLanguage, this.targetLanguage),
      },
    );
    root.querySelector('.capture-options-mount')!.replaceWith(this.captureOptions.element);
    if (cached && cached.historyQuery === this.historyQuery) {
      this.historyPage = cached.historyPage;
      this.historyTotal = cached.historyTotal;
      this.historyRecords = cached.utterances;
      this.seedAndroidCaptions(cached.utterances);
      this.conversation.renderUtterances(cached.utterances);
      this.conversation.setHistoryPagination(
        Math.min(cached.historyPage * 30, cached.historyTotal),
        cached.historyTotal,
        false,
      );
      this.resetLatency(cached.utterances);
    }
    const search = root.querySelector<HTMLInputElement>('.record-search input');
    if (search) search.value = this.historyQuery;
    if (!cached) this.monitor.reset();
    this.bindEvents();
    this.roomEditor = new RoomEditor((saved) => this.applyRoom(saved));
    this.languageDialog = new LanguageDialog(
      (source, target) => this.persistLanguages(source, target),
      this.onError,
    );
    refreshIcons(root);

    this.unsubscribePreferences = subscribePreferences(this.userId, (next) => {
      this.voice = next.voice;
      this.enhancedVoiceFilter = next.enhancedVoiceFilter;
      this.microphoneCapture = next.microphoneCapture;
      this.systemAudioCapture = next.systemAudioCapture && supportsSystemAudioCapture();
      this.noiseSuppression = next.noiseSuppression;
      this.echoCancellation = next.echoCancellation;
      this.captureOptions?.setValues(this.captureOptionValues());
      this.syncRecordButton();
      this.player.muted = !next.autoplay;
      this.voiceSession?.sendConfig();
    });
    this.unsubscribeSubtitlePreferences = subscribeSubtitlePreferences(this.userId, () => {
      this.pushAndroidSubtitle();
    });
    this.unsubscribeAndroidNative = subscribeAndroidNative((event) => {
      if (event.type === 'overlay-opened') this.androidOverlayActive = true;
      if (event.type === 'overlay-closed') this.androidOverlayActive = false;
      if (event.type === 'overlay-error') this.onError(event.message);
    });
    this.androidOverlayActive = isAndroidSubtitleOverlayVisible();
    this.pushAndroidSubtitle();
    window.setTimeout(() => void this.loadHistory(this.historyQuery, true), 0);
    if (detail.room.status !== 'active') {
      root.querySelector<HTMLButtonElement>('.record-button')!.disabled = true;
      this.setCaptureStatus('会议已结束', 'neutral');
      root.querySelector<HTMLButtonElement>('.open-subtitles')!.disabled = true;
      this.onConnection('hidden');
      return;
    }
    this.voiceSession = new VoiceSession(
      detail.room.id,
      this.canPublish,
      root.querySelector<HTMLCanvasElement>('#waveform')!,
      this.player,
      () => this.sessionConfig(),
      () => this.enhancedVoiceFilter,
      () => this.captureOptionValues(),
      {
        onEvent: (event) => this.handleEvent(event),
        onConnection: (status) => this.setConnection(status),
        onRecording: (recording) => this.setRecording(recording),
        onCaptureError: this.onError,
      },
    );
    this.voiceSession.connect();
  }

  async destroy() {
    const query = this.root?.querySelector<HTMLInputElement>('.record-search input')?.value ?? '';
    const scrollTop = this.root?.querySelector<HTMLElement>('.conversation-list')?.scrollTop ?? 0;
    translatorViewStates.set(this.cacheKey(), { query, scrollTop });
    this.cacheCurrentState();
    window.clearInterval(this.timer);
    this.unsubscribePreferences();
    this.unsubscribePreferences = () => {};
    this.unsubscribeSubtitlePreferences();
    this.unsubscribeSubtitlePreferences = () => {};
    this.unsubscribeAndroidNative();
    this.unsubscribeAndroidNative = () => {};
    this.historyRequestVersion += 1;
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
    this.captureOptions?.destroy();
    this.captureOptions = null;
    this.player.stop();
    this.onConnection('hidden');
    this.root = null;
  }

  private async getDetail(search = '') {
    const query = `?include_history=false${search.trim() ? `&q=${encodeURIComponent(search.trim())}` : ''}`;
    try {
      return await apiRequest<RoomDetail>(`/api/rooms/${this.roomId}${query}`);
    } catch (error) {
      if (error instanceof ApiRequestError && error.status === 403 && !search) {
        await apiRequest(`/api/rooms/${this.roomId}/join`, { method: 'POST' });
        return apiRequest<RoomDetail>(`/api/rooms/${this.roomId}?include_history=false`);
      }
      throw error;
    }
  }

  private template(room: RoomSummary) {
    const androidShell = isAndroidNativeShell();
    const captureConsole = `
      <div class="capture-console">
        <div class="record-control">
          <div class="record-primary-actions">
            <button class="record-button" type="button" aria-label="开始麦克风录音" title="开始麦克风录音" disabled><i data-lucide="mic"></i></button>
            <div class="capture-options-mount"></div>
          </div>
          <span class="record-button-copy">开始录音</span>
          <div class="capture-readout" data-tone="processing">
            <span class="capture-status-line"><i class="capture-status-dot" aria-hidden="true"></i><strong class="capture-state">连接中</strong></span>
            <span class="capture-time">00:00</span>
          </div>
          <canvas id="waveform" aria-hidden="true"></canvas>
        </div>
      </div>
    `;
    return `
      <main class="translator-page app-shell${androidShell ? ' android-native-shell' : ''}">
        <section class="room-toolbar">
          <button class="room-back icon-button" type="button" title="返回房间目录" aria-label="返回房间目录"><i data-lucide="arrow-left"></i></button>
          <div class="room-identity-static">
            <span class="room-status-icon"><i data-lucide="door-open"></i></span>
            <span><small>当前房间</small><strong class="room-name">${escapeHtml(room.name)}</strong></span>
            <span class="room-role${room.is_owner ? ' owner' : ''}">${room.status !== 'active' ? '会议已结束' : room.is_owner ? '房主控制' : this.canPublish ? '成员发言' : '已被禁言'}</span>
          </div>
          <div class="room-toolbar-actions" aria-label="会议操作">
            <button class="room-command open-record-search" type="button"><i data-lucide="search"></i><span>记录</span></button>
            <button class="room-command open-subtitles" type="button"><i data-lucide="captions"></i><span>字幕</span></button>
            <button class="room-command open-room-management" type="button" ${room.is_owner ? '' : 'hidden'}><i data-lucide="settings-2"></i><span>管理</span></button>
          </div>
        </section>

        <dialog class="room-action-dialog record-search-dialog">
          <form class="record-search">
            <header><div><small>TRANSCRIPTS</small><h2>检索会议记录</h2></div><button class="icon-button close-record-search" type="button" title="关闭" aria-label="关闭"><i data-lucide="x"></i></button></header>
            <label><span>原文或译文</span><span class="room-action-input"><i data-lucide="search"></i><input type="search" placeholder="输入关键词" aria-label="检索房间记录"></span></label>
            <div class="room-action-footer"><button class="secondary-command clear-record-search" type="button">清除</button><button class="primary-command" type="submit">检索记录</button></div>
          </form>
        </dialog>

        <dialog class="room-action-dialog room-management-dialog">
          <section>
            <header><div><small>ROOM CONTROL</small><h2>房间管理</h2></div><button class="icon-button close-room-management" type="button" title="关闭" aria-label="关闭"><i data-lucide="x"></i></button></header>
            <button class="room-action-item edit-room" type="button"><span class="room-action-icon"><i data-lucide="settings"></i></span><span><strong>会议设置</strong><small>名称、语言和断句时间</small></span><i data-lucide="chevron-right"></i></button>
            <button class="room-action-item danger delete-room" type="button"><span class="room-action-icon"><i data-lucide="trash-2"></i></span><span><strong>删除会议</strong><small>从会议目录移除，历史数据仍保留</small></span><i data-lucide="chevron-right"></i></button>
          </section>
        </dialog>

        <div class="workspace-grid">
          <section class="conversation-panel">
            <div class="panel-heading">
              <div><span class="section-kicker"><i data-lucide="radio"></i> LIVE SESSION</span><h1>实时对话</h1></div>
              <div class="panel-actions">
                <div class="monitor-mount"></div>
                <button class="icon-button refresh-history" type="button" title="刷新记录" aria-label="刷新记录"><i data-lucide="refresh-cw"></i></button>
              </div>
            </div>
            <div class="conversation-mount"></div>
            ${androidShell ? '' : captureConsole}
          </section>
          <aside class="member-panel" aria-label="房间成员">
            <div class="member-panel-heading"><div><span class="section-kicker"><i data-lucide="users"></i> PARTICIPANTS</span><h2>房间成员</h2></div><span class="member-count">0 人</span></div>
            <div class="member-list"></div>
          </aside>
        </div>
        ${androidShell ? captureConsole : ''}
      </main>
    `;
  }

  private bindEvents() {
    if (!this.root || !this.room) return;
    this.root.querySelector('.room-back')?.addEventListener('click', this.onRooms);
    const searchDialog = this.root.querySelector<HTMLDialogElement>('.record-search-dialog')!;
    const managementDialog = this.root.querySelector<HTMLDialogElement>('.room-management-dialog')!;
    this.root.querySelector('.open-record-search')?.addEventListener('click', () => {
      searchDialog.showModal();
      searchDialog.querySelector<HTMLInputElement>('input')?.focus();
    });
    this.root.querySelector('.close-record-search')?.addEventListener('click', () => searchDialog.close());
    this.root.querySelector('.clear-record-search')?.addEventListener('click', () => {
      const input = searchDialog.querySelector<HTMLInputElement>('input');
      if (input) input.value = '';
      void this.loadHistory('', true);
      searchDialog.close();
    });
    this.root.querySelector('.open-room-management')?.addEventListener('click', () => managementDialog.showModal());
    this.root.querySelector('.close-room-management')?.addEventListener('click', () => managementDialog.close());
    [searchDialog, managementDialog].forEach((dialog) => dialog.addEventListener('click', (event) => {
      if (event.target === dialog) dialog.close();
    }));
    this.root.querySelector('.record-button')?.addEventListener('click', () =>
      void this.voiceSession?.toggleRecording().catch((error) =>
        this.onError(error instanceof Error ? error.message : '无法启动录音'),
      ),
    );
    this.root.querySelector('.refresh-history')?.addEventListener('click', () =>
      void this.loadHistory(this.historyQuery, true),
    );
    this.root.querySelector('.open-subtitles')?.addEventListener('click', () =>
      void this.openSubtitleDisplay(),
    );
    this.root.querySelector<HTMLFormElement>('.record-search')?.addEventListener('submit', (event) => {
      event.preventDefault();
      const query = this.root?.querySelector<HTMLInputElement>('.record-search input')?.value ?? '';
      translatorViewStates.set(this.cacheKey(), { query, scrollTop: 0 });
      void this.loadHistory(query, true);
      searchDialog.close();
    });
    this.root.querySelector('.edit-room')?.addEventListener('click', () => {
      managementDialog.close();
      if (this.room) this.roomEditor?.open(this.room);
    });
    this.root.querySelector('.delete-room')?.addEventListener('click', () => {
      managementDialog.close();
      void this.deleteRoom();
    });
    this.root.querySelector('.member-list')?.addEventListener('click', (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-mute-user]');
      if (button) void this.toggleMemberMute(button);
    });
  }

  private sessionConfig(): SessionConfig {
    return {
      source_language: this.sourceLanguage,
      target_language: this.targetLanguage,
      voice: this.voice,
      max_utterance_seconds: this.maxUtteranceSeconds,
    };
  }

  private captureOptionValues(): CaptureOptionValues {
    return {
      microphone: this.microphoneCapture,
      systemAudio: this.systemAudioCapture,
      noiseSuppression: this.noiseSuppression,
      echoCancellation: this.echoCancellation,
    };
  }

  private updateCaptureOptions(values: CaptureOptionValues) {
    this.microphoneCapture = values.microphone;
    this.systemAudioCapture = values.systemAudio;
    this.noiseSuppression = values.noiseSuppression;
    this.echoCancellation = values.echoCancellation;
    savePreferences(this.userId, {
      ...loadPreferences(this.userId),
      microphoneCapture: values.microphone,
      systemAudioCapture: values.systemAudio,
      noiseSuppression: values.noiseSuppression,
      echoCancellation: values.echoCancellation,
    });
    this.syncRecordButton();
    if (!this.recording) return;
    if (!values.microphone && !values.systemAudio) {
      void this.voiceSession?.toggleRecording().catch((error) =>
        this.onError(error instanceof Error ? error.message : '无法停止录音'),
      );
      return;
    }
    void this.voiceSession?.reconfigureCapture();
  }

  private handleEvent(event: ServerEvent) {
    if (!this.root) return;
    switch (event.type) {
      case 'room_subscribed':
        this.monitor?.setBackend(event.backend);
        this.updatePublishPermission(event.can_publish);
        void this.reconcileHistory();
        break;
      case 'room_members': {
        this.members = event.members;
        this.renderMembers();
        this.cacheCurrentState();
        const self = event.members.find((member) => member.user_id === this.userId);
        this.updatePublishPermission(Boolean(self && (self.is_owner || !self.is_muted)));
        break;
      }
      case 'ready':
        this.monitor?.setBackend(event.backend);
        break;
      case 'configured':
        this.sourceLanguage = event.source_language;
        this.targetLanguage = event.target_language;
        this.maxUtteranceSeconds = event.max_utterance_seconds;
        this.updateLanguageButton();
        break;
      case 'state':
        this.setPhase(event.phase);
        if (event.phase === 'transcribing' && event.utterance_id) {
          this.conversation?.upsertTranscript(
            { utterance_id: event.utterance_id, text: '', language: this.sessionConfig().source_language },
            true,
          );
        }
        if (event.phase === 'synthesizing' && event.utterance_id) {
          this.conversation?.markTtsGenerating(event.utterance_id);
        }
        break;
      case 'utterance_queued':
        // Create the row before the first ASR token can arrive. This also keeps consecutive
        // VAD segments independent while earlier segments translate or synthesize.
        this.conversation?.upsertTranscript(
          {
            utterance_id: event.utterance_id,
            text: '',
            language: this.sessionConfig().source_language,
          },
          true,
        );
        this.ensureAndroidCaption(event.utterance_id);
        break;
      case 'utterance_discarded':
        this.conversation?.removeUtterance(event.utterance_id);
        this.removeAndroidCaption(event.utterance_id);
        break;
      case 'utterance_speakers':
        this.conversation?.applySpeakers(event.utterance_id, event.speakers);
        break;
      case 'recognition_failed':
        this.conversation?.markRecognitionFailed(event.utterance_id, event.message);
        break;
      case 'processing_failed':
        this.conversation?.markProcessingFailed(event.utterance_id, event.stage, event.message);
        break;
      case 'transcript':
        this.conversation?.upsertTranscript(event, false);
        this.setAndroidCaption(event.utterance_id, 'source', event.text);
        break;
      case 'transcript_delta':
        this.conversation?.applyTranscriptDelta(event);
        this.setAndroidCaption(event.utterance_id, 'source', event.text);
        break;
      case 'transcript_refinement':
        this.conversation?.applyRefinement(event);
        if (event.status === 'completed' && event.text) {
          this.setAndroidCaption(event.utterance_id, 'source', event.text);
        }
        break;
      case 'translation_delta':
        this.conversation?.applyTranslationDelta(event);
        this.setAndroidCaption(event.utterance_id, 'translation', event.text);
        break;
      case 'translation':
        this.conversation?.applyTranslation(event);
        this.setAndroidCaption(event.utterance_id, 'source', event.source_text);
        this.setAndroidCaption(event.utterance_id, 'translation', event.translated_text);
        break;
      case 'media':
        this.conversation?.applyMedia(event);
        break;
      case 'latency':
        this.addLatency(event.utterance_id, event.latency);
        this.conversation?.updateItemLatency(event.utterance_id, event.latency.total_ms);
        break;
      case 'warning':
        this.onError(event.message);
        break;
      default:
        break;
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
    if (this.recording) {
      this.setCaptureStatus(phase === 'speech' ? labels.speech : '持续聆听', 'recording');
    } else {
      this.setCaptureStatus(labels[phase], phase === 'listening' || phase === 'speech' ? 'recording' : 'processing');
    }
    this.monitor?.setPhase(phase);
  }

  private setConnection(status: ConnectionStatus) {
    this.connectionStatus = status;
    this.onConnection(!this.canPublish && status === 'connected' ? 'viewer' : status);
    if (!this.root) return;
    this.syncRecordButton();
    if (this.recording) return;
    if (status === 'connected') this.setCaptureStatus(this.canPublish ? '可以开始录音' : '已被禁言', this.canPublish ? 'ready' : 'neutral');
    else this.setCaptureStatus(status === 'connecting' ? '连接实时会话' : '连接已中断', 'processing');
  }

  private setRecording(recording: boolean) {
    if (!this.root) return;
    this.recording = recording;
    const button = this.root.querySelector<HTMLButtonElement>('.record-button')!;
    const console = this.root.querySelector<HTMLElement>('.capture-console')!;
    button.classList.toggle('recording', recording);
    this.captureOptions?.setRecording(recording);
    console.classList.toggle('is-recording', recording);
    this.root.querySelector('.translator-page')?.classList.toggle('is-recording', recording);
    this.setCaptureStatus(recording ? '持续聆听' : this.canPublish ? '可以开始录音' : '已被禁言', recording ? 'recording' : this.canPublish ? 'ready' : 'neutral');
    this.syncRecordButton();
    window.clearInterval(this.timer);
    if (recording) {
      this.startedAt = Date.now();
      this.timer = window.setInterval(() => this.updateTimer(), 250);
    } else {
      this.root.querySelector('.capture-time')!.textContent = '00:00';
    }
  }

  private syncRecordButton() {
    if (!this.root) return;
    const button = this.root.querySelector<HTMLButtonElement>('.record-button');
    const copy = this.root.querySelector<HTMLElement>('.record-button-copy');
    if (!button || !copy) return;
    const visibleOptions = this.captureOptions?.values();
    const microphone = visibleOptions?.microphone ?? this.microphoneCapture;
    const systemAudio = visibleOptions?.systemAudio ?? this.systemAudioCapture;
    const hasSource = microphone || systemAudio;
    const mode = microphone && systemAudio
      ? '混合录音'
      : systemAudio
        ? '系统内录'
        : '麦克风录音';
    const title = this.recording ? '停止录音' : hasSource ? `开始${mode}` : '选择音频来源';
    const icon = this.recording
      ? 'circle-stop'
      : microphone && systemAudio
        ? 'audio-lines'
        : systemAudio
          ? 'volume-2'
          : microphone
            ? 'mic'
            : 'mic-off';
    button.disabled = !this.recording && (!hasSource || !this.canPublish);
    button.innerHTML = `<i data-lucide="${icon}"></i>`;
    button.title = title;
    button.ariaLabel = title;
    copy.textContent = title;
    refreshIcons(button);
  }

  private updateTimer() {
    const seconds = Math.floor((Date.now() - this.startedAt) / 1000);
    this.root!.querySelector('.capture-time')!.textContent =
      `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
  }

  private setCaptureStatus(label: string, tone: 'ready' | 'recording' | 'processing' | 'neutral') {
    if (!this.root) return;
    const readout = this.root.querySelector<HTMLElement>('.capture-readout');
    const state = this.root.querySelector<HTMLElement>('.capture-state');
    if (!readout || !state) return;
    readout.dataset.tone = tone;
    state.textContent = label;
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
    this.cacheCurrentState();
  }

  private async openSubtitleDisplay() {
    const path = `/rooms/${this.roomId}/subtitles`;
    if (isAndroidNativeShell()) {
      try {
        await showAndroidSubtitleOverlay(this.androidSubtitlePayload());
        this.androidOverlayActive = true;
        this.pushAndroidSubtitle();
      } catch (error) {
        this.onError(error instanceof Error ? error.message : '无法创建字幕悬浮窗');
      }
      return;
    }
    if (!this.isAppShell) {
      const display = window.open(
        path,
        `voice-elf-subtitles-${this.roomId}`,
        'popup,width=1100,height=460,resizable=yes',
      );
      if (!display) this.onError('浏览器阻止了字幕窗口，请允许本站打开弹窗');
      else display.focus();
      return;
    }
    try {
      const response = await fetch('/__voice_elf/subtitle-window', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ room_id: this.roomId }),
      });
      if (!response.ok) {
        const payload = (await response.json().catch(() => null)) as { error?: string } | null;
        throw new Error(payload?.error ?? '无法创建字幕悬浮窗');
      }
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法创建字幕悬浮窗');
    }
  }

  private ensureAndroidCaption(id: string) {
    let caption = this.androidCaptions.get(id);
    if (!caption) {
      caption = { source: '', translation: '' };
      this.androidCaptions.set(id, caption);
      this.androidCaptionOrder = [...this.androidCaptionOrder.filter((item) => item !== id), id].slice(-3);
    }
    return caption;
  }

  private setAndroidCaption(id: string, kind: 'source' | 'translation', text: string) {
    this.ensureAndroidCaption(id)[kind] = text;
    this.pushAndroidSubtitle();
  }

  private removeAndroidCaption(id: string) {
    this.androidCaptions.delete(id);
    this.androidCaptionOrder = this.androidCaptionOrder.filter((item) => item !== id);
    this.pushAndroidSubtitle();
  }

  private seedAndroidCaptions(utterances: UtteranceHistory[]) {
    if (this.androidCaptionOrder.length > 0) return;
    utterances
      .filter((utterance) => utterance.source_text || utterance.translated_text)
      .slice(0, 3)
      .reverse()
      .forEach((utterance) => {
        this.androidCaptions.set(utterance.id, {
          source: utterance.source_text,
          translation: utterance.translated_text,
        });
        this.androidCaptionOrder.push(utterance.id);
      });
    this.pushAndroidSubtitle();
  }

  private androidSubtitlePayload(): AndroidSubtitlePayload {
    const latest = [...this.androidCaptionOrder]
      .reverse()
      .map((id) => this.androidCaptions.get(id))
      .find((caption) => caption?.source || caption?.translation);
    const preferences = loadSubtitlePreferences(this.userId);
    return {
      roomId: this.roomId,
      roomName: this.room?.name ?? '实时字幕',
      source: latest?.source ?? '',
      translation: latest?.translation ?? '',
      sourceVisible: preferences.displayMode !== 'translation',
      translationVisible: preferences.displayMode !== 'source',
      backgroundColor: preferences.backgroundColor,
      sourceColor: preferences.sourceColor,
      translationColor: preferences.translationColor,
    };
  }

  private pushAndroidSubtitle() {
    if (this.androidOverlayActive) updateAndroidSubtitleOverlay(this.androidSubtitlePayload());
  }

  private loadHistory(search = '', reset = true) {
    const normalizedSearch = search.trim();
    if (this.historySync && normalizedSearch === this.historyQuery) return this.historySync;
    const requestVersion = ++this.historyRequestVersion;
    const page = reset || normalizedSearch !== this.historyQuery ? 1 : this.historyPage + 1;
    this.conversation?.setHistoryPagination(this.historyPage * 30, this.historyTotal || 1, true);
    const params = new URLSearchParams({ page: String(page), page_size: '30' });
    if (normalizedSearch) params.set('q', normalizedSearch);
    const sync = apiRequest<Paginated<UtteranceHistory>>(
      `/api/rooms/${this.roomId}/utterances?${params}`,
    )
      .then((result) => {
        if (requestVersion !== this.historyRequestVersion || !this.conversation) return;
        this.historyQuery = normalizedSearch;
        this.historyPage = result.page;
        this.historyTotal = result.total;
        if (page === 1) {
          this.historyRecords = result.items;
          if (!normalizedSearch) this.seedAndroidCaptions(result.items);
          this.conversation.renderUtterances(result.items);
          this.resetLatency(result.items);
          if (this.pendingHistoryScroll !== null) {
            const scrollTop = this.pendingHistoryScroll;
            this.pendingHistoryScroll = null;
            requestAnimationFrame(() => {
              const list = this.root?.querySelector<HTMLElement>('.conversation-list');
              if (list) list.scrollTop = scrollTop;
            });
          }
        } else {
          const known = new Set(this.historyRecords.map((utterance) => utterance.id));
          this.historyRecords = [
            ...this.historyRecords,
            ...result.items.filter((utterance) => !known.has(utterance.id)),
          ];
          this.conversation.prependHistory(result.items);
          result.items.forEach((utterance) => this.addLatency(utterance.id, utterance.latency));
        }
        this.conversation.setHistoryPagination(
          Math.min(result.page * result.page_size, result.total),
          result.total,
          false,
        );
        this.cacheCurrentState();
      })
      .catch((error) => {
        if (requestVersion !== this.historyRequestVersion) return;
        this.conversation?.setHistoryPagination(
          Math.min(this.historyPage * 30, this.historyTotal),
          this.historyTotal,
          false,
        );
        this.onError(error instanceof Error ? error.message : '无法加载记录');
      })
      .finally(() => {
        if (this.historySync === sync) this.historySync = null;
      });
    this.historySync = sync;
    return sync;
  }

  private reconcileHistory() {
    return this.loadHistory(this.historyQuery, true);
  }

  private resetLatency(utterances: UtteranceHistory[]) {
    this.monitor?.reset();
    this.latencyIds.clear();
    utterances
      .slice()
      .reverse()
      .forEach((utterance) => this.addLatency(utterance.id, utterance.latency));
  }

  private addLatency(utteranceId: string, latency: UtteranceHistory['latency']) {
    if (this.latencyIds.has(utteranceId)) return;
    this.latencyIds.add(utteranceId);
    this.monitor?.addLatency(latency);
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
    this.cacheCurrentState();
  }

  private cacheCurrentState() {
    if (!this.room) return;
    translatorCache.set(this.cacheKey(), {
      detail: { room: this.room, members: this.members, utterances: [] },
      utterances: this.historyRecords,
      historyPage: this.historyPage,
      historyTotal: this.historyTotal,
      historyQuery: this.historyQuery,
    });
  }

  private cacheKey() {
    return `${this.userId}:${this.roomId}`;
  }

  private updateLanguageButton() {
    this.captureOptions?.setLanguages(
      languageNames[this.sourceLanguage] ?? this.sourceLanguage,
      languageNames[this.targetLanguage] ?? this.targetLanguage,
      Boolean(this.room?.is_owner),
    );
  }

  private updatePublishPermission(canPublish: boolean) {
    this.canPublish = canPublish;
    this.voiceSession?.setCanPublish(canPublish);
    if (!this.root) return;
    this.syncRecordButton();
    const role = this.root.querySelector<HTMLElement>('.room-role');
    if (role && !this.room?.is_owner) {
      role.textContent = canPublish ? '成员发言' : '已被禁言';
      role.classList.toggle('muted', !canPublish);
    }
    if (!this.recording && this.connectionStatus === 'connected') {
      this.setCaptureStatus(canPublish ? '可以开始录音' : '已被禁言', canPublish ? 'ready' : 'neutral');
    }
  }

  private renderMembers() {
    if (!this.root) return;
    const list = this.root.querySelector<HTMLElement>('.member-list');
    if (!list) return;
    const ordered = this.members
      .slice()
      .sort((left, right) => Number(right.is_owner) - Number(left.is_owner) || left.username.localeCompare(right.username));
    list.innerHTML = ordered
      .map((member) => {
        const status = member.is_speaking
          ? '正在发言'
          : member.is_muted
            ? '已禁言'
            : member.is_online
              ? '在线'
              : '离线';
        const control = this.room?.is_owner && !member.is_owner
          ? `<button class="icon-button member-mute" type="button" data-mute-user="${member.user_id}" data-muted="${member.is_muted}" title="${member.is_muted ? '允许发言' : '禁言'}" aria-label="${member.is_muted ? '允许发言' : '禁言'}"><i data-lucide="${member.is_muted ? 'mic' : 'mic-off'}"></i></button>`
          : '';
        return `<div class="member-row${member.is_speaking ? ' speaking' : ''}${member.is_muted ? ' muted' : ''}${member.is_online ? ' online' : ''}">
          <span class="member-avatar">${escapeHtml(member.username.slice(0, 1).toUpperCase())}</span>
          <span class="member-copy"><strong>${escapeHtml(member.username)}${member.user_id === this.userId ? '<small>你</small>' : ''}</strong><span><i></i>${member.is_owner ? '房主 · ' : ''}${status}</span></span>
          ${control}
        </div>`;
      })
      .join('');
    const count = this.root.querySelector<HTMLElement>('.member-count');
    if (count) count.textContent = `${this.members.filter((member) => member.is_online).length}/${this.members.length} 在线`;
    refreshIcons(list);
  }

  private async toggleMemberMute(button: HTMLButtonElement) {
    if (!this.room?.is_owner || button.disabled) return;
    const userId = button.dataset.muteUser;
    if (!userId) return;
    const isMuted = button.dataset.muted === 'true';
    button.disabled = true;
    try {
      const updated = await apiRequest<RoomMemberState>(
        `/api/rooms/${this.room.id}/members/${userId}`,
        { method: 'PATCH', body: JSON.stringify({ is_muted: !isMuted }) },
      );
      this.members = this.members.map((member) => member.user_id === updated.user_id ? updated : member);
      this.renderMembers();
    } catch (error) {
      button.disabled = false;
      this.onError(error instanceof Error ? error.message : '无法更新发言权限');
    }
  }

  private async deleteRoom() {
    if (
      !this.room?.is_owner ||
      !window.confirm(`确认删除会议“${this.room.name}”？\n\n会议将从列表移除，历史数据不会被物理删除。`)
    ) return;
    try {
      await apiRequest(`/api/rooms/${this.room.id}`, { method: 'DELETE' });
      translatorCache.delete(this.cacheKey());
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

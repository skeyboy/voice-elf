import {
  ApiRequestError,
  apiRequest,
  type RoomDetail,
  type RoomInput,
  type RoomMemberState,
  type RoomSummary,
} from '../api';
import { loadAppConfig } from '../app-config';
import { PcmPlayer } from '../audio';
import { ConversationView } from '../components/conversation-view';
import { LanguageDialog } from '../components/language-dialog';
import { refreshIcons } from '../components/icons';
import { LatencyMonitor } from '../components/latency-monitor';
import { RoomEditor } from '../components/room-editor';
import type { ConnectionStatus } from '../components/topbar';
import { VoiceSession } from '../controllers/voice-session';
import type { PipelinePhase, ServerEvent, SessionConfig } from '../protocol';
import { languageNames } from '../shared/languages';
import { loadPreferences, savePreferences, subscribePreferences } from '../shared/preferences';
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
  private timer = 0;
  private startedAt = 0;
  private recording = false;
  private sourceLanguage = 'auto';
  private targetLanguage = 'zh';
  private maxUtteranceSeconds = 20;
  private voice = 'F1';
  private enhancedVoiceFilter = true;
  private noiseSuppression = true;
  private echoCancellation = true;
  private readonly latencyIds = new Set<string>();
  private historySync: Promise<void> | null = null;
  private members: RoomMemberState[] = [];
  private canPublish = false;
  private connectionStatus: ConnectionStatus = 'offline';
  private isAppShell = false;
  private unsubscribePreferences = () => {};

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
    this.noiseSuppression = preferences.noiseSuppression;
    this.echoCancellation = preferences.echoCancellation;
    this.player.muted = !preferences.autoplay;
    root.innerHTML = this.template(detail.room);
    this.renderMembers();
    this.conversation = new ConversationView(this.player, this.onError, (active) =>
      this.voiceSession?.setExternalPlaybackActive(active),
    );
    this.monitor = new LatencyMonitor();
    root.querySelector('.conversation-mount')!.replaceWith(this.conversation.element);
    root.querySelector('.monitor-mount')!.replaceWith(this.monitor.element);
    this.conversation.renderHistory(detail);
    this.resetLatency(detail);
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
      this.noiseSuppression = next.noiseSuppression;
      this.echoCancellation = next.echoCancellation;
      const noiseToggle = this.root?.querySelector<HTMLInputElement>('.capture-noise-suppression');
      const echoToggle = this.root?.querySelector<HTMLInputElement>('.capture-echo-cancellation');
      if (noiseToggle) noiseToggle.checked = next.noiseSuppression;
      if (echoToggle) echoToggle.checked = next.echoCancellation;
      this.player.muted = !next.autoplay;
      this.voiceSession?.sendConfig();
    });
    if (detail.room.status !== 'active') {
      root.querySelector<HTMLButtonElement>('.record-button')!.disabled = true;
      root.querySelector<HTMLElement>('.capture-state')!.textContent = '会议已结束';
      root.querySelector<HTMLElement>('.session-badge-text')!.textContent = '仅查看记录';
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
      () => this.noiseSuppression,
      () => this.echoCancellation,
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
    window.clearInterval(this.timer);
    this.unsubscribePreferences();
    this.unsubscribePreferences = () => {};
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
            <span class="room-role${room.is_owner ? ' owner' : ''}">${room.status !== 'active' ? '会议已结束' : room.is_owner ? '房主控制' : this.canPublish ? '成员发言' : '已被禁言'}</span>
          </div>
          <form class="record-search">
            <i data-lucide="search"></i><input type="search" placeholder="检索转写或翻译记录" aria-label="检索房间记录"><button type="submit">检索</button>
          </form>
          <div class="room-display-actions">
            <button class="icon-button open-subtitles" type="button" title="打开字幕大屏" aria-label="打开字幕大屏"><i data-lucide="captions"></i></button>
            <div class="room-owner-actions" ${room.is_owner ? '' : 'hidden'}>
              <button class="icon-button edit-room" type="button" title="编辑房间" aria-label="编辑房间"><i data-lucide="settings"></i></button>
              <button class="icon-button danger delete-room" type="button" title="删除房间" aria-label="删除房间"><i data-lucide="trash-2"></i></button>
            </div>
          </div>
        </section>

        <div class="workspace-grid">
          <section class="conversation-panel">
            <div class="panel-heading">
              <div><span class="section-kicker"><i data-lucide="radio"></i> LIVE SESSION</span><h1>实时对话</h1></div>
              <div class="panel-actions">
                <span class="capture-session-badge"><span class="session-dot"></span><span class="session-badge-text">${this.canPublish ? '等待录音' : '已被禁言'}</span></span>
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
              <div class="record-control">
                <button class="record-button" type="button" aria-label="开始录音" title="开始录音" disabled><i data-lucide="mic"></i></button>
                <span class="record-button-copy">开始录音</span>
                <div class="capture-processing-options">
                  <label class="capture-processing-option">
                    <input class="capture-noise-suppression" type="checkbox" role="switch" ${this.noiseSuppression ? 'checked' : ''}>
                    <span class="capture-processing-track" aria-hidden="true"></span>
                    <span class="capture-processing-copy"><strong>系统降噪</strong><small>下次录音生效</small></span>
                  </label>
                  <label class="capture-processing-option">
                    <input class="capture-echo-cancellation" type="checkbox" role="switch" ${this.echoCancellation ? 'checked' : ''}>
                    <span class="capture-processing-track" aria-hidden="true"></span>
                    <span class="capture-processing-copy"><strong>回声消除</strong><small>下次录音生效</small></span>
                  </label>
                </div>
              </div>
              <div class="capture-readout"><strong class="capture-state">连接中</strong><span class="capture-time">00:00</span></div>
            </div>
          </section>
          <aside class="member-panel" aria-label="房间成员">
            <div class="member-panel-heading"><div><span class="section-kicker"><i data-lucide="users"></i> PARTICIPANTS</span><h2>房间成员</h2></div><span class="member-count">0 人</span></div>
            <div class="member-list"></div>
          </aside>
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
    this.root.querySelector<HTMLInputElement>('.capture-noise-suppression')?.addEventListener('change', (event) => {
      const preferences = loadPreferences(this.userId);
      savePreferences(this.userId, {
        ...preferences,
        noiseSuppression: (event.currentTarget as HTMLInputElement).checked,
      });
    });
    this.root.querySelector<HTMLInputElement>('.capture-echo-cancellation')?.addEventListener('change', (event) => {
      const preferences = loadPreferences(this.userId);
      savePreferences(this.userId, {
        ...preferences,
        echoCancellation: (event.currentTarget as HTMLInputElement).checked,
      });
    });
    this.root.querySelector('.language-config-button')?.addEventListener('click', () =>
      this.languageDialog?.open(this.sourceLanguage, this.targetLanguage),
    );
    this.root.querySelector('.refresh-history')?.addEventListener('click', () => void this.loadHistory());
    this.root.querySelector('.open-subtitles')?.addEventListener('click', () =>
      void this.openSubtitleDisplay(),
    );
    this.root.querySelector<HTMLFormElement>('.record-search')?.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.loadHistory(this.root?.querySelector<HTMLInputElement>('.record-search input')?.value ?? '');
    });
    this.root.querySelector('.edit-room')?.addEventListener('click', () => {
      if (this.room) this.roomEditor?.open(this.room);
    });
    this.root.querySelector('.delete-room')?.addEventListener('click', () => void this.deleteRoom());
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
        break;
      case 'utterance_discarded':
        this.conversation?.removeUtterance(event.utterance_id);
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
        break;
      case 'transcript_delta':
        this.conversation?.applyTranscriptDelta(event);
        break;
      case 'transcript_refinement':
        this.conversation?.applyRefinement(event);
        break;
      case 'translation_delta':
        this.conversation?.applyTranslationDelta(event);
        break;
      case 'translation':
        this.conversation?.applyTranslation(event);
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
    if (!this.recording || phase === 'listening' || phase === 'speech') {
      this.root!.querySelector('.capture-state')!.textContent =
        this.recording && phase === 'listening' ? '持续聆听' : labels[phase];
    }
    this.monitor?.setPhase(phase);
  }

  private setConnection(status: ConnectionStatus) {
    this.connectionStatus = status;
    this.onConnection(!this.canPublish && status === 'connected' ? 'viewer' : status);
    if (!this.root) return;
    this.root.querySelector<HTMLButtonElement>('.record-button')!.disabled =
      !this.canPublish || status !== 'connected';
    if (status === 'connected') {
      this.root.querySelector('.capture-state')!.textContent = this.canPublish ? '就绪' : '已被禁言';
    } else {
      this.root.querySelector('.capture-state')!.textContent =
        status === 'connecting' ? '连接实时会话' : '连接已中断';
    }
  }

  private setRecording(recording: boolean) {
    if (!this.root) return;
    this.recording = recording;
    const button = this.root.querySelector<HTMLButtonElement>('.record-button')!;
    const console = this.root.querySelector<HTMLElement>('.capture-console')!;
    const badge = this.root.querySelector<HTMLElement>('.capture-session-badge')!;
    button.classList.toggle('recording', recording);
    this.root.querySelectorAll<HTMLInputElement>('.capture-processing-option input').forEach((toggle) => {
      toggle.disabled = recording;
    });
    console.classList.toggle('is-recording', recording);
    badge.classList.toggle('active', recording);
    this.root.querySelector('.translator-page')?.classList.toggle('is-recording', recording);
    this.root.querySelector('.session-badge-text')!.textContent = recording
      ? '连续录音中'
      : this.canPublish
        ? '等待录音'
        : '已被禁言';
    this.root.querySelector('.capture-state')!.textContent = recording
      ? '持续聆听'
      : this.canPublish
        ? '就绪'
        : '已被禁言';
    button.innerHTML = `<i data-lucide="${recording ? 'circle-stop' : 'mic'}"></i>`;
    button.title = recording ? '停止录音' : '开始录音';
    button.ariaLabel = button.title;
    this.root.querySelector('.record-button-copy')!.textContent = button.title;
    this.root.querySelectorAll('.capture-processing-copy small').forEach((description) => {
      description.textContent = recording ? '停止后可切换' : '下次录音生效';
    });
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

  private async openSubtitleDisplay() {
    const path = `/rooms/${this.roomId}/subtitles`;
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

  private async loadHistory(search = '') {
    try {
      const detail = await this.getDetail(search);
      this.members = detail.members;
      this.renderMembers();
      this.conversation?.renderHistory(detail);
      this.resetLatency(detail);
    } catch (error) {
      this.onError(error instanceof Error ? error.message : '无法加载记录');
    }
  }

  private reconcileHistory() {
    if (this.historySync) return this.historySync;
    this.historySync = this.getDetail()
      .then((detail) => {
        this.members = detail.members;
        this.renderMembers();
        this.conversation?.mergeHistory(detail);
        detail.utterances
          .slice()
          .reverse()
          .forEach((utterance) => this.addLatency(utterance.id, utterance.latency));
      })
      .catch((error) => {
        this.onError(error instanceof Error ? error.message : '无法同步房间记录');
      })
      .finally(() => {
        this.historySync = null;
      });
    return this.historySync;
  }

  private resetLatency(detail: RoomDetail) {
    this.monitor?.reset();
    this.latencyIds.clear();
    detail.utterances
      .slice()
      .reverse()
      .forEach((utterance) => this.addLatency(utterance.id, utterance.latency));
  }

  private addLatency(utteranceId: string, latency: RoomDetail['utterances'][number]['latency']) {
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
  }

  private updateLanguageButton() {
    if (!this.root) return;
    this.root.querySelector('.language-source-label')!.textContent =
      languageNames[this.sourceLanguage] ?? this.sourceLanguage;
    this.root.querySelector('.language-target-label')!.textContent =
      languageNames[this.targetLanguage] ?? this.targetLanguage;
  }

  private updatePublishPermission(canPublish: boolean) {
    this.canPublish = canPublish;
    this.voiceSession?.setCanPublish(canPublish);
    if (!this.root) return;
    const button = this.root.querySelector<HTMLButtonElement>('.record-button')!;
    button.disabled = !canPublish || this.connectionStatus !== 'connected';
    const role = this.root.querySelector<HTMLElement>('.room-role');
    if (role && !this.room?.is_owner) {
      role.textContent = canPublish ? '成员发言' : '已被禁言';
      role.classList.toggle('muted', !canPublish);
    }
    if (!this.recording && this.connectionStatus === 'connected') {
      this.root.querySelector('.capture-state')!.textContent = canPublish ? '就绪' : '已被禁言';
      this.root.querySelector('.session-badge-text')!.textContent = canPublish ? '等待录音' : '已被禁言';
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

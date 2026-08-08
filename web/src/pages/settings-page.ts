import { loadAppConfig, saveAppConfig } from '../app-config';
import {
  createVoiceReference,
  deleteVoiceReference,
  listTtsVoices,
  listVoiceReferences,
  type TtsVoiceCatalog,
  type User,
  type VoiceReference,
} from '../api';
import { refreshIcons } from '../components/icons';
import { isCustomVoice, loadPreferences, savePreferences } from '../shared/preferences';
import {
  defaultSubtitlePreferences,
  loadSubtitlePreferences,
  matchingSubtitlePreset,
  saveSubtitlePreferences,
  subscribeSubtitlePreferences,
  subtitlePresets,
  type SubtitlePreferences,
} from '../shared/subtitle-preferences';
import {
  prepareVoiceReference,
  type PreparedVoiceReference,
} from '../shared/voice-reference-audio';
import type { Page } from './page';

export class SettingsPage implements Page {
  private root: HTMLElement | null = null;
  private subtitleSavedTimer = 0;
  private unsubscribeSubtitlePreferences = () => {};
  private voiceReferences: VoiceReference[] = [];
  private ttsVoiceCatalog: TtsVoiceCatalog | null = null;
  private pendingVoiceReference: PreparedVoiceReference | null = null;
  private pendingVoiceReferenceUrl = '';
  private mediaRecorder: MediaRecorder | null = null;
  private recordingStream: MediaStream | null = null;
  private recordingTimer = 0;

  constructor(
    private readonly user: User,
    private readonly onServerChanged: () => void,
  ) {}

  mount(root: HTMLElement) {
    this.root = root;
    const preferences = loadPreferences(this.user.id);
    const avatar = escapeHtml(Array.from(this.user.username)[0]?.toUpperCase() ?? 'V');
    root.innerHTML = `
      <main class="profile-page app-shell">
        <section class="profile-page-heading">
          <span class="section-kicker"><i data-lucide="user-round"></i> ACCOUNT</span>
          <h1>我的</h1>
          <p>管理个人音色、设备偏好与字幕大屏显示。</p>
        </section>
        <div class="profile-layout">
          <aside class="profile-sidebar" aria-label="用户信息">
            <div class="profile-identity">
              <span class="profile-avatar" aria-hidden="true">${avatar}</span>
              <div><strong>${escapeHtml(this.user.username)}</strong><span>Voice Elf 用户</span></div>
            </div>
            <dl class="profile-details">
              <div><dt>用户 ID</dt><dd title="${this.user.id}">${escapeHtml(shortUserId(this.user.id))}</dd></div>
              <div><dt>加入时间</dt><dd>${formatAccountDate(this.user.created_at)}</dd></div>
            </dl>
            <div class="profile-security-note"><i data-lucide="shield-check"></i><span><strong>访问受保护</strong><small>仅可查看你创建或参加过的会议</small></span></div>
            <nav class="profile-section-nav" aria-label="设置分区">
              <a href="#user-settings">用户设置</a>
              <a href="#general-settings">通用设置</a>
              <a href="#subtitle-settings">字幕大屏</a>
            </nav>
          </aside>
          <section class="settings-form" aria-label="个人与应用设置">
          <div class="settings-category-heading" id="user-settings"><span>用户设置</span><h2>语音与播报</h2></div>
          <div class="settings-section-heading"><i data-lucide="volume-2"></i><div><strong>翻译播报</strong><span>选择播报声音并设置自动播放</span></div></div>
          <label class="settings-field"><span>播报人</span><select class="settings-voice"><option value="">正在加载可用音色...</option></select><small class="voice-preview"></small></label>
          <section class="voice-reference-settings" aria-labelledby="voice-reference-title">
            <div class="voice-reference-heading"><div><strong id="voice-reference-title">我的声音</strong><small>录制或上传 3–15 秒的人声参考</small></div><span class="voice-reference-count">0 / 5</span></div>
            <label class="voice-reference-name-field"><span>音色名称</span><input class="voice-reference-name" maxlength="32" autocomplete="off" placeholder="例如：我的会议声线" required></label>
            <div class="voice-reference-actions">
              <button class="secondary-command voice-reference-record" type="button"><i data-lucide="mic"></i><span>录制</span></button>
              <button class="secondary-command voice-reference-upload" type="button"><i data-lucide="upload"></i><span>上传音频</span></button>
              <input class="voice-reference-file" type="file" accept="audio/*" hidden>
              <span class="voice-reference-status" role="status" aria-live="polite"></span>
            </div>
            <div class="voice-reference-preview" hidden>
              <audio controls preload="metadata"></audio>
              <button class="primary-command compact voice-reference-save" type="button"><i data-lucide="save"></i><span>保存音色</span></button>
            </div>
            <div class="voice-reference-list" aria-live="polite"><span class="voice-reference-empty">正在加载...</span></div>
          </section>
          <label class="toggle-field">
            <span><strong>自动播报</strong><small>译声生成后立即通过扬声器播放</small></span>
            <input class="settings-autoplay" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <div class="settings-category-heading" id="general-settings"><span>通用设置</span><h2>设备与采集</h2></div>
          <div class="settings-section-heading"><i data-lucide="audio-waveform"></i><div><strong>音频采集</strong><span>控制当前设备的浏览器人声检测方式</span></div></div>
          <label class="toggle-field">
            <span><strong>增强分段过滤</strong><small>过滤持续低频嗡声、宽带噪声和短促非人声触发</small></span>
            <input class="settings-voice-filter" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <label class="toggle-field">
            <span><strong>系统降噪</strong><small>抑制风扇、空调等持续背景噪声；外置声卡已降噪时可关闭</small></span>
            <input class="settings-noise-suppression" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <label class="toggle-field">
            <span><strong>回声消除</strong><small>减少扬声器声音被麦克风再次采集；使用耳机时可关闭</small></span>
            <input class="settings-echo-cancellation" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <div class="settings-divider"></div>
          <div class="settings-section-heading" id="subtitle-settings"><i data-lucide="captions"></i><div><strong>字幕大屏</strong><span>设置独立字幕页和 App 悬浮窗的实时显示样式</span></div></div>
          <div class="subtitle-settings-controls">
            <fieldset class="subtitle-setting-group">
              <legend>展示内容</legend>
              <div class="subtitle-mode-control">
                <label><input type="radio" name="subtitle-display" value="both"><span>原文 + 译文</span></label>
                <label><input type="radio" name="subtitle-display" value="source"><span>仅原文</span></label>
                <label><input type="radio" name="subtitle-display" value="translation"><span>仅译文</span></label>
              </div>
            </fieldset>
            <fieldset class="subtitle-setting-group">
              <legend>全文对齐方式</legend>
              <div class="subtitle-mode-control">
                <label><input type="radio" name="subtitle-alignment" value="top"><span>居上</span></label>
                <label><input type="radio" name="subtitle-alignment" value="center"><span>居中</span></label>
                <label><input type="radio" name="subtitle-alignment" value="bottom"><span>居下</span></label>
              </div>
            </fieldset>
            <fieldset class="subtitle-setting-group">
              <legend>视觉方案</legend>
              <div class="subtitle-preset-control">
                ${subtitlePresets.map((preset) => `<button type="button" data-subtitle-preset="${preset.id}"><span class="subtitle-preset-swatches"><i style="background:${preset.values.backgroundColor}"></i><i style="background:${preset.values.sourceColor}"></i><i style="background:${preset.values.translationColor}"></i></span><span><strong>${preset.name}</strong><small>${preset.description}</small></span></button>`).join('')}
              </div>
            </fieldset>
            <div class="subtitle-color-grid">
              <label class="subtitle-color-field"><span>背景色</span><span class="subtitle-color-input"><input type="color" data-subtitle-key="backgroundColor"><output></output></span></label>
              <label class="subtitle-color-field"><span>原文颜色</span><span class="subtitle-color-input"><input type="color" data-subtitle-key="sourceColor"><output></output></span></label>
              <label class="subtitle-color-field"><span>译文颜色</span><span class="subtitle-color-input"><input type="color" data-subtitle-key="translationColor"><output></output></span></label>
            </div>
            <div class="subtitle-range-grid">
              ${rangeControl('sourceFontSize', '原文字号', 24, 88, 1, 'px')}
              ${rangeControl('translationFontSize', '译文字号', 20, 76, 1, 'px')}
              ${rangeControl('lineHeight', '文字行距', 1, 1.8, 0.05, '')}
              ${rangeControl('blockGap', '字幕间距', 0, 48, 1, 'px')}
              ${rangeControl('screenPadding', '画面边距', 12, 96, 1, 'px')}
            </div>
            <div class="subtitle-preview" aria-label="字幕样式预览">
              <span>陈晨</span>
              <p class="subtitle-preview-source">欢迎参加今天的产品会议。</p>
              <p class="subtitle-preview-translation">Welcome to today's product meeting.</p>
            </div>
            <div class="subtitle-settings-footer">
              <button class="secondary-command subtitle-reset" type="button"><i data-lucide="rotate-ccw"></i><span>恢复默认</span></button>
              <span class="settings-saved subtitle-saved" role="status" aria-live="polite"></span>
            </div>
          </div>
          <div class="app-server-settings" hidden>
            <div class="settings-section-heading"><i data-lucide="server"></i><div><strong>服务连接</strong><span>设置 App 请求的 Voice Elf API 地址</span></div></div>
            <label class="settings-field settings-api-field"><span>API 地址</span><input class="settings-api-url" type="url" inputmode="url" autocomplete="url" autocapitalize="none" spellcheck="false" placeholder="https://voice.example.com" required><small>切换服务后需要重新登录</small></label>
            <div class="settings-server-actions">
              <button class="primary-command compact settings-server-save" type="button"><i data-lucide="save"></i><span>保存地址</span></button>
              <span class="settings-server-status" role="status" aria-live="polite"></span>
            </div>
          </div>
          <div class="settings-saved playback-saved" role="status" aria-live="polite"></div>
          </section>
        </div>
      </main>
    `;
    const voice = root.querySelector<HTMLSelectElement>('.settings-voice')!;
    const autoplay = root.querySelector<HTMLInputElement>('.settings-autoplay')!;
    const enhancedVoiceFilter = root.querySelector<HTMLInputElement>('.settings-voice-filter')!;
    const noiseSuppression = root.querySelector<HTMLInputElement>('.settings-noise-suppression')!;
    const echoCancellation = root.querySelector<HTMLInputElement>('.settings-echo-cancellation')!;
    voice.value = '';
    autoplay.checked = preferences.autoplay;
    enhancedVoiceFilter.checked = preferences.enhancedVoiceFilter;
    noiseSuppression.checked = preferences.noiseSuppression;
    echoCancellation.checked = preferences.echoCancellation;
    const renderVoiceDescription = () => {
      const custom = this.voiceReferences.find(
        (reference) => customVoiceValue(reference.id) === voice.value,
      );
      root.querySelector('.voice-preview')!.textContent =
        this.ttsVoiceCatalog?.voices.find((item) => item.id === voice.value)?.description ??
        (custom ? `${custom.name} · 我的参考声` : '正在加载我的声音');
    };
    const persist = () => {
      savePreferences(this.user.id, {
        ...loadPreferences(this.user.id),
        voice: voice.value,
        autoplay: autoplay.checked,
        enhancedVoiceFilter: enhancedVoiceFilter.checked,
        noiseSuppression: noiseSuppression.checked,
        echoCancellation: echoCancellation.checked,
      });
      renderVoiceDescription();
      const saved = root.querySelector('.playback-saved')!;
      saved.textContent = '设置已保存';
      window.setTimeout(() => {
        if (this.root) saved.textContent = '';
      }, 1600);
    };
    voice.addEventListener('change', persist);
    autoplay.addEventListener('change', persist);
    enhancedVoiceFilter.addEventListener('change', persist);
    noiseSuppression.addEventListener('change', persist);
    echoCancellation.addEventListener('change', persist);
    renderVoiceDescription();
    refreshIcons(root);
    void this.mountVoiceReferences(root, voice, preferences.voice, persist, renderVoiceDescription);
    this.mountSubtitleSettings(root);
    void this.mountAppConfig(root);
  }

  destroy() {
    window.clearTimeout(this.subtitleSavedTimer);
    this.unsubscribeSubtitlePreferences();
    this.unsubscribeSubtitlePreferences = () => {};
    this.root = null;
    this.stopRecording(true);
    if (this.pendingVoiceReferenceUrl) URL.revokeObjectURL(this.pendingVoiceReferenceUrl);
    this.pendingVoiceReferenceUrl = '';
    this.pendingVoiceReference = null;
    this.voiceReferences = [];
    this.ttsVoiceCatalog = null;
  }

  private async mountVoiceReferences(
    root: HTMLElement,
    voice: HTMLSelectElement,
    preferredVoice: string,
    persist: () => void,
    renderVoiceDescription: () => void,
  ) {
    const nameInput = root.querySelector<HTMLInputElement>('.voice-reference-name')!;
    const recordButton = root.querySelector<HTMLButtonElement>('.voice-reference-record')!;
    const uploadButton = root.querySelector<HTMLButtonElement>('.voice-reference-upload')!;
    const fileInput = root.querySelector<HTMLInputElement>('.voice-reference-file')!;
    const saveButton = root.querySelector<HTMLButtonElement>('.voice-reference-save')!;

    recordButton.addEventListener('click', () => {
      if (this.mediaRecorder?.state === 'recording') {
        this.stopRecording();
        this.setRecordingButton(root, false);
        this.setVoiceReferenceStatus(root, '正在处理录音...');
      } else {
        void this.startRecording(root);
      }
    });
    uploadButton.addEventListener('click', () => fileInput.click());
    fileInput.addEventListener('change', () => {
      const file = fileInput.files?.[0];
      fileInput.value = '';
      if (!file) return;
      if (!nameInput.value.trim()) nameInput.value = file.name.replace(/\.[^.]+$/, '').slice(0, 32);
      void this.preparePendingVoiceReference(root, file);
    });
    saveButton.addEventListener('click', async () => {
      if (!this.pendingVoiceReference || !nameInput.reportValidity()) return;
      saveButton.disabled = true;
      this.setVoiceReferenceStatus(root, '正在保存...');
      try {
        const reference = await createVoiceReference(
          nameInput.value.trim(),
          this.pendingVoiceReference.wav,
        );
        if (this.root !== root) return;
        this.voiceReferences = [reference, ...this.voiceReferences];
        this.clearPendingVoiceReference(root);
        nameInput.value = '';
        this.renderVoiceReferenceOptions(voice, customVoiceValue(reference.id));
        this.renderVoiceReferenceList(root, voice, persist);
        persist();
        this.setVoiceReferenceStatus(root, '音色已保存并选用');
      } catch (error) {
        this.setVoiceReferenceStatus(root, errorMessage(error, '保存音色失败'), true);
      } finally {
        if (this.root === root) {
          saveButton.disabled = this.voiceReferences.length >= 5 || !this.pendingVoiceReference;
        }
      }
    });

    try {
      const [catalog, references] = await Promise.all([listTtsVoices(), listVoiceReferences()]);
      if (this.root !== root) return;
      this.ttsVoiceCatalog = catalog;
      this.voiceReferences = references;
      this.renderProviderVoiceOptions(voice);
      const selected = this.renderVoiceReferenceOptions(voice, preferredVoice);
      this.renderVoiceReferenceList(root, voice, persist);
      renderVoiceDescription();
      if (!selected && isCustomVoice(preferredVoice)) persist();
    } catch (error) {
      if (this.root !== root) return;
      this.setVoiceReferenceStatus(root, errorMessage(error, '无法加载我的声音'), true);
      root.querySelector('.voice-reference-list')!.innerHTML =
        '<span class="voice-reference-empty">加载失败</span>';
    }
  }

  private renderProviderVoiceOptions(voice: HTMLSelectElement) {
    voice.replaceChildren();
    const catalog = this.ttsVoiceCatalog;
    if (!catalog?.voices.length) {
      const option = document.createElement('option');
      option.value = '';
      option.textContent = '当前 TTS 没有可用音色';
      option.disabled = true;
      voice.append(option);
      return;
    }
    const groups = new Map<string, typeof catalog.voices>();
    for (const item of catalog.voices) {
      const group = groups.get(item.group) ?? [];
      group.push(item);
      groups.set(item.group, group);
    }
    for (const [label, items] of groups) {
      const group = document.createElement('optgroup');
      group.label = label;
      for (const item of items) {
        const option = document.createElement('option');
        option.value = item.id;
        option.textContent = item.display_name;
        option.title = item.description;
        group.append(option);
      }
      voice.append(group);
    }
  }

  private renderVoiceReferenceOptions(voice: HTMLSelectElement, selectedValue: string) {
    voice.querySelector('optgroup[data-custom-voices]')?.remove();
    if (this.voiceReferences.length && this.ttsVoiceCatalog?.supports_custom_voices) {
      const group = document.createElement('optgroup');
      group.label = '我的声音';
      group.dataset.customVoices = '';
      for (const reference of this.voiceReferences) {
        const option = document.createElement('option');
        option.value = customVoiceValue(reference.id);
        option.textContent = reference.name;
        group.append(option);
      }
      voice.append(group);
    }
    const exists =
      this.ttsVoiceCatalog?.voices.some((item) => item.id === selectedValue) ||
      (this.ttsVoiceCatalog?.supports_custom_voices &&
        this.voiceReferences.some((reference) => customVoiceValue(reference.id) === selectedValue));
    const fallback = this.ttsVoiceCatalog?.voices[0]?.id ?? '';
    voice.value = exists ? selectedValue : fallback;
    return exists;
  }

  private renderVoiceReferenceList(
    root: HTMLElement,
    voice: HTMLSelectElement,
    persist: () => void,
  ) {
    const list = root.querySelector<HTMLElement>('.voice-reference-list')!;
    const count = root.querySelector<HTMLElement>('.voice-reference-count')!;
    const atLimit = this.voiceReferences.length >= 5;
    count.textContent = `${this.voiceReferences.length} / 5`;
    root.querySelector<HTMLButtonElement>('.voice-reference-record')!.disabled = atLimit;
    root.querySelector<HTMLButtonElement>('.voice-reference-upload')!.disabled = atLimit;
    root.querySelector<HTMLButtonElement>('.voice-reference-save')!.disabled =
      atLimit || !this.pendingVoiceReference;
    if (!this.voiceReferences.length) {
      list.innerHTML = '<span class="voice-reference-empty">尚未保存自定义音色</span>';
      return;
    }
    list.innerHTML = this.voiceReferences
      .map(
        (reference) => `
          <div class="voice-reference-item" data-voice-reference-id="${reference.id}">
            <div class="voice-reference-meta"><strong>${escapeHtml(reference.name)}</strong><span>${formatDuration(reference.duration_ms)}</span></div>
            <audio controls preload="none" src="${reference.audio_url}"></audio>
            <button class="secondary-command voice-reference-use" type="button">使用</button>
            <button class="icon-button danger voice-reference-delete" type="button" title="删除音色" aria-label="删除 ${escapeHtml(reference.name)}"><i data-lucide="trash-2"></i></button>
          </div>`,
      )
      .join('');
    list.querySelectorAll<HTMLElement>('.voice-reference-item').forEach((item) => {
      const id = item.dataset.voiceReferenceId!;
      item.querySelector('.voice-reference-use')?.addEventListener('click', () => {
        voice.value = customVoiceValue(id);
        persist();
      });
      item.querySelector('.voice-reference-delete')?.addEventListener('click', () => {
        void this.removeVoiceReference(root, voice, id, persist);
      });
    });
    refreshIcons(list);
  }

  private async removeVoiceReference(
    root: HTMLElement,
    voice: HTMLSelectElement,
    id: string,
    persist: () => void,
  ) {
    const reference = this.voiceReferences.find((item) => item.id === id);
    if (!reference || !window.confirm(`删除音色“${reference.name}”？`)) return;
    this.setVoiceReferenceStatus(root, '正在删除...');
    try {
      await deleteVoiceReference(id);
      if (this.root !== root) return;
      const wasSelected = voice.value === customVoiceValue(id);
      this.voiceReferences = this.voiceReferences.filter((item) => item.id !== id);
      this.renderVoiceReferenceOptions(voice, wasSelected ? 'F1' : voice.value);
      this.renderVoiceReferenceList(root, voice, persist);
      if (wasSelected) persist();
      this.setVoiceReferenceStatus(root, '音色已删除');
    } catch (error) {
      this.setVoiceReferenceStatus(root, errorMessage(error, '删除音色失败'), true);
    }
  }

  private async startRecording(root: HTMLElement) {
    if (!navigator.mediaDevices?.getUserMedia || !('MediaRecorder' in window)) {
      this.setVoiceReferenceStatus(root, '当前环境不支持录音，请上传音频', true);
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: loadPreferences(this.user.id).echoCancellation,
          noiseSuppression: loadPreferences(this.user.id).noiseSuppression,
          autoGainControl: false,
        },
      });
      if (this.root !== root) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      const mimeType = ['audio/webm;codecs=opus', 'audio/mp4', 'audio/webm'].find((type) =>
        MediaRecorder.isTypeSupported(type),
      );
      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
      const chunks: Blob[] = [];
      recorder.addEventListener('dataavailable', (event) => {
        if (event.data.size) chunks.push(event.data);
      });
      recorder.addEventListener('stop', () => {
        stream.getTracks().forEach((track) => track.stop());
        this.recordingStream = null;
        this.mediaRecorder = null;
        if (this.root !== root) return;
        this.setRecordingButton(root, false);
        void this.preparePendingVoiceReference(root, new Blob(chunks, { type: recorder.mimeType }));
      });
      this.recordingStream = stream;
      this.mediaRecorder = recorder;
      recorder.start(250);
      const startedAt = performance.now();
      this.setRecordingButton(root, true);
      this.recordingTimer = window.setInterval(() => {
        const seconds = (performance.now() - startedAt) / 1_000;
        this.setVoiceReferenceStatus(root, `录制中 ${seconds.toFixed(1)} 秒`);
        if (seconds >= 15) this.stopRecording();
      }, 100);
    } catch (error) {
      this.setRecordingButton(root, false);
      this.setVoiceReferenceStatus(root, errorMessage(error, '无法使用麦克风'), true);
    }
  }

  private stopRecording(cancel = false) {
    window.clearInterval(this.recordingTimer);
    this.recordingTimer = 0;
    const recorder = this.mediaRecorder;
    if (recorder?.state === 'recording') {
      recorder.stop();
    }
    if (cancel) {
      this.recordingStream?.getTracks().forEach((track) => track.stop());
      this.recordingStream = null;
      this.mediaRecorder = null;
    }
  }

  private async preparePendingVoiceReference(root: HTMLElement, source: Blob) {
    this.setVoiceReferenceStatus(root, '正在处理音频...');
    try {
      const prepared = await prepareVoiceReference(source);
      if (this.root !== root) return;
      this.pendingVoiceReference = prepared;
      if (this.pendingVoiceReferenceUrl) URL.revokeObjectURL(this.pendingVoiceReferenceUrl);
      this.pendingVoiceReferenceUrl = URL.createObjectURL(prepared.wav);
      const preview = root.querySelector<HTMLElement>('.voice-reference-preview')!;
      const audio = preview.querySelector<HTMLAudioElement>('audio')!;
      audio.src = this.pendingVoiceReferenceUrl;
      preview.hidden = false;
      root.querySelector<HTMLButtonElement>('.voice-reference-save')!.disabled =
        this.voiceReferences.length >= 5;
      this.setVoiceReferenceStatus(root, `${prepared.durationSeconds.toFixed(1)} 秒 · 可以保存`);
    } catch (error) {
      this.setVoiceReferenceStatus(root, errorMessage(error, '无法处理音频'), true);
    }
  }

  private clearPendingVoiceReference(root: HTMLElement) {
    this.pendingVoiceReference = null;
    if (this.pendingVoiceReferenceUrl) URL.revokeObjectURL(this.pendingVoiceReferenceUrl);
    this.pendingVoiceReferenceUrl = '';
    const preview = root.querySelector<HTMLElement>('.voice-reference-preview')!;
    preview.hidden = true;
    preview.querySelector<HTMLAudioElement>('audio')!.removeAttribute('src');
  }

  private setRecordingButton(root: HTMLElement, recording: boolean) {
    if (this.root !== root) return;
    const button = root.querySelector<HTMLButtonElement>('.voice-reference-record')!;
    button.classList.toggle('danger', recording);
    button.innerHTML = recording
      ? '<i data-lucide="circle-stop"></i><span>停止</span>'
      : '<i data-lucide="mic"></i><span>录制</span>';
    refreshIcons(button);
  }

  private setVoiceReferenceStatus(root: HTMLElement, message: string, error = false) {
    if (this.root !== root) return;
    const status = root.querySelector<HTMLElement>('.voice-reference-status')!;
    status.textContent = message;
    status.classList.toggle('error', error);
  }

  private mountSubtitleSettings(root: HTMLElement) {
    const controls = root.querySelector<HTMLElement>('.subtitle-settings-controls')!;
    let preferences = loadSubtitlePreferences(this.user.id);
    const render = (next: SubtitlePreferences) => {
      preferences = next;
      controls.querySelectorAll<HTMLInputElement>('input[name="subtitle-display"]').forEach((input) => {
        input.checked = input.value === next.displayMode;
      });
      controls.querySelectorAll<HTMLInputElement>('input[name="subtitle-alignment"]').forEach((input) => {
        input.checked = input.value === next.contentAlignment;
      });
      controls.querySelectorAll<HTMLInputElement>('input[data-subtitle-key]').forEach((input) => {
        const key = input.dataset.subtitleKey as keyof SubtitlePreferences;
        input.value = String(next[key]);
        const output = input.closest('label')?.querySelector('output');
        if (output) output.textContent = `${next[key]}${input.dataset.unit ?? ''}`;
      });
      const preset = matchingSubtitlePreset(next);
      controls.querySelectorAll<HTMLButtonElement>('[data-subtitle-preset]').forEach((button) => {
        button.classList.toggle('active', button.dataset.subtitlePreset === preset?.id);
      });
      const preview = controls.querySelector<HTMLElement>('.subtitle-preview')!;
      preview.dataset.displayMode = next.displayMode;
      preview.dataset.contentAlignment = next.contentAlignment;
      preview.style.setProperty('--preview-background', next.backgroundColor);
      preview.style.setProperty('--preview-source', next.sourceColor);
      preview.style.setProperty('--preview-translation', next.translationColor);
      preview.style.setProperty('--preview-source-size', `${Math.max(16, next.sourceFontSize * 0.46)}px`);
      preview.style.setProperty(
        '--preview-translation-size',
        `${Math.max(14, next.translationFontSize * 0.46)}px`,
      );
      preview.style.setProperty('--preview-line-height', String(next.lineHeight));
      preview.style.setProperty('--preview-gap', `${Math.min(18, next.blockGap * 0.55)}px`);
      preview.style.setProperty('--preview-padding', `${Math.min(32, next.screenPadding * 0.58)}px`);
    };
    const persist = () => {
      const displayMode = controls.querySelector<HTMLInputElement>(
        'input[name="subtitle-display"]:checked',
      )?.value as SubtitlePreferences['displayMode'];
      const contentAlignment = controls.querySelector<HTMLInputElement>(
        'input[name="subtitle-alignment"]:checked',
      )?.value as SubtitlePreferences['contentAlignment'];
      const next = { ...preferences, displayMode, contentAlignment };
      controls.querySelectorAll<HTMLInputElement>('input[data-subtitle-key]').forEach((input) => {
        const key = input.dataset.subtitleKey as keyof SubtitlePreferences;
        (next as Record<string, string | number>)[key] =
          input.type === 'color' ? input.value : Number(input.value);
      });
      render(saveSubtitlePreferences(this.user.id, next));
      const saved = controls.querySelector<HTMLElement>('.subtitle-saved')!;
      saved.textContent = '已自动保存并同步';
      window.clearTimeout(this.subtitleSavedTimer);
      this.subtitleSavedTimer = window.setTimeout(() => {
        if (this.root) saved.textContent = '';
      }, 1600);
    };
    controls.querySelectorAll<HTMLInputElement>('input[name="subtitle-display"]').forEach((input) =>
      input.addEventListener('change', persist),
    );
    controls.querySelectorAll<HTMLInputElement>('input[name="subtitle-alignment"]').forEach((input) =>
      input.addEventListener('change', persist),
    );
    controls.querySelectorAll<HTMLInputElement>('input[data-subtitle-key]').forEach((input) =>
      input.addEventListener('input', persist),
    );
    controls.querySelectorAll<HTMLButtonElement>('[data-subtitle-preset]').forEach((button) => {
      button.addEventListener('click', () => {
        const preset = subtitlePresets.find((item) => item.id === button.dataset.subtitlePreset);
        if (!preset) return;
        preferences = { ...preferences, ...preset.values };
        render(preferences);
        persist();
      });
    });
    controls.querySelector('.subtitle-reset')?.addEventListener('click', () => {
      render({ ...defaultSubtitlePreferences });
      preferences = { ...defaultSubtitlePreferences };
      persist();
    });
    render(preferences);
    this.unsubscribeSubtitlePreferences = subscribeSubtitlePreferences(this.user.id, render);
  }

  private async mountAppConfig(root: HTMLElement) {
    const config = await loadAppConfig();
    if (!config || this.root !== root) return;
    const section = root.querySelector<HTMLElement>('.app-server-settings')!;
    const input = section.querySelector<HTMLInputElement>('.settings-api-url')!;
    const save = section.querySelector<HTMLButtonElement>('.settings-server-save')!;
    const status = section.querySelector<HTMLElement>('.settings-server-status')!;
    section.hidden = false;
    input.value = config.api_url;
    save.addEventListener('click', async () => {
      status.classList.remove('error');
      status.textContent = '';
      if (!input.reportValidity()) return;
      save.disabled = true;
      try {
        const saved = await saveAppConfig(input.value);
        input.value = saved.api_url;
        status.textContent = '已保存，正在切换服务';
        this.onServerChanged();
      } catch (error) {
        status.classList.add('error');
        status.textContent = error instanceof Error ? error.message : '无法保存 API 地址';
      } finally {
        save.disabled = false;
      }
    });
    refreshIcons(section);
  }
}

function rangeControl(
  key: keyof SubtitlePreferences,
  label: string,
  minimum: number,
  maximum: number,
  step: number,
  unit: string,
) {
  return `<label class="subtitle-range-field"><span><strong>${label}</strong><output></output></span><input type="range" min="${minimum}" max="${maximum}" step="${step}" data-subtitle-key="${key}" data-unit="${unit}"></label>`;
}

function customVoiceValue(id: string) {
  return `custom:${id}`;
}

function formatDuration(durationMs: number) {
  return `${(durationMs / 1_000).toFixed(1)} 秒`;
}

function formatAccountDate(value: string) {
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  }).format(new Date(value));
}

function shortUserId(value: string) {
  return `${value.slice(0, 8)}…${value.slice(-4)}`;
}

function errorMessage(error: unknown, fallback: string) {
  return error instanceof Error ? error.message : fallback;
}

function escapeHtml(value: string) {
  return value.replace(
    /[&<>'"]/g,
    (character) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;' })[character]!,
  );
}

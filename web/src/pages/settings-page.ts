import { loadAppConfig, saveAppConfig } from '../app-config';
import { refreshIcons } from '../components/icons';
import { voiceOptions, voices } from '../shared/languages';
import { loadPreferences, savePreferences } from '../shared/preferences';
import type { Page } from './page';

export class SettingsPage implements Page {
  private root: HTMLElement | null = null;

  constructor(
    private readonly userId: string,
    private readonly onRooms: () => void,
    private readonly onServerChanged: () => void,
  ) {}

  mount(root: HTMLElement) {
    this.root = root;
    const preferences = loadPreferences(this.userId);
    root.innerHTML = `
      <main class="settings-page app-shell">
        <section class="settings-heading">
          <button class="icon-button settings-back" type="button" title="返回房间" aria-label="返回房间"><i data-lucide="arrow-left"></i></button>
          <div><span class="section-kicker"><i data-lucide="settings-2"></i> PLAYBACK SETTINGS</span><h1>设置</h1></div>
        </section>
        <section class="settings-form" aria-label="应用设置">
          <div class="settings-section-heading"><i data-lucide="volume-2"></i><div><strong>翻译播报</strong><span>设置译文语音与生成后是否自动播放</span></div></div>
          <label class="settings-field"><span>播报音色</span><select class="settings-voice">${voiceOptions()}</select><small class="voice-preview"></small></label>
          <label class="toggle-field">
            <span><strong>自动播报</strong><small>译声生成后立即通过扬声器播放</small></span>
            <input class="settings-autoplay" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <div class="settings-section-heading"><i data-lucide="audio-waveform"></i><div><strong>音频采集</strong><span>控制当前设备的浏览器人声检测方式</span></div></div>
          <label class="toggle-field">
            <span><strong>增强人声过滤</strong><small>过滤持续低频嗡声、宽带噪声和短促非人声</small></span>
            <input class="settings-voice-filter" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
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
      </main>
    `;
    const voice = root.querySelector<HTMLSelectElement>('.settings-voice')!;
    const autoplay = root.querySelector<HTMLInputElement>('.settings-autoplay')!;
    const enhancedVoiceFilter = root.querySelector<HTMLInputElement>('.settings-voice-filter')!;
    voice.value = preferences.voice;
    autoplay.checked = preferences.autoplay;
    enhancedVoiceFilter.checked = preferences.enhancedVoiceFilter;
    const persist = () => {
      savePreferences(this.userId, {
        voice: voice.value,
        autoplay: autoplay.checked,
        enhancedVoiceFilter: enhancedVoiceFilter.checked,
      });
      root.querySelector('.voice-preview')!.textContent = `${voices[voice.value] ?? voice.value} · 服务端 TTS`;
      const saved = root.querySelector('.playback-saved')!;
      saved.textContent = '设置已保存';
      window.setTimeout(() => {
        if (this.root) saved.textContent = '';
      }, 1600);
    };
    root.querySelector('.settings-back')?.addEventListener('click', this.onRooms);
    voice.addEventListener('change', persist);
    autoplay.addEventListener('change', persist);
    enhancedVoiceFilter.addEventListener('change', persist);
    persist();
    refreshIcons(root);
    void this.mountAppConfig(root);
  }

  destroy() {
    this.root = null;
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

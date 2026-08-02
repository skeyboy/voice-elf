import { refreshIcons } from '../components/icons';
import { voiceOptions, voices } from '../shared/languages';
import { loadPreferences, savePreferences } from '../shared/preferences';
import type { Page } from './page';

export class SettingsPage implements Page {
  private root: HTMLElement | null = null;

  constructor(
    private readonly userId: string,
    private readonly onRooms: () => void,
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
        <section class="settings-form" aria-label="语音播报设置">
          <div class="settings-section-heading"><i data-lucide="volume-2"></i><div><strong>翻译播报</strong><span>设置译文语音与生成后是否自动播放</span></div></div>
          <label class="settings-field"><span>播报音色</span><select class="settings-voice">${voiceOptions()}</select><small class="voice-preview"></small></label>
          <label class="toggle-field">
            <span><strong>自动播报</strong><small>译声生成后立即通过扬声器播放</small></span>
            <input class="settings-autoplay" type="checkbox" role="switch">
            <span class="toggle-track" aria-hidden="true"></span>
          </label>
          <div class="settings-saved" role="status" aria-live="polite"></div>
        </section>
      </main>
    `;
    const voice = root.querySelector<HTMLSelectElement>('.settings-voice')!;
    const autoplay = root.querySelector<HTMLInputElement>('.settings-autoplay')!;
    voice.value = preferences.voice;
    autoplay.checked = preferences.autoplay;
    const persist = () => {
      savePreferences(this.userId, { voice: voice.value, autoplay: autoplay.checked });
      root.querySelector('.voice-preview')!.textContent = `${voices[voice.value] ?? voice.value} · Supertonic Web TTS`;
      const saved = root.querySelector('.settings-saved')!;
      saved.textContent = '设置已保存';
      window.setTimeout(() => {
        if (this.root) saved.textContent = '';
      }, 1600);
    };
    root.querySelector('.settings-back')?.addEventListener('click', this.onRooms);
    voice.addEventListener('change', persist);
    autoplay.addEventListener('change', persist);
    persist();
    refreshIcons(root);
  }

  destroy() {
    this.root = null;
  }
}

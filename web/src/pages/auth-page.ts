import { apiRequest, type User } from '../api';
import { loadAppConfig, saveAppConfig } from '../app-config';
import { refreshIcons } from '../components/icons';
import type { Page } from './page';

export class AuthPage implements Page {
  private root: HTMLElement | null = null;
  private mode: 'login' | 'register' = 'login';

  constructor(private readonly onAuthenticated: (user: User) => void) {}

  mount(root: HTMLElement) {
    this.root = root;
    root.innerHTML = `
      <main class="auth-page">
        <form class="auth-panel">
          <div class="auth-brand"><span class="brand-mark"><i data-lucide="audio-lines"></i></span><strong>Voice Elf</strong></div>
          <div class="auth-mode" role="tablist">
            <button class="active" data-mode="login" type="button" role="tab">登录</button>
            <button data-mode="register" type="button" role="tab">注册</button>
          </div>
          <label><span>账号名称</span><input name="username" autocomplete="username" minlength="3" maxlength="32" required></label>
          <label><span>密码</span><input name="password" type="password" autocomplete="current-password" minlength="8" maxlength="128" required></label>
          <p class="form-error" role="alert"></p>
          <button class="primary-command auth-submit" type="submit">登录</button>
          <div class="auth-server" hidden>
            <button class="auth-server-toggle" type="button" aria-expanded="false">
              <span><i data-lucide="server"></i><span><strong>API 服务</strong><small class="auth-server-current"></small></span></span>
              <i data-lucide="chevron-down"></i>
            </button>
            <div class="auth-server-editor" hidden>
              <label><span>API 地址</span><input class="auth-api-url" type="url" inputmode="url" autocomplete="url" autocapitalize="none" spellcheck="false" placeholder="https://voice.example.com"></label>
              <button class="server-save" type="button"><i data-lucide="save"></i><span>保存地址</span></button>
              <p class="server-config-status" role="status" aria-live="polite"></p>
            </div>
          </div>
        </form>
      </main>
    `;
    const form = root.querySelector<HTMLFormElement>('form')!;
    root.querySelectorAll<HTMLButtonElement>('[data-mode]').forEach((button) => {
      button.addEventListener('click', () => this.setMode(button.dataset.mode as 'login' | 'register'));
    });
    form.addEventListener('submit', (event) => void this.submit(event, form));
    refreshIcons(root);
    void this.mountAppConfig(root);
  }

  destroy() {
    this.root = null;
  }

  private setMode(mode: 'login' | 'register') {
    if (!this.root) return;
    this.mode = mode;
    this.root.querySelectorAll<HTMLButtonElement>('[data-mode]').forEach((button) => {
      button.classList.toggle('active', button.dataset.mode === mode);
    });
    this.root.querySelector('.auth-submit')!.textContent = mode === 'login' ? '登录' : '创建账号';
    this.root.querySelector<HTMLInputElement>('[name="password"]')!.autocomplete =
      mode === 'login' ? 'current-password' : 'new-password';
    this.root.querySelector('.form-error')!.textContent = '';
  }

  private async submit(event: SubmitEvent, form: HTMLFormElement) {
    event.preventDefault();
    const submit = form.querySelector<HTMLButtonElement>('.auth-submit')!;
    const errorElement = form.querySelector<HTMLElement>('.form-error')!;
    const values = new FormData(form);
    submit.disabled = true;
    errorElement.textContent = '';
    try {
      const user = await apiRequest<User>(`/api/auth/${this.mode}`, {
        method: 'POST',
        body: JSON.stringify({
          username: values.get('username'),
          password: values.get('password'),
        }),
      });
      this.onAuthenticated(user);
    } catch (error) {
      errorElement.textContent = error instanceof Error ? error.message : '账号请求失败';
    } finally {
      submit.disabled = false;
    }
  }

  private async mountAppConfig(root: HTMLElement) {
    const config = await loadAppConfig();
    if (!config || this.root !== root) return;
    const section = root.querySelector<HTMLElement>('.auth-server')!;
    const toggle = section.querySelector<HTMLButtonElement>('.auth-server-toggle')!;
    const editor = section.querySelector<HTMLElement>('.auth-server-editor')!;
    const current = section.querySelector<HTMLElement>('.auth-server-current')!;
    const input = section.querySelector<HTMLInputElement>('.auth-api-url')!;
    const save = section.querySelector<HTMLButtonElement>('.server-save')!;
    const status = section.querySelector<HTMLElement>('.server-config-status')!;
    section.hidden = false;
    input.value = config.api_url;
    current.textContent = config.api_url;
    toggle.addEventListener('click', () => {
      const expanded = toggle.getAttribute('aria-expanded') === 'true';
      toggle.setAttribute('aria-expanded', String(!expanded));
      editor.hidden = expanded;
      if (!expanded) input.focus();
    });
    const persist = async () => {
      status.classList.remove('error');
      status.textContent = '';
      input.setCustomValidity(input.value.trim() ? '' : '请输入 API 地址');
      if (!input.reportValidity()) return;
      save.disabled = true;
      try {
        const saved = await saveAppConfig(input.value);
        input.value = saved.api_url;
        current.textContent = saved.api_url;
        status.textContent = '已保存并切换到新地址';
      } catch (error) {
        status.classList.add('error');
        status.textContent = error instanceof Error ? error.message : '无法保存 API 地址';
      } finally {
        save.disabled = false;
      }
    };
    input.addEventListener('input', () => input.setCustomValidity(''));
    input.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter') return;
      event.preventDefault();
      void persist();
    });
    save.addEventListener('click', () => void persist());
    refreshIcons(section);
  }
}

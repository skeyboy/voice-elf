import { apiRequest, type User } from '../api';
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
        </form>
      </main>
    `;
    const form = root.querySelector<HTMLFormElement>('form')!;
    root.querySelectorAll<HTMLButtonElement>('[data-mode]').forEach((button) => {
      button.addEventListener('click', () => this.setMode(button.dataset.mode as 'login' | 'register'));
    });
    form.addEventListener('submit', (event) => void this.submit(event, form));
    refreshIcons(root);
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
}
